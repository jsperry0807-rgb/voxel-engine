//! Analytic 128³ scale-up model (M1-40).
//!
//! The gate measures the 64³ edge. M1-42 has to decide whether a 128³ build is
//! viable before one exists, so this module projects the measured p95 terms
//! instead of multiplying every number by eight.
//!
//! # Fixed and variable terms
//!
//! The edit report (M1-37) already splits one edit into:
//!
//! | Term | Components | Scaling at 128³ |
//! | --- | --- | --- |
//! | **Fixed** | upload + fence + flip | The fence and flip are per batch. Upload follows the COW path, which gains one level (`8` vs `7`), not a whole-chunk transfer. |
//! | **Dirty (variable)** | COW + light | Depends on the dirty region. An optimistic single-voxel edit still touches one path and one 16³ block cell; a pessimistic edit whose dirty region spans the chunk touches the whole chunk. |
//!
//! The projection is deliberately two-sided:
//!
//! - **Optimistic.** A single-voxel edit stays a single-voxel edit. COW and
//!   upload grow with path depth (`8/7`); light stays at the measured 27 block
//!   cells (one dirty cell plus its apron); the mesh grows with surface area
//!   (`×4`); the affected probe set grows with the local surface (`×4`).
//! - **Pessimistic.** The dirty region spans the chunk. COW grows with chunk
//!   volume (`×8`), light evaluates every 128³ block cell (`512`, i.e.
//!   `512/27` of the measured 27 cells), the probe update rebakes the whole
//!   512-probe grid (`32,768` rays), and memory grows with volume for every
//!   component rather than letting the mesh scale with area.
//!
//! The fixed term is identical in both projections: a dirty span cannot make
//! the fence, the flip or the per-edit upload path longer.
//!
//! # Reproducing the 64³ measurement
//!
//! [`project`] returns the measured terms unchanged when source and target are
//! the same edge. That is the calibration check in the tests: fed 64³ inputs
//! and asked for 64³, the model reproduces every measured p95 exactly. The
//! projection only starts scaling when the target edge is larger.
//!
//! # What this is not
//!
//! This is an analytic model, not a 128³ measurement. Components are projected
//! independently and summed; the sum of projected p95s is not itself a p95 of
//! sums. The model also cannot see the real 128³ content distribution, so the
//! fixture remains provisional and M1-42 must re-run after a real save.

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use voxel_core::constants::{BLOCK_DIM, CHUNK_BITS, TARGET_CHUNK_BITS};

use crate::save_scan::Distribution;

use super::edits::{self, BUDGET_NS as EDIT_BUDGET_NS, EditsReport};
use super::memory::{self, FixtureInfo, MemoryReport, NODE_BYTES};
use super::probes::{self, BUDGET_NS as PROBE_BUDGET_NS, ProbesReport};
use super::report::{NODE_BUDGET_64, NODE_BUDGET_128, VOXEL_FAIL_MIB, VOXEL_TARGET_MIB};
use super::residency::{self, ResidencyConfig, ResidencyReport};
use super::stat::{format_duration, format_signed_duration, grouped};

/// Schema version of the scale-up report itself.
pub const SCHEMA_VERSION: u32 = 1;

/// Bytes in one MiB.
const MIB: u64 = 1024 * 1024;

/// The two sides of the analytic projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionMode {
    /// A single-voxel edit remains local; mesh and probe work grow with area.
    Optimistic,
    /// The dirty region spans the chunk; content work grows with volume.
    Pessimistic,
}

impl ProjectionMode {
    /// Lower-case name used on the command line and in JSON.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Optimistic => "optimistic",
            Self::Pessimistic => "pessimistic",
        }
    }
}

/// An exact integer scale factor, applied with nearest-integer rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    /// Scale numerator.
    pub numerator: u64,
    /// Scale denominator; never zero.
    pub denominator: u64,
}

impl Ratio {
    /// Builds an exact ratio. The denominator must be non-zero.
    pub const fn new(numerator: u64, denominator: u64) -> Self {
        assert!(denominator != 0, "a scale ratio needs a denominator");
        Self {
            numerator,
            denominator,
        }
    }

    /// The identity ratio.
    pub const fn identity() -> Self {
        Self::new(1, 1)
    }

    /// Applies the ratio to `value` with nearest-integer, half-up rounding.
    pub fn apply(self, value: u64) -> u64 {
        let scaled = u128::from(value) * u128::from(self.numerator);
        let rounded = (scaled + u128::from(self.denominator) / 2) / u128::from(self.denominator);
        u64::try_from(rounded).unwrap_or(u64::MAX)
    }

    /// Floating-point view for report text; the arithmetic itself stays integer.
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
}

/// One chunk edge and the fixed per-chunk volumes derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// `log2` of the chunk edge in voxels.
    pub chunk_bits: u32,
    /// Voxels per chunk edge.
    pub chunk_dim: u32,
    /// Voxels in one chunk.
    pub chunk_volume: u64,
    /// Internal levels, including the root and the one-voxel level.
    pub levels: u32,
    /// Light cells in one chunk (one per 16³ block).
    pub light_cells: u64,
    /// Near-LOD probes in one chunk (one per 16³ block).
    pub probes: u64,
    /// SVDAG node cap for this edge.
    pub node_budget: u64,
}

impl Edge {
    /// Derives the edge geometry from `log2(chunk edge)`.
    pub fn at(chunk_bits: u32) -> Self {
        let chunk_dim = 1u32
            .checked_shl(chunk_bits)
            .expect("chunk bits fit in a chunk edge");
        let dim = u64::from(chunk_dim);
        let blocks_per_edge = u64::from(chunk_dim / BLOCK_DIM as u32);
        let blocks = blocks_per_edge * blocks_per_edge * blocks_per_edge;
        let node_budget = if chunk_bits == TARGET_CHUNK_BITS {
            NODE_BUDGET_128
        } else {
            NODE_BUDGET_64
        };
        Self {
            chunk_bits,
            chunk_dim,
            chunk_volume: dim * dim * dim,
            levels: chunk_bits + 1,
            light_cells: blocks,
            probes: blocks,
            node_budget,
        }
    }
}

/// One p95 edit term and the bytes/cells that drove it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredEdit {
    /// Appended runs, leaves and `levelStart` copied to staging.
    pub upload_ns: u64,
    /// Batch fence boundary.
    pub fence_ns: u64,
    /// Version-index flip.
    pub flip_ns: u64,
    /// Measured p95 of `upload + fence + flip`.
    pub fixed_ns: u64,
    /// Banded `ChunkDag::apply_edits`.
    pub cow_ns: u64,
    /// Reference fill over the dirty cell and apron.
    pub light_ns: u64,
    /// Measured p95 of `cow + light`.
    pub dirty_ns: u64,
    /// Measured p95 of `fixed + dirty`.
    pub total_ns: u64,
    /// Uploaded bytes per edit.
    pub upload_bytes_per_edit: u64,
    /// COW bytes appended per edit.
    pub cow_bytes_per_edit: u64,
    /// Light cells evaluated per edit.
    pub light_cells_per_edit: u64,
}

/// One p95 probe-update term and the work that drove it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredProbes {
    /// Probe selection.
    pub select_ns: u64,
    /// Affected-probe rebake.
    pub rebake_ns: u64,
    /// Changed-record staging copy.
    pub upload_ns: u64,
    /// Measured p95 of `select + rebake + upload`.
    pub update_ns: u64,
    /// Mean affected probes per edit.
    pub affected_probes_per_edit: u64,
    /// Mean rays tested while selecting.
    pub rays_tested_per_edit: u64,
    /// Mean rays rebaked.
    pub rays_baked_per_edit: u64,
    /// Near probes in one chunk at the measured edge.
    pub probes_per_chunk: u64,
    /// Hemisphere rays per valid probe.
    pub rays_per_probe: u64,
}

/// One p95 per-chunk memory component and its DAG node count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredMemory {
    /// Banded DAG bytes per chunk.
    pub dag_bytes: u64,
    /// Allocated overlay bytes per chunk.
    pub overlay_bytes: u64,
    /// Fixed block-light bytes per chunk.
    pub light_bytes: u64,
    /// Fixed near-LOD probe bytes per chunk.
    pub probe_bytes: u64,
    /// Count-only greedy mesh estimate per chunk.
    pub mesh_bytes: u64,
    /// Measured p95 of the five-component total.
    pub total_bytes: u64,
    /// Banded internal nodes per chunk at p95.
    pub dag_nodes: u64,
}

/// Everything the projection consumes, measured at one edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measured {
    /// Edit fixed/variable terms.
    pub edit: MeasuredEdit,
    /// Probe-update terms.
    pub probes: MeasuredProbes,
    /// Per-chunk memory components.
    pub memory: MeasuredMemory,
}

impl Measured {
    /// Extracts the p95 inputs from the four gate harnesses.
    pub fn from_reports(
        memory: &MemoryReport,
        edits: &EditsReport,
        probes: &ProbesReport,
    ) -> Result<Self, String> {
        let p95 = |distribution: &memory::ComponentDistribution, name: &str| {
            distribution
                .p95
                .ok_or_else(|| format!("the fixture has no {name} p95 to project"))
        };

        if edits.edit_count == 0 {
            return Err("the edit report has no samples to project".to_owned());
        }
        if memory.chunks.is_empty() {
            return Err("the memory report has no chunks to project".to_owned());
        }

        let upload_bytes_per_edit = edits.bytes.upload_bytes / edits.edit_count as u64;
        let cow_bytes_per_edit = edits.bytes.cow_bytes / edits.edit_count as u64;

        let mut dag_node_bytes: Vec<u64> = memory
            .chunks
            .iter()
            .map(|chunk| chunk.dag_node_bytes)
            .collect();
        let dag_nodes = Distribution::from_values(&mut dag_node_bytes)
            .p95
            .unwrap_or(0)
            / NODE_BYTES;

        let measured = Self {
            edit: MeasuredEdit {
                upload_ns: edits.terms.upload.p95_ns,
                fence_ns: edits.terms.fence.p95_ns,
                flip_ns: edits.terms.flip.p95_ns,
                fixed_ns: edits.terms.fixed.p95_ns,
                cow_ns: edits.terms.cow.p95_ns,
                light_ns: edits.terms.light.p95_ns,
                dirty_ns: edits.terms.dirty.p95_ns,
                total_ns: edits.terms.total.p95_ns,
                upload_bytes_per_edit,
                cow_bytes_per_edit,
                light_cells_per_edit: edits.bytes.light_cells_per_edit,
            },
            probes: MeasuredProbes {
                select_ns: probes.terms.select.p95_ns,
                rebake_ns: probes.terms.rebake.p95_ns,
                upload_ns: probes.terms.upload.p95_ns,
                update_ns: probes.terms.update.p95_ns,
                affected_probes_per_edit: probes.counts.affected_per_edit,
                rays_tested_per_edit: probes.counts.rays_tested_per_edit,
                rays_baked_per_edit: probes.counts.rays_baked_per_edit,
                probes_per_chunk: probes.probes_per_chunk as u64,
                rays_per_probe: probes.rays_per_probe as u64,
            },
            memory: MeasuredMemory {
                dag_bytes: p95(&memory.components.dag, "DAG")?,
                overlay_bytes: p95(&memory.components.overlay, "overlay")?,
                light_bytes: p95(&memory.components.light, "light")?,
                probe_bytes: p95(&memory.components.probes, "probe")?,
                mesh_bytes: p95(&memory.components.mesh, "mesh")?,
                total_bytes: p95(&memory.components.total, "total")?,
                dag_nodes,
            },
        };
        measured.validate()?;
        Ok(measured)
    }

    /// Rejects inputs the projection cannot divide by.
    pub fn validate(&self) -> Result<(), String> {
        if self.edit.light_cells_per_edit == 0 {
            return Err("the edit report evaluated no light cells to project".to_owned());
        }
        if self.probes.probes_per_chunk == 0 || self.probes.rays_per_probe == 0 {
            return Err("the probe report has no near grid to project".to_owned());
        }
        if self.memory.dag_bytes == 0
            && self.memory.overlay_bytes == 0
            && self.memory.light_bytes == 0
            && self.memory.probe_bytes == 0
            && self.memory.mesh_bytes == 0
        {
            return Err("the memory report has no component bytes to project".to_owned());
        }
        Ok(())
    }
}

/// Projected edit terms at the target edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditProjection {
    /// Projected upload.
    pub upload_ns: u64,
    /// Fence, unchanged by the edge.
    pub fence_ns: u64,
    /// Flip, unchanged by the edge.
    pub flip_ns: u64,
    /// Projected `upload + fence + flip`.
    pub fixed_ns: u64,
    /// Projected COW.
    pub cow_ns: u64,
    /// Projected light.
    pub light_ns: u64,
    /// Projected `cow + light`.
    pub dirty_ns: u64,
    /// Projected `fixed + dirty`.
    pub total_ns: u64,
    /// Light cells the dirty region evaluates.
    pub light_cells: u64,
    /// Edit budget, 8 ms.
    pub budget_ns: u64,
    /// `budget - total`.
    pub margin_ns: i64,
    /// True when the projected total is inside the budget.
    pub within_budget: bool,
}

/// Projected probe-update terms at the target edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeProjection {
    /// Projected selection.
    pub select_ns: u64,
    /// Projected rebake.
    pub rebake_ns: u64,
    /// Projected staging copy.
    pub upload_ns: u64,
    /// Projected `select + rebake + upload`.
    pub update_ns: u64,
    /// Probes selected for rebake.
    pub affected_probes: u64,
    /// Rays cast by the rebake.
    pub rays_baked: u64,
    /// Probe budget, 4 ms.
    pub budget_ns: u64,
    /// `budget - update`.
    pub margin_ns: i64,
    /// True when the projected update is inside the budget.
    pub within_budget: bool,
}

/// Projected per-chunk memory and the loaded-volume total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProjection {
    /// Projected DAG bytes per chunk.
    pub dag_bytes_per_chunk: u64,
    /// Projected overlay bytes per chunk.
    pub overlay_bytes_per_chunk: u64,
    /// Projected light bytes per chunk.
    pub light_bytes_per_chunk: u64,
    /// Projected probe bytes per chunk.
    pub probe_bytes_per_chunk: u64,
    /// Projected mesh bytes per chunk.
    pub mesh_bytes_per_chunk: u64,
    /// Sum of the five projected components per chunk.
    pub total_bytes_per_chunk: u64,
    /// Projected banded internal nodes per chunk.
    pub dag_nodes_per_chunk: u64,
    /// Loaded chunks at the target edge.
    pub loaded_chunks: u64,
    /// `total_bytes_per_chunk × loaded_chunks`.
    pub projected_bytes: u64,
    /// 40% VRAM target.
    pub target_bytes: u64,
    /// 60% VRAM hard-fail line.
    pub fail_bytes: u64,
    /// True when the projection is inside the 40% target.
    pub within_target: bool,
    /// True when the projection is inside the hard-fail line.
    pub within_fail: bool,
    /// SVDAG node cap at the target edge.
    pub node_budget: u64,
    /// True when the projected node count is inside the cap.
    pub node_within_budget: bool,
}

/// One side of the scale-up projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    /// Optimistic or pessimistic.
    pub mode: ProjectionMode,
    /// True when source and target are the same edge, so the terms are measured.
    pub same_edge: bool,
    /// Edit terms.
    pub edit: EditProjection,
    /// Probe-update terms.
    pub probes: ProbeProjection,
    /// Per-chunk memory and loaded-volume total.
    pub memory: MemoryProjection,
}

/// The complete M1-40 scale-up report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleupReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Fixture identity and provenance.
    pub fixture: FixtureInfo,
    /// Edge the inputs were measured at.
    pub source: Edge,
    /// Edge being projected.
    pub target: Edge,
    /// Residency profile the loaded-chunk counts came from.
    pub profile: String,
    /// Loaded chunks at the source edge.
    pub n64: u64,
    /// Loaded chunks at the target edge, equal loaded volume.
    pub n128: u64,
    /// `source chunk volume / target chunk volume`.
    pub equal_volume_factor: u64,
    /// Measured p95 inputs.
    pub measured: Measured,
    /// Local single-voxel projection.
    pub optimistic: Projection,
    /// Dirty-spans-chunk projection.
    pub pessimistic: Projection,
    /// Notes a reader needs before quoting a projection.
    pub notes: Vec<String>,
}

impl ScaleupReport {
    /// True while the measured fixture is the synthetic M1 placeholder.
    pub fn is_provisional(&self) -> bool {
        self.fixture.provisional_until_first_real_save
    }

    /// Renders both projections as Markdown.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate scale-up (M1-40)\n").unwrap();
        writeln!(
            out,
            "**Fixture:** `{}` ({})\n",
            self.fixture.id, self.fixture.status
        )
        .unwrap();
        writeln!(out, "**Manifest:** `{}`\n", self.fixture.manifest).unwrap();
        writeln!(out, "**Save:** `{}`\n", self.fixture.save_dir).unwrap();
        writeln!(out, "**World hash:** `{}`\n", self.fixture.world_hash).unwrap();

        if self.is_provisional() {
            writeln!(
                out,
                "> **Provisional fixture.** {}\n",
                self.fixture.limitation
            )
            .unwrap();
        }

        writeln!(
            out,
            "**Source edge:** {}³ ({} bits), {} voxels/chunk, {} light cells, {} probes, {} levels\n",
            self.source.chunk_dim,
            self.source.chunk_bits,
            grouped(self.source.chunk_volume),
            grouped(self.source.light_cells),
            grouped(self.source.probes),
            self.source.levels,
        )
        .unwrap();
        writeln!(
            out,
            "**Target edge:** {}³ ({} bits), {} voxels/chunk, {} light cells, {} probes, {} levels\n",
            self.target.chunk_dim,
            self.target.chunk_bits,
            grouped(self.target.chunk_volume),
            grouped(self.target.light_cells),
            grouped(self.target.probes),
            self.target.levels,
        )
        .unwrap();
        writeln!(
            out,
            "**Profile:** `{}` — N128 = {} chunks at the target edge\n",
            self.profile,
            grouped(self.n128),
        )
        .unwrap();
        writeln!(
            out,
            "**Loaded volume:** N64 = {}; N128 = {}; {} = {} × N128\n",
            grouped(self.n64),
            grouped(self.n128),
            grouped(self.n64),
            grouped(self.equal_volume_factor),
        )
        .unwrap();

        writeln!(out, "## Measured 64³ input (p95)\n").unwrap();
        writeln!(out, "| Component | Measured |").unwrap();
        writeln!(out, "|---|---:|").unwrap();
        for (name, value) in [
            ("Edit upload", format_duration(self.measured.edit.upload_ns)),
            ("Edit fence", format_duration(self.measured.edit.fence_ns)),
            ("Edit flip", format_duration(self.measured.edit.flip_ns)),
            ("Edit fixed", format_duration(self.measured.edit.fixed_ns)),
            ("Edit COW", format_duration(self.measured.edit.cow_ns)),
            ("Edit light", format_duration(self.measured.edit.light_ns)),
            ("Edit dirty", format_duration(self.measured.edit.dirty_ns)),
            ("Edit total", format_duration(self.measured.edit.total_ns)),
            (
                "Probe select",
                format_duration(self.measured.probes.select_ns),
            ),
            (
                "Probe rebake",
                format_duration(self.measured.probes.rebake_ns),
            ),
            (
                "Probe upload",
                format_duration(self.measured.probes.upload_ns),
            ),
            (
                "Probe update",
                format_duration(self.measured.probes.update_ns),
            ),
            (
                "Memory total/chunk",
                human_bytes(self.measured.memory.total_bytes),
            ),
        ] {
            writeln!(out, "| {name} | {value} |").unwrap();
        }
        writeln!(
            out,
            "\n**Measured work:** {} B/edit uploaded, {} B/edit COW, {} light cells/edit, \
             {} affected probes/edit, {} rays/edit, {} nodes/chunk\n",
            grouped(self.measured.edit.upload_bytes_per_edit),
            grouped(self.measured.edit.cow_bytes_per_edit),
            grouped(self.measured.edit.light_cells_per_edit),
            grouped(self.measured.probes.affected_probes_per_edit),
            grouped(self.measured.probes.rays_baked_per_edit),
            grouped(self.measured.memory.dag_nodes),
        )
        .unwrap();

        writeln!(out, "## {}³ projection\n", self.target.chunk_dim).unwrap();
        writeln!(out, "| Term | Optimistic | Pessimistic | Limit |").unwrap();
        writeln!(out, "|---|---:|---:|---:|").unwrap();
        for (name, optimistic, pessimistic, limit) in [
            (
                "Edit fixed",
                format_duration(self.optimistic.edit.fixed_ns),
                format_duration(self.pessimistic.edit.fixed_ns),
                format_duration(self.optimistic.edit.budget_ns),
            ),
            (
                "Edit dirty",
                format_duration(self.optimistic.edit.dirty_ns),
                format_duration(self.pessimistic.edit.dirty_ns),
                "—".to_owned(),
            ),
            (
                "Edit total",
                format!(
                    "{} ({}, {})",
                    format_duration(self.optimistic.edit.total_ns),
                    if self.optimistic.edit.within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                    format_signed_duration(self.optimistic.edit.margin_ns),
                ),
                format!(
                    "{} ({}, {})",
                    format_duration(self.pessimistic.edit.total_ns),
                    if self.pessimistic.edit.within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                    format_signed_duration(self.pessimistic.edit.margin_ns),
                ),
                format_duration(self.optimistic.edit.budget_ns),
            ),
            (
                "Probe update",
                format!(
                    "{} ({}, {})",
                    format_duration(self.optimistic.probes.update_ns),
                    if self.optimistic.probes.within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                    format_signed_duration(self.optimistic.probes.margin_ns),
                ),
                format!(
                    "{} ({}, {})",
                    format_duration(self.pessimistic.probes.update_ns),
                    if self.pessimistic.probes.within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                    format_signed_duration(self.pessimistic.probes.margin_ns),
                ),
                format_duration(self.optimistic.probes.budget_ns),
            ),
            (
                "Memory projected",
                format!(
                    "{} ({})",
                    human_bytes(self.optimistic.memory.projected_bytes),
                    if self.optimistic.memory.within_fail {
                        "PASS"
                    } else {
                        "MISS"
                    },
                ),
                format!(
                    "{} ({})",
                    human_bytes(self.pessimistic.memory.projected_bytes),
                    if self.pessimistic.memory.within_fail {
                        "PASS"
                    } else {
                        "MISS"
                    },
                ),
                human_bytes(self.optimistic.memory.fail_bytes),
            ),
            (
                "DAG nodes/chunk",
                format!(
                    "{} ({})",
                    grouped(self.optimistic.memory.dag_nodes_per_chunk),
                    if self.optimistic.memory.node_within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                ),
                format!(
                    "{} ({})",
                    grouped(self.pessimistic.memory.dag_nodes_per_chunk),
                    if self.pessimistic.memory.node_within_budget {
                        "PASS"
                    } else {
                        "MISS"
                    },
                ),
                grouped(self.optimistic.memory.node_budget),
            ),
        ] {
            writeln!(out, "| {name} | {optimistic} | {pessimistic} | {limit} |").unwrap();
        }

        writeln!(
            out,
            "\n**Optimistic memory:** {} per chunk × {} chunks = {}; \
             target {} ({}), hard fail {} ({}).\n",
            human_bytes(self.optimistic.memory.total_bytes_per_chunk),
            grouped(self.optimistic.memory.loaded_chunks),
            human_bytes(self.optimistic.memory.projected_bytes),
            human_bytes(self.optimistic.memory.target_bytes),
            if self.optimistic.memory.within_target {
                "PASS"
            } else {
                "MISS"
            },
            human_bytes(self.optimistic.memory.fail_bytes),
            if self.optimistic.memory.within_fail {
                "PASS"
            } else {
                "MISS"
            },
        )
        .unwrap();
        writeln!(
            out,
            "**Pessimistic memory:** {} per chunk × {} chunks = {}; \
             target {} ({}), hard fail {} ({}).\n",
            human_bytes(self.pessimistic.memory.total_bytes_per_chunk),
            grouped(self.pessimistic.memory.loaded_chunks),
            human_bytes(self.pessimistic.memory.projected_bytes),
            human_bytes(self.pessimistic.memory.target_bytes),
            if self.pessimistic.memory.within_target {
                "PASS"
            } else {
                "MISS"
            },
            human_bytes(self.pessimistic.memory.fail_bytes),
            if self.pessimistic.memory.within_fail {
                "PASS"
            } else {
                "MISS"
            },
        )
        .unwrap();

        writeln!(
            out,
            "**Light cells:** optimistic {} (one dirty block cell plus apron); \
             pessimistic {} (the whole target chunk).\n",
            grouped(self.optimistic.edit.light_cells),
            grouped(self.pessimistic.edit.light_cells),
        )
        .unwrap();
        writeln!(
            out,
            "**Probe work:** optimistic {} probes, {} rays; pessimistic {} probes, {} rays.\n",
            grouped(self.optimistic.probes.affected_probes),
            grouped(self.optimistic.probes.rays_baked),
            grouped(self.pessimistic.probes.affected_probes),
            grouped(self.pessimistic.probes.rays_baked),
        )
        .unwrap();

        for note in &self.notes {
            writeln!(out, "\n- {note}").unwrap();
        }
        out
    }
}

/// Measures the default fixture and projects the default profile to the target edge.
pub fn default_report() -> Result<ScaleupReport, String> {
    report(&crate::fixture::default_manifest())
}

/// Measures `manifest` and projects the default view to the target edge.
pub fn report(manifest_path: &Path) -> Result<ScaleupReport, String> {
    report_for(manifest_path, ResidencyConfig::default())
}

/// Measures `manifest` and projects `config` to the target edge.
///
/// The per-chunk projection is independent of the profile; the loaded chunk
/// count, and therefore the total, is not. Pass `Minimum` to evaluate the
/// fallback profile before deciding the edge.
pub fn report_for(manifest_path: &Path, config: ResidencyConfig) -> Result<ScaleupReport, String> {
    let memory = memory::report(manifest_path)?;
    let residency = residency::report(config)?;
    let edits = edits::report(manifest_path)?;
    let probes = probes::report(manifest_path)?;
    from_reports(&memory, &residency, &edits, &probes)
}

/// Assembles the projection from already-measured harness reports.
pub fn from_reports(
    memory: &MemoryReport,
    residency: &ResidencyReport,
    edits: &EditsReport,
    probes: &ProbesReport,
) -> Result<ScaleupReport, String> {
    let measured = Measured::from_reports(memory, edits, probes)?;
    let source = Edge::at(CHUNK_BITS);
    let target = Edge::at(TARGET_CHUNK_BITS);
    let n64 = residency.baseline.loaded_chunks;
    let n128 = residency.target.loaded_chunks;
    let factor = residency.equal_volume.factor;
    if !residency.equal_volume.holds || n64 != factor * n128 {
        return Err(format!(
            "residency does not hold the equal-volume identity: N64 = {n64}, N128 = {n128}, \
             factor = {factor}"
        ));
    }

    let optimistic = project(&measured, source, target, n128, ProjectionMode::Optimistic);
    let pessimistic = project(&measured, source, target, n128, ProjectionMode::Pessimistic);

    let profile = residency.config.profile.name().to_owned();
    let fixture = memory.fixture.clone();
    let limitation = fixture.limitation.clone();

    let notes = vec![
        "The model is analytic, not a 128³ measurement. Each p95 component is projected \
         independently and the projected components are summed; the sum of p95s is not itself a \
         p95 of sums."
            .to_owned(),
        format!(
            "Fixed = upload + fence + flip. The fence and flip are per batch and unchanged. Upload \
             follows the COW path, which grows from {} to {} levels ({}), not a whole-chunk \
             transfer. Both projections share the same fixed term.",
            source.levels,
            target.levels,
            format!(
                "{:.3}",
                Ratio::new(u64::from(target.levels), u64::from(source.levels)).as_f64()
            ),
        ),
        format!(
            "Optimistic dirty = one COW path ({} scale) and the measured {} light cells, because a \
             block is still 16³ voxels at the larger edge. The mesh and the affected probe set grow \
             with surface area (×{}).",
            format!(
                "{:.3}",
                Ratio::new(u64::from(target.levels), u64::from(source.levels)).as_f64()
            ),
            grouped(measured.edit.light_cells_per_edit),
            Ratio::new(
                u64::from(target.chunk_dim) * u64::from(target.chunk_dim),
                u64::from(source.chunk_dim) * u64::from(source.chunk_dim),
            )
            .apply(1),
        ),
        format!(
            "Pessimistic dirty spans the chunk: COW grows with volume (×{}), light evaluates all {} \
             target block cells ({}× the measured {}), the probe update rebakes the whole {} probe \
             grid ({} rays), and every memory component grows with volume.",
            Ratio::new(target.chunk_volume, source.chunk_volume).apply(1),
            grouped(target.light_cells),
            format!(
                "{:.2}",
                target.light_cells as f64 / measured.edit.light_cells_per_edit as f64
            ),
            grouped(measured.edit.light_cells_per_edit),
            grouped(target.probes),
            grouped(target.probes * measured.probes.rays_per_probe),
        ),
        format!(
            "Memory is compared at equal loaded volume: N64 = {} and N128 = {} (factor {}). The \
             optimistic projection scales the mesh by area and the other components by volume; the \
             pessimistic projection scales every component by volume.",
            grouped(n64),
            grouped(n128),
            grouped(factor),
        ),
        "Fed the measured 64³ inputs and asked for 64³, the model returns those inputs unchanged; \
         that identity is asserted by the unit tests. The projection starts only at a larger edge."
            .to_owned(),
        format!(
            "Fixture limitation, verbatim: {limitation} Re-run this projection after the real save \
             before M1-42 quotes it."
        ),
    ];

    Ok(ScaleupReport {
        schema_version: SCHEMA_VERSION,
        fixture,
        source,
        target,
        profile,
        n64,
        n128,
        equal_volume_factor: factor,
        measured,
        optimistic,
        pessimistic,
        notes,
    })
}

/// Projects measured terms from `source` to `target`.
///
/// When the two edges match, this returns the measured terms exactly for both
/// modes: there is no extrapolation to do. When the target is larger, the
/// fixed term grows only with path depth and the dirty/variable term follows
/// the mode.
pub fn project(
    measured: &Measured,
    source: Edge,
    target: Edge,
    target_chunks: u64,
    mode: ProjectionMode,
) -> Projection {
    debug_assert!(
        target.chunk_bits >= source.chunk_bits,
        "the scale-up model projects upward, not downward"
    );
    if target.chunk_bits == source.chunk_bits {
        return measured_projection(measured, target, target_chunks, mode);
    }

    let path = Ratio::new(u64::from(target.levels), u64::from(source.levels));
    let volume = Ratio::new(target.chunk_volume, source.chunk_volume);
    let area = Ratio::new(
        u64::from(target.chunk_dim) * u64::from(target.chunk_dim),
        u64::from(source.chunk_dim) * u64::from(source.chunk_dim),
    );

    let upload_ns = path.apply(measured.edit.upload_ns);
    let fence_ns = measured.edit.fence_ns;
    let flip_ns = measured.edit.flip_ns;
    let fixed_ns = upload_ns + fence_ns + flip_ns;

    let (cow_ns, light_ns, light_cells) = match mode {
        ProjectionMode::Optimistic => (
            path.apply(measured.edit.cow_ns),
            measured.edit.light_ns,
            measured.edit.light_cells_per_edit,
        ),
        ProjectionMode::Pessimistic => {
            let light_ratio = Ratio::new(
                target.light_cells,
                measured.edit.light_cells_per_edit.max(1),
            );
            (
                volume.apply(measured.edit.cow_ns),
                light_ratio.apply(measured.edit.light_ns),
                target.light_cells,
            )
        }
    };
    let dirty_ns = cow_ns + light_ns;
    let total_ns = fixed_ns + dirty_ns;
    let edit_margin = i64::try_from(EDIT_BUDGET_NS).unwrap_or(i64::MAX)
        - i64::try_from(total_ns).unwrap_or(i64::MAX);
    let edit = EditProjection {
        upload_ns,
        fence_ns,
        flip_ns,
        fixed_ns,
        cow_ns,
        light_ns,
        dirty_ns,
        total_ns,
        light_cells,
        budget_ns: EDIT_BUDGET_NS,
        margin_ns: edit_margin,
        within_budget: edit_margin >= 0,
    };

    let probe_ratio = match mode {
        ProjectionMode::Optimistic => area,
        ProjectionMode::Pessimistic => Ratio::new(
            target.probes,
            measured.probes.affected_probes_per_edit.max(1),
        ),
    };
    let select_ns = probe_ratio.apply(measured.probes.select_ns);
    let rebake_ns = probe_ratio.apply(measured.probes.rebake_ns);
    let probe_upload_ns = probe_ratio.apply(measured.probes.upload_ns);
    let update_ns = select_ns + rebake_ns + probe_upload_ns;
    let (affected_probes, rays_baked) = match mode {
        ProjectionMode::Optimistic => (
            probe_ratio.apply(measured.probes.affected_probes_per_edit),
            probe_ratio.apply(measured.probes.rays_baked_per_edit),
        ),
        ProjectionMode::Pessimistic => (
            target.probes,
            target.probes * measured.probes.rays_per_probe,
        ),
    };
    let probe_margin = i64::try_from(PROBE_BUDGET_NS).unwrap_or(i64::MAX)
        - i64::try_from(update_ns).unwrap_or(i64::MAX);
    let probes = ProbeProjection {
        select_ns,
        rebake_ns,
        upload_ns: probe_upload_ns,
        update_ns,
        affected_probes,
        rays_baked,
        budget_ns: PROBE_BUDGET_NS,
        margin_ns: probe_margin,
        within_budget: probe_margin >= 0,
    };

    let (dag_ratio, overlay_ratio, light_ratio, probe_bytes_ratio, mesh_ratio) = match mode {
        ProjectionMode::Optimistic => (volume, volume, volume, volume, area),
        ProjectionMode::Pessimistic => (volume, volume, volume, volume, volume),
    };
    let dag_bytes_per_chunk = dag_ratio.apply(measured.memory.dag_bytes);
    let overlay_bytes_per_chunk = overlay_ratio.apply(measured.memory.overlay_bytes);
    let light_bytes_per_chunk = light_ratio.apply(measured.memory.light_bytes);
    let probe_bytes_per_chunk = probe_bytes_ratio.apply(measured.memory.probe_bytes);
    let mesh_bytes_per_chunk = mesh_ratio.apply(measured.memory.mesh_bytes);
    let total_bytes_per_chunk = dag_bytes_per_chunk
        + overlay_bytes_per_chunk
        + light_bytes_per_chunk
        + probe_bytes_per_chunk
        + mesh_bytes_per_chunk;
    let projected_bytes = total_bytes_per_chunk.saturating_mul(target_chunks);
    let target_bytes = VOXEL_TARGET_MIB * MIB;
    let fail_bytes = VOXEL_FAIL_MIB * MIB;
    let dag_nodes_per_chunk = volume.apply(measured.memory.dag_nodes);
    let memory = MemoryProjection {
        dag_bytes_per_chunk,
        overlay_bytes_per_chunk,
        light_bytes_per_chunk,
        probe_bytes_per_chunk,
        mesh_bytes_per_chunk,
        total_bytes_per_chunk,
        dag_nodes_per_chunk,
        loaded_chunks: target_chunks,
        projected_bytes,
        target_bytes,
        fail_bytes,
        within_target: projected_bytes <= target_bytes,
        within_fail: projected_bytes <= fail_bytes,
        node_budget: target.node_budget,
        node_within_budget: dag_nodes_per_chunk <= target.node_budget,
    };

    Projection {
        mode,
        same_edge: false,
        edit,
        probes,
        memory,
    }
}

/// The same-edge case: the projection is the measurement, not an extrapolation.
fn measured_projection(
    measured: &Measured,
    target: Edge,
    target_chunks: u64,
    mode: ProjectionMode,
) -> Projection {
    let edit = EditProjection {
        upload_ns: measured.edit.upload_ns,
        fence_ns: measured.edit.fence_ns,
        flip_ns: measured.edit.flip_ns,
        fixed_ns: measured.edit.fixed_ns,
        cow_ns: measured.edit.cow_ns,
        light_ns: measured.edit.light_ns,
        dirty_ns: measured.edit.dirty_ns,
        total_ns: measured.edit.total_ns,
        light_cells: measured.edit.light_cells_per_edit,
        budget_ns: EDIT_BUDGET_NS,
        margin_ns: i64::try_from(EDIT_BUDGET_NS).unwrap_or(i64::MAX)
            - i64::try_from(measured.edit.total_ns).unwrap_or(i64::MAX),
        within_budget: measured.edit.total_ns <= EDIT_BUDGET_NS,
    };
    let probes = ProbeProjection {
        select_ns: measured.probes.select_ns,
        rebake_ns: measured.probes.rebake_ns,
        upload_ns: measured.probes.upload_ns,
        update_ns: measured.probes.update_ns,
        affected_probes: measured.probes.affected_probes_per_edit,
        rays_baked: measured.probes.rays_baked_per_edit,
        budget_ns: PROBE_BUDGET_NS,
        margin_ns: i64::try_from(PROBE_BUDGET_NS).unwrap_or(i64::MAX)
            - i64::try_from(measured.probes.update_ns).unwrap_or(i64::MAX),
        within_budget: measured.probes.update_ns <= PROBE_BUDGET_NS,
    };
    let projected_bytes = measured.memory.total_bytes.saturating_mul(target_chunks);
    let target_bytes = VOXEL_TARGET_MIB * MIB;
    let fail_bytes = VOXEL_FAIL_MIB * MIB;
    let memory = MemoryProjection {
        dag_bytes_per_chunk: measured.memory.dag_bytes,
        overlay_bytes_per_chunk: measured.memory.overlay_bytes,
        light_bytes_per_chunk: measured.memory.light_bytes,
        probe_bytes_per_chunk: measured.memory.probe_bytes,
        mesh_bytes_per_chunk: measured.memory.mesh_bytes,
        total_bytes_per_chunk: measured.memory.total_bytes,
        dag_nodes_per_chunk: measured.memory.dag_nodes,
        loaded_chunks: target_chunks,
        projected_bytes,
        target_bytes,
        fail_bytes,
        within_target: projected_bytes <= target_bytes,
        within_fail: projected_bytes <= fail_bytes,
        node_budget: target.node_budget,
        node_within_budget: measured.memory.dag_nodes <= target.node_budget,
    };
    Projection {
        mode,
        same_edge: true,
        edit,
        probes,
        memory,
    }
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::constants::BASELINE_CHUNK_BITS;

    fn sample_measured() -> Measured {
        Measured {
            edit: MeasuredEdit {
                upload_ns: 700,
                fence_ns: 100,
                flip_ns: 50,
                fixed_ns: 850,
                cow_ns: 900,
                light_ns: 300,
                dirty_ns: 1_200,
                total_ns: 2_050,
                upload_bytes_per_edit: 360,
                cow_bytes_per_edit: 344,
                light_cells_per_edit: 27,
            },
            probes: MeasuredProbes {
                select_ns: 200,
                rebake_ns: 800,
                upload_ns: 40,
                update_ns: 1_040,
                affected_probes_per_edit: 8,
                rays_tested_per_edit: 512,
                rays_baked_per_edit: 512,
                probes_per_chunk: 64,
                rays_per_probe: 64,
            },
            memory: MeasuredMemory {
                dag_bytes: 1_000,
                overlay_bytes: 100,
                light_bytes: 1_024,
                probe_bytes: 768,
                mesh_bytes: 100_000,
                total_bytes: 102_892,
                dag_nodes: 10_000,
            },
        }
    }

    #[test]
    fn ratios_scale_by_edge_volume_area_and_levels() {
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        assert_eq!(source.chunk_dim, 64);
        assert_eq!(target.chunk_dim, 128);
        assert_eq!(source.chunk_volume, 64 * 64 * 64);
        assert_eq!(target.chunk_volume, 128 * 128 * 128);
        assert_eq!(source.levels, 7);
        assert_eq!(target.levels, 8);
        assert_eq!(
            Ratio::new(target.chunk_volume, source.chunk_volume).apply(1),
            8
        );
        assert_eq!(
            Ratio::new(
                u64::from(target.chunk_dim) * u64::from(target.chunk_dim),
                u64::from(source.chunk_dim) * u64::from(source.chunk_dim),
            )
            .apply(1),
            4
        );
        assert_eq!(Ratio::new(8, 7).apply(700), 800);
        assert_eq!(Ratio::new(512, 27).apply(300), 5_689);
    }

    #[test]
    fn same_edge_reproduces_the_measured_terms_exactly() {
        let measured = sample_measured();
        let edge = Edge::at(BASELINE_CHUNK_BITS);
        for mode in [ProjectionMode::Optimistic, ProjectionMode::Pessimistic] {
            let projection = project(&measured, edge, edge, 25_736, mode);
            assert!(projection.same_edge);
            assert_eq!(projection.edit.upload_ns, measured.edit.upload_ns);
            assert_eq!(projection.edit.fixed_ns, measured.edit.fixed_ns);
            assert_eq!(projection.edit.cow_ns, measured.edit.cow_ns);
            assert_eq!(projection.edit.light_ns, measured.edit.light_ns);
            assert_eq!(projection.edit.dirty_ns, measured.edit.dirty_ns);
            assert_eq!(projection.edit.total_ns, measured.edit.total_ns);
            assert_eq!(
                projection.edit.light_cells,
                measured.edit.light_cells_per_edit
            );
            assert_eq!(projection.probes.update_ns, measured.probes.update_ns);
            assert_eq!(
                projection.probes.affected_probes,
                measured.probes.affected_probes_per_edit
            );
            assert_eq!(
                projection.memory.total_bytes_per_chunk,
                measured.memory.total_bytes
            );
            assert_eq!(
                projection.memory.projected_bytes,
                measured.memory.total_bytes * 25_736
            );
            assert_eq!(
                projection.memory.dag_nodes_per_chunk,
                measured.memory.dag_nodes
            );
        }
    }

    #[test]
    fn the_fixed_term_is_shared_and_only_the_path_depth_scales() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let optimistic = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);
        let pessimistic = project(
            &measured,
            source,
            target,
            3_217,
            ProjectionMode::Pessimistic,
        );

        assert_eq!(optimistic.edit.fixed_ns, pessimistic.edit.fixed_ns);
        assert_eq!(optimistic.edit.upload_ns, 800);
        assert_eq!(optimistic.edit.fence_ns, 100);
        assert_eq!(optimistic.edit.flip_ns, 50);
        assert_eq!(optimistic.edit.fixed_ns, 950);
        assert_eq!(
            optimistic.edit.fixed_ns,
            optimistic.edit.upload_ns + optimistic.edit.fence_ns + optimistic.edit.flip_ns
        );
    }

    #[test]
    fn optimistic_dirty_is_one_path_and_one_block_cell() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let projection = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);

        assert_eq!(projection.edit.cow_ns, 1_029, "900 × 8/7 rounded");
        assert_eq!(projection.edit.light_ns, 300);
        assert_eq!(projection.edit.light_cells, 27);
        assert_eq!(projection.edit.dirty_ns, 1_329);
        assert_eq!(projection.edit.total_ns, 2_279);
        assert!(projection.edit.within_budget);
    }

    #[test]
    fn pessimistic_dirty_spans_the_chunk() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let projection = project(
            &measured,
            source,
            target,
            3_217,
            ProjectionMode::Pessimistic,
        );

        assert_eq!(projection.edit.cow_ns, 7_200, "900 × volume 8");
        assert_eq!(projection.edit.light_cells, 512);
        assert_eq!(projection.edit.light_ns, 5_689, "300 × 512/27 rounded");
        assert_eq!(projection.edit.dirty_ns, 12_889);
        assert_eq!(projection.edit.total_ns, 13_839);
        assert!(
            projection.edit.total_ns > 0,
            "the pessimistic projection is still a finite number"
        );
    }

    #[test]
    fn probe_optimistic_uses_area_and_pessimistic_uses_the_full_grid() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let optimistic = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);
        let pessimistic = project(
            &measured,
            source,
            target,
            3_217,
            ProjectionMode::Pessimistic,
        );

        assert_eq!(optimistic.probes.select_ns, 800);
        assert_eq!(optimistic.probes.rebake_ns, 3_200);
        assert_eq!(optimistic.probes.upload_ns, 160);
        assert_eq!(optimistic.probes.update_ns, 4_160);
        assert_eq!(optimistic.probes.affected_probes, 32);
        assert_eq!(optimistic.probes.rays_baked, 2_048);

        assert_eq!(pessimistic.probes.affected_probes, 512);
        assert_eq!(pessimistic.probes.rays_baked, 512 * 64);
        assert_eq!(pessimistic.probes.select_ns, 200 * 64);
        assert_eq!(pessimistic.probes.rebake_ns, 800 * 64);
        assert_eq!(pessimistic.probes.upload_ns, 40 * 64);
        assert_eq!(pessimistic.probes.update_ns, 1_040 * 64);
        assert!(pessimistic.probes.update_ns > optimistic.probes.update_ns);
    }

    #[test]
    fn memory_optimistic_scales_the_mesh_by_area_and_pessimistic_by_volume() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let optimistic = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);
        let pessimistic = project(
            &measured,
            source,
            target,
            3_217,
            ProjectionMode::Pessimistic,
        );

        assert_eq!(optimistic.memory.dag_bytes_per_chunk, 8_000);
        assert_eq!(optimistic.memory.overlay_bytes_per_chunk, 800);
        assert_eq!(optimistic.memory.light_bytes_per_chunk, 8_192);
        assert_eq!(optimistic.memory.probe_bytes_per_chunk, 6_144);
        assert_eq!(optimistic.memory.mesh_bytes_per_chunk, 400_000);
        assert_eq!(optimistic.memory.total_bytes_per_chunk, 423_136);
        assert_eq!(optimistic.memory.projected_bytes, 423_136 * 3_217);

        assert_eq!(pessimistic.memory.mesh_bytes_per_chunk, 800_000);
        assert_eq!(pessimistic.memory.total_bytes_per_chunk, 823_136);
        assert_eq!(pessimistic.memory.projected_bytes, 823_136 * 3_217);
        assert!(
            pessimistic.memory.projected_bytes > optimistic.memory.projected_bytes,
            "the dirty-span projection cannot be cheaper than the local one"
        );
        assert_eq!(optimistic.memory.dag_nodes_per_chunk, 80_000);
        assert!(optimistic.memory.node_within_budget);
    }

    #[test]
    fn the_pessimistic_edit_is_never_cheaper_than_the_optimistic_one() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let optimistic = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);
        let pessimistic = project(
            &measured,
            source,
            target,
            3_217,
            ProjectionMode::Pessimistic,
        );

        assert!(pessimistic.edit.dirty_ns > optimistic.edit.dirty_ns);
        assert!(pessimistic.edit.total_ns > optimistic.edit.total_ns);
        assert!(pessimistic.edit.light_cells > optimistic.edit.light_cells);
        assert!(pessimistic.probes.update_ns >= optimistic.probes.update_ns);
        assert!(pessimistic.memory.projected_bytes >= optimistic.memory.projected_bytes);
    }

    #[test]
    fn a_budget_margin_is_computed_from_the_projected_total() {
        let measured = sample_measured();
        let source = Edge::at(BASELINE_CHUNK_BITS);
        let target = Edge::at(TARGET_CHUNK_BITS);
        let projection = project(&measured, source, target, 3_217, ProjectionMode::Optimistic);
        assert_eq!(
            projection.edit.margin_ns,
            EDIT_BUDGET_NS as i64 - projection.edit.total_ns as i64
        );
        assert_eq!(
            projection.probes.margin_ns,
            PROBE_BUDGET_NS as i64 - projection.probes.update_ns as i64
        );
        assert_eq!(
            projection.edit.within_budget,
            projection.edit.margin_ns >= 0
        );
        assert_eq!(
            projection.probes.within_budget,
            projection.probes.margin_ns >= 0
        );
    }
}
