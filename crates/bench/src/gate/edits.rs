//! Per-edit latency on the frozen fixture (M1-37).
//!
//! The pre-registered edit budget is `p95 <= 8 ms` for
//! consolidate + upload + fence + flip + dirty-region light. One number cannot
//! say where that time goes, and the 128³ projection (M1-40) needs the split, so
//! this module reports every edit as two terms:
//!
//! | Term | Components | What is measured |
//! | --- | --- | --- |
//! | **Fixed** | upload, fence, flip | Copy the appended runs and the `levelStart` snapshot to a staging buffer, wait for the batch fence, flip the chunk's version index |
//! | **Dirty region** | COW, light | Append the edited paths in the banded layout, relight the edited block cell plus its one-cell apron |
//!
//! `fixed + dirty` is the per-edit total. Each component is timed separately and
//! the aggregate terms are the per-edit sums, so the sub-timers' overhead is
//! charged to the aggregate rather than hidden.
//!
//! M2-24 adds a second, production **scrub** path over the same edits: the
//! per-block dirty bitset (M2-22), one scheduler batch (M2-23), the production
//! greedy remesh, incremental light (M2-15) and the production probe update
//! (M2-21), compared with the 4 ms scrub budget. The M1-37 edit terms are kept
//! unchanged, so the historical 8 ms measurement stays comparable.
//!
//! # What is real and what is a model
//!
//! The gate runs headless: there is no wgpu adapter and no GPU fence. Upload
//! copies the **actual appended run bytes**, leaf materials and the seven-word
//! `levelStart` snapshot; no padding words are added, and no whole-chunk
//! transfer is charged. The fence is the CPU side of the M4 protocol — submit a
//! token, publish the signal with a Release store, Acquire-load until it is
//! visible. It is a lower bound on a real GPU fence, which M4-05/M4-16 replace
//! with the device fence. The flip writes the chunk's version index only after
//! that wait, matching ADR-007.
//!
//! The dirty region is one block cell: a 64³ chunk has 4³ light cells, so a
//! single-voxel edit dirties one cell and the M1-29 reference fill is run over
//! that cell plus its one-cell apron (27 cells at either chunk edge). The
//! edited cell uses the material just written; the apron samples the current
//! fixture chunk. Sky exposure is a chunk-local heightmap, so a chunk directly
//! above this one is not consulted and the apron is treated as open sky. Those
//! approximations affect light *values*, not the measured cell count or the COW
//! cost, and M2 replaces this reference with the incremental pass.
//!
//! # Input
//!
//! Every edit targets a real non-air voxel in a live fixture chunk. Candidates
//! are spread through each chunk, and each edit alternates between two real
//! materials — another material already present in the chunk when it has one,
//! else stone/cobblestone — so the workload is never a synthetic pattern, never
//! carves a chunk to air, and never repeats a no-op write. The fixture is still
//! **provisional until the first real save**; the report carries that limitation
//! verbatim.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use voxel_block::{BlockFlags, BlockKind, BlockRegistryBuilder, MaterialId};
use voxel_core::constants::ROOT_LEVEL;
use voxel_core::content_constants::{LIGHT_CELLS_PER_CHUNK, LIGHT_CELLS_PER_EDGE};
use voxel_mesh::dirty::DirtyBlocks;
use voxel_mesh::light::{BlockLight as MeshBlockLight, CellEdit, LightState};
use voxel_mesh::probe::{ProbeGrid, ProbeLod, ProbeVolume, probes_reaching};
use voxel_mesh::{BlockScratch, RemeshScheduler, greedy_block};
use voxel_storage::bands::BAND_COUNT;
use voxel_storage::reference::light::{BlockLight, LightFill, full_pass};
use voxel_storage::scan::scan_save;
use voxel_storage::serialize;
use voxel_storage::{
    AIR, Apron, BLOCK_DIM, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME, ChunkDag, ChunkPos, Face, Node,
    local_index, local_xyz,
};

use crate::fixture::{self, FixtureStatus, Manifest};

use super::memory::FixtureInfo;
pub use super::stat::{BudgetCheck, Term};
use super::stat::{format_duration, format_signed_duration, grouped, mean, term};

/// Schema version of the edits report itself.
///
/// Version 3 adds the M2-24 production scrub terms.
pub const SCHEMA_VERSION: u32 = 3;

/// Measured edits. The task fixes this count; the report is not a microbenchmark
/// that silently changes its sample size.
pub const EDITS: usize = 1_000;

/// Untimed edits before the measured run, so first-touch page faults and cold
/// code paths do not become the reported p95/max.
pub const WARMUP_EDITS: usize = 100;

/// Pre-registered edit-latency budget: p95 ≤ 8 ms (`pre-registration.md` §3).
pub const BUDGET_NS: u64 = 8_000_000;

/// M2-24 scrub budget: single edit → remesh + light + probe p95 ≤ 4 ms
/// (`voxel-engine-roadmap.md` M2 exit).
pub const SCRUB_BUDGET_NS: u64 = 4_000_000;

/// Candidate voxels sampled per fixture chunk.
const CANDIDATES_PER_CHUNK: usize = 16;

/// The four timed components and the two aggregate terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditTerms {
    /// Appended runs, leaves and `levelStart` copied to the staging buffer.
    pub upload: Term,
    /// Submit + signal + wait for the batch fence.
    pub fence: Term,
    /// Version-index flip after the fence.
    pub flip: Term,
    /// `upload + fence + flip`.
    pub fixed: Term,
    /// Banded `ChunkDag::apply_edits` for the edited paths.
    pub cow: Term,
    /// M1-29 reference fill over the dirty cell and its apron.
    pub light: Term,
    /// `cow + light`.
    pub dirty: Term,
    /// `fixed + dirty`.
    pub total: Term,
}

/// One measured edit, including its two terms and the bytes/cells that drove
/// them. The per-edit rows are what makes "both terms per edit" auditable; the
/// Markdown report prints the distributions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditSample {
    /// Zero-based index in the measured run.
    pub edit: u32,
    /// Fixture chunk that received the edit.
    pub chunk: [i32; 3],
    /// Chunk-local voxel index.
    pub voxel: u32,
    /// Material written by the edit.
    pub material: u16,
    /// Material the voxel held immediately before this edit.
    pub previous_material: u16,
    /// Internal sibling runs appended by the COW edit.
    pub cow_runs: u64,
    /// Internal node words appended by the COW edit.
    pub cow_nodes: u64,
    /// Total COW bytes appended: nodes, alignment padding and leaf materials.
    pub cow_bytes: u64,
    /// Leaf runs appended by the COW edit.
    pub cow_leaf_runs: u64,
    /// Internal run bytes uploaded, padding included because it is real.
    pub upload_node_bytes: u64,
    /// Leaf-material bytes uploaded.
    pub upload_leaf_bytes: u64,
    /// `levelStart` bytes uploaded once per version.
    pub upload_level_start_bytes: u64,
    /// `upload_node_bytes + upload_leaf_bytes + upload_level_start_bytes`.
    pub upload_bytes: u64,
    /// Light cells evaluated by the reference fill (dirty cell + apron).
    pub light_cells: u64,
    /// Version index written after the fence (0 or 1).
    pub version: u32,
    /// COW component latency.
    pub cow_ns: u64,
    /// Upload component latency.
    pub upload_ns: u64,
    /// Fence component latency.
    pub fence_ns: u64,
    /// Flip component latency.
    pub flip_ns: u64,
    /// Light component latency.
    pub light_ns: u64,
    /// `upload_ns + fence_ns + flip_ns`.
    pub fixed_ns: u64,
    /// Periodic compaction charged to this edit (0 for 63 of 64 edits).
    pub compact_ns: u64,
    /// `cow_ns + light_ns`.
    pub dirty_ns: u64,
    /// `fixed_ns + dirty_ns + compact_ns`.
    pub total_ns: u64,
    /// Blocks in the scrub batch (M2-22 dirty set drained by M2-23).
    pub dirty_blocks: u32,
    /// Quads the production greedy mesher produced for the batch.
    pub meshed_quads: u32,
    /// Probes rebaked for this edit (M2-21 selection).
    pub affected_probes: u32,
    /// Dirty bitset + scheduler batch.
    pub scrub_dirty_ns: u64,
    /// Production greedy remesh of the batch.
    pub remesh_ns: u64,
    /// Production incremental light (M2-15).
    pub scrub_light_ns: u64,
    /// Production probe selection + rebake (M2-21).
    pub probe_ns: u64,
    /// `compact_ns + cow_ns + scrub_dirty_ns + remesh_ns + scrub_light_ns +
    /// probe_ns`.
    pub scrub_total_ns: u64,
}

/// The M2-24 scrub terms: what a single edit costs after the COW write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubTerms {
    /// Dirty bits (M2-22) plus the scheduler batch (M2-23).
    pub dirty: Term,
    /// Production greedy remesh of the dirty blocks.
    pub remesh: Term,
    /// Production incremental light (M2-15).
    pub light: Term,
    /// Production probe selection and rebake (M2-21).
    pub probe: Term,
    /// `compaction + cow + dirty + remesh + light + probe`.
    pub total: Term,
}

/// Mean scrub work per edited chunk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubCounts {
    /// Blocks in the scrub batch.
    pub dirty_blocks_per_edit: u64,
    /// Quads produced by the production mesher.
    pub quads_per_edit: u64,
    /// Probes rebaked per edit.
    pub affected_probes_per_edit: u64,
}

/// Byte and light-cell totals behind the timing rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditBytes {
    /// Sum of `EditSample::upload_bytes`.
    pub upload_bytes: u64,
    /// Sum of `EditSample::upload_node_bytes`.
    pub upload_node_bytes: u64,
    /// Sum of `EditSample::upload_leaf_bytes`.
    pub upload_leaf_bytes: u64,
    /// Sum of `EditSample::upload_level_start_bytes`.
    pub upload_level_start_bytes: u64,
    /// Sum of `EditSample::cow_bytes`.
    pub cow_bytes: u64,
    /// Mean uploaded bytes per edit.
    pub upload_bytes_per_edit: u64,
    /// Mean COW bytes per edit.
    pub cow_bytes_per_edit: u64,
    /// Light cells evaluated per edit; the dirty-region fill size.
    pub light_cells_per_edit: u64,
}

/// The complete M1-37 edit report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditsReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Fixture identity and provenance.
    pub fixture: FixtureInfo,
    /// Measured edits.
    pub edit_count: usize,
    /// Untimed edits before the measured run.
    pub warmup_edits: usize,
    /// Fixture chunks that received a measured edit.
    pub chunks_edited: usize,
    /// Fixture chunks available as edit targets.
    pub chunks_available: usize,
    /// Component and term distributions.
    pub terms: EditTerms,
    /// Byte and light-cell totals.
    pub bytes: EditBytes,
    /// Budget comparison for the pre-registered 8 ms edit budget.
    pub budget: BudgetCheck,
    /// M2-24 scrub component and aggregate terms.
    pub scrub_terms: ScrubTerms,
    /// Mean scrub work per edit.
    pub scrub_counts: ScrubCounts,
    /// Budget comparison for the 4 ms scrub budget.
    pub scrub_budget: BudgetCheck,
    /// Every measured edit, in run order.
    pub edits: Vec<EditSample>,
    /// Definitions and caveats a reader needs to interpret the table.
    pub notes: Vec<String>,
}

impl EditsReport {
    /// True while the measured fixture is the synthetic M1 placeholder.
    pub fn is_provisional(&self) -> bool {
        self.fixture.provisional_until_first_real_save
    }

    /// Renders the report as a Markdown table plus provenance and notes.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate edits (M1-37)\n").unwrap();
        writeln!(
            out,
            "**Fixture:** `{}` ({})\n",
            self.fixture.id, self.fixture.status
        )
        .unwrap();
        writeln!(out, "**Manifest:** `{}`\n", self.fixture.manifest).unwrap();
        writeln!(out, "**Save:** `{}`\n", self.fixture.save_dir).unwrap();
        writeln!(
            out,
            "**Format:** {}³ ({} bits), {} chunks, {} non-air voxels\n",
            self.fixture.chunk_dim,
            self.fixture.chunk_bits,
            grouped(self.fixture.chunks as u64),
            grouped(self.fixture.total_voxels),
        )
        .unwrap();
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
            "**Edits:** {} measured + {} warm-up, across {} of {} populated fixture chunks \
             ({} live)\n",
            grouped(self.edit_count as u64),
            grouped(self.warmup_edits as u64),
            grouped(self.chunks_edited as u64),
            grouped(self.chunks_available as u64),
            grouped(self.fixture.chunks as u64),
        )
        .unwrap();
        writeln!(
            out,
            "**Budget:** p95 ≤ {}; measured p95 {} → {} ({} margin)\n",
            format_duration(self.budget.budget_ns),
            format_duration(self.budget.measured_p95_ns),
            if self.budget.within_budget {
                "**PASS**"
            } else {
                "**MISS**"
            },
            format_signed_duration(self.budget.margin_ns),
        )
        .unwrap();

        writeln!(out, "| Term | Component | p50 | p95 | max | total |").unwrap();
        writeln!(out, "|---|---|---:|---:|---:|---:|").unwrap();
        for (term, component, value) in [
            ("Fixed", "Upload", &self.terms.upload),
            ("Fixed", "Fence", &self.terms.fence),
            ("Fixed", "Flip", &self.terms.flip),
            ("Fixed", "**Upload + fence + flip**", &self.terms.fixed),
            ("Dirty", "COW", &self.terms.cow),
            ("Dirty", "Light", &self.terms.light),
            ("Dirty", "**COW + light**", &self.terms.dirty),
            ("Edit", "**Fixed + dirty**", &self.terms.total),
        ] {
            writeln!(
                out,
                "| {term} | {component} | {} | {} | {} | {} |",
                format_duration(value.p50_ns),
                format_duration(value.p95_ns),
                format_duration(value.max_ns),
                format_duration(value.total_ns),
            )
            .unwrap();
        }

        writeln!(
            out,
            "\n**Scrub budget (M2-24):** p95 ≤ {}; measured p95 {} → {} ({} margin)\n",
            format_duration(self.scrub_budget.budget_ns),
            format_duration(self.scrub_budget.measured_p95_ns),
            if self.scrub_budget.within_budget {
                "**PASS**"
            } else {
                "**MISS**"
            },
            format_signed_duration(self.scrub_budget.margin_ns),
        )
        .unwrap();
        writeln!(out, "| Scrub | Component | p50 | p95 | max | total |").unwrap();
        writeln!(out, "|---|---|---:|---:|---:|---:|").unwrap();
        for (component, value) in [
            ("Dirty blocks", &self.scrub_terms.dirty),
            ("Remesh", &self.scrub_terms.remesh),
            ("Light", &self.scrub_terms.light),
            ("Probe", &self.scrub_terms.probe),
            (
                "**Compaction + edit + dirty + remesh + light + probe**",
                &self.scrub_terms.total,
            ),
        ] {
            writeln!(
                out,
                "| Scrub | {component} | {} | {} | {} | {} |",
                format_duration(value.p50_ns),
                format_duration(value.p95_ns),
                format_duration(value.max_ns),
                format_duration(value.total_ns),
            )
            .unwrap();
        }
        writeln!(
            out,
            "\n**Scrub work per edit:** dirty blocks {}, quads {}, probes {}\n",
            self.scrub_counts.dirty_blocks_per_edit,
            self.scrub_counts.quads_per_edit,
            self.scrub_counts.affected_probes_per_edit,
        )
        .unwrap();

        writeln!(
            out,
            "\n**Upload:** {} B/edit ({} nodes, {} leaves, {} `levelStart`); {} total\n",
            self.bytes.upload_bytes_per_edit,
            self.bytes.upload_node_bytes / self.edit_count.max(1) as u64,
            self.bytes.upload_leaf_bytes / self.edit_count.max(1) as u64,
            self.bytes.upload_level_start_bytes / self.edit_count.max(1) as u64,
            self.bytes.upload_bytes,
        )
        .unwrap();
        writeln!(
            out,
            "**COW:** {} B/edit; {} total\n",
            self.bytes.cow_bytes_per_edit, self.bytes.cow_bytes,
        )
        .unwrap();
        writeln!(
            out,
            "**Light:** {} cells/edit (one dirty block cell plus its 1-cell apron)\n",
            self.bytes.light_cells_per_edit,
        )
        .unwrap();

        for note in &self.notes {
            writeln!(out, "\n- {note}").unwrap();
        }
        out
    }
}

/// Measures the default fixture manifest.
pub fn default_report() -> Result<EditsReport, String> {
    report(&fixture::default_manifest())
}

/// Measures the fixture named by `manifest_path`.
///
/// The manifest is checked for schema, chunk edge and chunk version before the
/// save is read. The scan must match the manifest's live chunk, voxel, overlay
/// and region counts; a save edited after freezing is an error, not a silently
/// different report.
pub fn report(manifest_path: &Path) -> Result<EditsReport, String> {
    let manifest = fixture::read_manifest(manifest_path)?;
    if manifest.schema_version != fixture::SCHEMA_VERSION {
        return Err(format!(
            "manifest schema {} is not supported (this build is {})",
            manifest.schema_version,
            fixture::SCHEMA_VERSION
        ));
    }
    manifest.recipe.validate()?;
    if manifest.format.chunk_bits != CHUNK_BITS || manifest.format.chunk_dim != CHUNK_DIM as u32 {
        return Err(format!(
            "fixture was frozen at {}-bit chunks ({}³); this build uses {}-bit chunks ({}³). \
             The 64³ fixture is intentionally not valid at another edge; re-freeze at the matching edge.",
            manifest.format.chunk_bits, manifest.format.chunk_dim, CHUNK_BITS, CHUNK_DIM
        ));
    }
    if manifest.format.chunk_version != serialize::VERSION {
        return Err(format!(
            "fixture chunk version {} is not supported (this build is {})",
            manifest.format.chunk_version,
            serialize::VERSION
        ));
    }

    let save_dir = fixture::save_dir(manifest_path, &manifest)?;
    measure(manifest_path, &save_dir, manifest)
}

/// One edit target inside a fixture chunk.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    index: u32,
    /// Material the voxel held when the fixture was scanned.
    first: u16,
    /// A different material present in the same chunk, when one exists.
    second: u16,
}

/// A fixture chunk prepared for edits.
struct ChunkState {
    pos: ChunkPos,
    dag: ChunkDag,
    /// The same content as `dag`, kept dense for the scrub path.
    dense: Vec<u16>,
    /// One-voxel border from the six neighbours when they are resident.
    apron: Apron,
    candidates: Vec<Candidate>,
    /// Chunk-local sky exposure per light cell.
    sky: Vec<bool>,
    /// Production incremental light state (M2-15).
    light_state: LightState,
    /// Near-grid probe volume, updated per edit (M2-21).
    probe: ProbeVolume,
}

/// Band lengths before one edit, used to isolate the appended ranges.
#[derive(Clone, Copy, Debug)]
struct BandSnapshot {
    nodes: [u64; BAND_COUNT],
    leaves: u64,
}

impl BandSnapshot {
    fn capture(dag: &ChunkDag) -> Self {
        let mut nodes = [0u64; BAND_COUNT];
        for (level, slot) in nodes.iter_mut().enumerate() {
            *slot = dag.bands().len(level as u32);
        }
        Self {
            nodes,
            leaves: dag.bands().leaf_len(),
        }
    }
}

/// The bytes one edit appends to the staging buffer.
#[derive(Clone, Copy, Debug, Default)]
struct UploadBytes {
    node_bytes: u64,
    leaf_bytes: u64,
    level_start_bytes: u64,
}

impl UploadBytes {
    fn total(self) -> u64 {
        self.node_bytes + self.leaf_bytes + self.level_start_bytes
    }
}

/// Headless stand-in for the GPU upload buffer, fence and version table.
struct UploadDevice {
    /// Staging buffer for node words and `levelStart`.
    staging: Vec<Node>,
    /// Staging buffer for leaf materials.
    leaves: Vec<u16>,
    /// One version index per fixture chunk.
    versions: Vec<u32>,
    /// Batch fence model.
    fence: Fence,
}

impl UploadDevice {
    fn new(chunks: usize) -> Self {
        Self {
            // One edit appends at most a full run per level plus `levelStart`.
            // The capacity is scratch space, not measured or reported bytes.
            staging: Vec::with_capacity(4_096),
            leaves: Vec::with_capacity(64),
            versions: vec![0; chunks],
            fence: Fence::new(),
        }
    }

    /// Copies exactly the appended runs and leaf materials, then the
    /// seven-word `levelStart` snapshot. No padding words are added.
    fn upload(&mut self, dag: &ChunkDag, before: &BandSnapshot) -> UploadBytes {
        self.staging.clear();
        self.leaves.clear();

        for level in 0..=ROOT_LEVEL {
            let start = before.nodes[level as usize];
            let end = dag.bands().len(level);
            if end > start {
                self.staging
                    .extend_from_slice(&dag.bands().band(level)[start as usize..end as usize]);
            }
        }
        let node_bytes = self.staging.len() as u64 * core::mem::size_of::<Node>() as u64;

        for word in dag.bands().level_start() {
            self.staging.push(Node::decode(word));
        }
        let level_start_bytes = (BAND_COUNT * core::mem::size_of::<Node>()) as u64;

        let leaf_end = dag.bands().leaf_len();
        if leaf_end > before.leaves {
            self.leaves.extend_from_slice(
                &dag.bands().leaves()[before.leaves as usize..leaf_end as usize],
            );
        }
        let leaf_bytes = self.leaves.len() as u64 * core::mem::size_of::<u16>() as u64;

        UploadBytes {
            node_bytes,
            leaf_bytes,
            level_start_bytes,
        }
    }

    /// Writes the opposite version index for one chunk, after the fence.
    fn flip(&mut self, chunk: usize) -> u32 {
        let version = self.versions[chunk] ^ 1;
        self.versions[chunk] = version;
        version
    }
}

/// CPU-side model of the M4 batch fence.
///
/// The gate has no GPU, so this records the cost of the boundary itself: submit
/// a token, publish its completion, and poll with Acquire until the Release
/// store is visible. Because the signal happens immediately before the wait,
/// the measured fence term is the submit/signal/poll protocol cost, not GPU
/// completion latency; M4-05/M4-16 replace this model with a real fence.
struct Fence {
    signalled: AtomicU64,
    next_token: u64,
}

impl Fence {
    fn new() -> Self {
        Self {
            signalled: AtomicU64::new(0),
            next_token: 1,
        }
    }

    fn submit(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        token
    }

    fn signal(&self, token: u64) {
        self.signalled.store(token, Ordering::Release);
    }

    fn wait(&self, token: u64) {
        while self.signalled.load(Ordering::Acquire) < token {
            std::hint::spin_loop();
        }
    }
}

fn measure(
    manifest_path: &Path,
    save_dir: &Path,
    manifest: Manifest,
) -> Result<EditsReport, String> {
    let light_table = builtin_light_table();
    let mut dense = vec![0u16; CHUNK_VOLUME];
    // The scrub path needs dense neighbours to build each chunk's apron, so the
    // scan keeps every populated chunk until the states are assembled.
    let mut scanned: Vec<(ChunkPos, Vec<u16>)> =
        Vec::with_capacity(manifest.observed.live_chunks.min(4096));
    let mut total_voxels = 0u64;
    let mut overlay_entries = 0u64;

    let summary = scan_save(save_dir, |chunk| {
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }

        total_voxels += dense.iter().filter(|&&material| material != AIR).count() as u64;
        overlay_entries += chunk.overlay.len() as u64;
        scanned.push((chunk.pos, dense.clone()));
    })
    .map_err(|error| format!("failed to scan fixture {}: {error}", save_dir.display()))?;

    if !summary.warnings.is_empty() {
        return Err(format!(
            "fixture scan reported warnings; refusing to build a gate report from a damaged save: {:?}",
            summary.warnings
        ));
    }
    if summary.chunks != manifest.observed.live_chunks
        || total_voxels != manifest.observed.total_voxels
        || overlay_entries != manifest.observed.total_overrides
        || summary.region_files != manifest.observed.region_files
    {
        return Err(format!(
            "fixture observations differ from the manifest (chunks {} vs {}, voxels {} vs {}, \
             overlays {} vs {}, regions {} vs {}); run `voxel-bench fixture verify` or re-freeze",
            summary.chunks,
            manifest.observed.live_chunks,
            total_voxels,
            manifest.observed.total_voxels,
            overlay_entries,
            manifest.observed.total_overrides,
            summary.region_files,
            manifest.observed.region_files,
        ));
    }
    let index: HashMap<ChunkPos, usize> = scanned
        .iter()
        .enumerate()
        .map(|(slot, (pos, _))| (*pos, slot))
        .collect();

    // Build every apron before taking any dense buffer: a later chunk's apron
    // reads its neighbours, including chunks already turned into states.
    let mut aprons: Vec<Option<Apron>> = Vec::with_capacity(scanned.len());
    for (pos, dense) in &scanned {
        if candidates(dense).is_empty() {
            aprons.push(None);
        } else {
            aprons.push(Some(build_apron(*pos, &scanned, &index)));
        }
    }

    let mut states = Vec::with_capacity(scanned.len());
    for slot in 0..scanned.len() {
        let Some(apron) = aprons[slot].take() else {
            continue;
        };
        let pos = scanned[slot].0;
        let dense = std::mem::take(&mut scanned[slot].1);
        let candidates = candidates(&dense);
        let sky = sky_cells(&dense, &light_table);
        let light_state = build_light_state(&dense, &sky, &light_table);
        let probe =
            voxel_mesh::probe::bake(pos, ProbeGrid::for_chunk(ProbeLod::Near), |x, y, z| {
                dense[local_index(x, y, z) as usize] != AIR
            });
        states.push(ChunkState {
            pos,
            dag: ChunkDag::from_dense(&dense),
            dense,
            apron,
            candidates,
            sky,
            light_state,
            probe,
        });
    }
    drop(scanned);

    if states.is_empty() {
        return Err(
            "fixture has no live chunk with a non-air voxel to edit; the gate needs real targets"
                .to_string(),
        );
    }

    let chunks_available = states.len();
    let mut edited_chunks = vec![false; chunks_available];
    let mut device = UploadDevice::new(chunks_available);
    let mut edits = Vec::with_capacity(EDITS);
    // Reused scrub scratch: one scheduler, one output batch, one bitmask, one
    // block scratch and one probe-selection list for the whole run.
    let probe_grid = ProbeGrid::for_chunk(ProbeLod::Near);
    let mut scheduler = RemeshScheduler::new();
    let mut scrub_blocks = Vec::new();
    let mut affected = Vec::new();
    let mut scrub_quads = Vec::new();
    let mut scratch = BlockScratch::new();

    for step in 0..(WARMUP_EDITS + EDITS) {
        let record = step >= WARMUP_EDITS;
        let state_index = step % chunks_available;
        let visit = step / chunks_available;
        let state = &mut states[state_index];
        let candidate = state.candidates[visit % state.candidates.len()];
        // Read the current value so every measured edit is a real change, not
        // a repeated write of the material the fixture already holds.
        let (vx, vy, vz) = local_xyz(candidate.index);
        let previous_material = state.dag.get(vx, vy, vz);
        let material = if previous_material == candidate.first {
            candidate.second
        } else {
            candidate.first
        };

        // Keep the arena in a steady state: without periodic compaction the
        // retired band slots accumulate and p95 depends on the edit index.
        let compact_start = Instant::now();
        if step.is_multiple_of(64) {
            let _ = state.dag.compact_if_needed();
        }
        // Charged to the edit that triggered it. Without this the amortized
        // O(arena) rebuild was invisible to the p95 and to the 128³ projection.
        let compact_ns = elapsed_ns(compact_start);
        let before = BandSnapshot::capture(&state.dag);

        let cow_start = Instant::now();
        let stats = state.dag.apply_edits([(candidate.index, material)]);
        let cow_ns = elapsed_ns(cow_start);
        black_box(&stats);

        let upload_start = Instant::now();
        let uploaded = device.upload(&state.dag, &before);
        let upload_ns = elapsed_ns(upload_start);
        black_box(device.staging.as_slice());
        black_box(device.leaves.as_slice());

        let fence_start = Instant::now();
        let token = device.fence.submit();
        device.fence.signal(token);
        device.fence.wait(token);
        let fence_ns = elapsed_ns(fence_start);

        let flip_start = Instant::now();
        let version = device.flip(state_index);
        let flip_ns = elapsed_ns(flip_start);
        black_box(version);

        let light_start = Instant::now();
        let (light_cells, fill) = relight(state, candidate.index, material, &light_table);
        let light_ns = elapsed_ns(light_start);
        black_box(&fill);

        // M2-24 scrub path: the edit triggers dirty blocks (M2-22), one
        // scheduler batch (M2-23), the production greedy remesh, incremental
        // light (M2-15) and the production probe update (M2-21).
        let cell_block = BLOCK_DIM as u32;
        let cx = (vx / cell_block) as i32;
        let cy = (vy / cell_block) as i32;
        let cz = (vz / cell_block) as i32;
        let sky_edge = LIGHT_CELLS_PER_EDGE as i32;
        let sky_index = (cx + cy * sky_edge + cz * sky_edge * sky_edge) as usize;

        let scrub_dirty_start = Instant::now();
        let mut dirty = DirtyBlocks::new();
        assert!(
            dirty.mark_edit(vx, vy, vz, previous_material, material),
            "a measured edit must change the voxel"
        );
        let queued = scheduler.enqueue(&dirty);
        let dirty_blocks = scheduler.next_batch_into(usize::MAX, &mut scrub_blocks);
        black_box(queued);
        let scrub_dirty_ns = elapsed_ns(scrub_dirty_start);

        // Selection sees the pre-edit voxel; `dense` is not updated until the
        // selection has run, so this is the same pre-edit view the probe gate
        // uses.
        let probe_start = Instant::now();
        let reach = probes_reaching(
            state.pos,
            probe_grid,
            [vx, vy, vz],
            |x, y, z| state.dense[local_index(x, y, z) as usize] != AIR,
            &mut affected,
        );
        state.dense[candidate.index as usize] = material;
        debug_assert_eq!(
            state.dag.get(vx, vy, vz),
            material,
            "the dense scrub buffer must track the COW chunk"
        );
        let rebaked = {
            let dense = &state.dense;
            state.probe.rebake(state.pos, &affected, |x, y, z| {
                dense[local_index(x, y, z) as usize] != AIR
            })
        };
        black_box((reach, rebaked));
        let probe_ns = elapsed_ns(probe_start);

        let scrub_light_start = Instant::now();
        let cell_light = light_for(&light_table, material);
        state.light_state.apply_edit(CellEdit::new(
            cx,
            cy,
            cz,
            MeshBlockLight::new(cell_light.emission, cell_light.opacity),
            state.sky[sky_index],
        ));
        let scrub_light_ns = elapsed_ns(scrub_light_start);

        let remesh_start = Instant::now();
        let mut meshed_quads = 0usize;
        for &block in &scrub_blocks[..dirty_blocks] {
            scratch.fill(&state.dense, &state.apron, block);
            greedy_block(&scratch, &mut scrub_quads);
            meshed_quads += scrub_quads.len();
        }
        let remesh_ns = elapsed_ns(remesh_start);
        black_box(meshed_quads);
        let scrub_total_ns =
            compact_ns + cow_ns + scrub_dirty_ns + remesh_ns + scrub_light_ns + probe_ns;

        if record {
            edited_chunks[state_index] = true;
            let fixed_ns = upload_ns + fence_ns + flip_ns;
            let dirty_ns = cow_ns + light_ns;
            edits.push(EditSample {
                edit: (step - WARMUP_EDITS) as u32,
                chunk: [state.pos.x, state.pos.y, state.pos.z],
                voxel: candidate.index,
                material,
                previous_material,
                cow_runs: stats.runs,
                cow_nodes: stats.nodes,
                cow_bytes: stats.bytes,
                cow_leaf_runs: stats.leaf_runs,
                upload_node_bytes: uploaded.node_bytes,
                upload_leaf_bytes: uploaded.leaf_bytes,
                upload_level_start_bytes: uploaded.level_start_bytes,
                upload_bytes: uploaded.total(),
                light_cells,
                version,
                cow_ns,
                upload_ns,
                fence_ns,
                flip_ns,
                light_ns,
                fixed_ns,
                compact_ns,
                dirty_ns,
                total_ns: fixed_ns + dirty_ns + compact_ns,
                dirty_blocks: dirty_blocks as u32,
                meshed_quads: meshed_quads as u32,
                affected_probes: affected.len() as u32,
                scrub_dirty_ns,
                remesh_ns,
                scrub_light_ns,
                probe_ns,
                scrub_total_ns,
            });
        }
    }

    let terms = EditTerms {
        upload: term(&edits, |sample| sample.upload_ns),
        fence: term(&edits, |sample| sample.fence_ns),
        flip: term(&edits, |sample| sample.flip_ns),
        fixed: term(&edits, |sample| sample.fixed_ns),
        cow: term(&edits, |sample| sample.cow_ns),
        light: term(&edits, |sample| sample.light_ns),
        dirty: term(&edits, |sample| sample.dirty_ns),
        total: term(&edits, |sample| sample.total_ns),
    };
    let bytes = EditBytes {
        upload_bytes: edits.iter().map(|sample| sample.upload_bytes).sum(),
        upload_node_bytes: edits.iter().map(|sample| sample.upload_node_bytes).sum(),
        upload_leaf_bytes: edits.iter().map(|sample| sample.upload_leaf_bytes).sum(),
        upload_level_start_bytes: edits
            .iter()
            .map(|sample| sample.upload_level_start_bytes)
            .sum(),
        cow_bytes: edits.iter().map(|sample| sample.cow_bytes).sum(),
        upload_bytes_per_edit: mean(edits.iter().map(|sample| sample.upload_bytes)),
        cow_bytes_per_edit: mean(edits.iter().map(|sample| sample.cow_bytes)),
        light_cells_per_edit: mean(edits.iter().map(|sample| sample.light_cells)),
    };
    let budget = BudgetCheck::new(BUDGET_NS, terms.total.p95_ns);
    let scrub_terms = ScrubTerms {
        dirty: term(&edits, |sample| sample.scrub_dirty_ns),
        remesh: term(&edits, |sample| sample.remesh_ns),
        light: term(&edits, |sample| sample.scrub_light_ns),
        probe: term(&edits, |sample| sample.probe_ns),
        total: term(&edits, |sample| sample.scrub_total_ns),
    };
    let scrub_counts = ScrubCounts {
        dirty_blocks_per_edit: mean(edits.iter().map(|sample| sample.dirty_blocks as u64)),
        quads_per_edit: mean(edits.iter().map(|sample| sample.meshed_quads as u64)),
        affected_probes_per_edit: mean(edits.iter().map(|sample| sample.affected_probes as u64)),
    };
    let scrub_budget = BudgetCheck::new(SCRUB_BUDGET_NS, scrub_terms.total.p95_ns);

    let status = match manifest.status {
        FixtureStatus::Provisional => "provisional".to_owned(),
    };
    let fixture = FixtureInfo {
        id: manifest.id.clone(),
        status,
        provisional_until_first_real_save: manifest.provisional_until_first_real_save,
        limitation: manifest.limitation.clone(),
        manifest: manifest_path.display().to_string(),
        save_dir: save_dir.display().to_string(),
        world_hash: manifest.integrity.world_hash.clone(),
        chunk_bits: CHUNK_BITS,
        chunk_dim: CHUNK_DIM as u32,
        chunks: summary.chunks,
        total_voxels,
        overlay_entries,
        region_files: summary.region_files,
    };

    let notes = vec![
        "Fixed = upload + fence + flip; dirty = COW + light; total = fixed + dirty. \
         Every component is timed on its own and the aggregate terms are the per-edit sums, \
         so sub-timer overhead is charged to the aggregate and the rows reconcile."
            .to_owned(),
        format!(
            "Upload is the CPU-side staging copy of exactly the appended internal runs (padding \
             included because it is real), the appended leaf materials and the {BAND_COUNT}-word \
             levelStart snapshot. No padding words and no whole-chunk transfer are charged: \
             {bytes_per_edit} B/edit ({node_per_edit} node, {leaf_per_edit} leaf, \
             {start_per_edit} levelStart). A real device transfer adds GPU time, so this is a \
             lower bound on the upload half of the fixed term.",
            bytes_per_edit = bytes.upload_bytes_per_edit,
            node_per_edit = bytes.upload_node_bytes / EDITS as u64,
            leaf_per_edit = bytes.upload_leaf_bytes / EDITS as u64,
            start_per_edit = bytes.upload_level_start_bytes / EDITS as u64,
        ),
        "The fence is the CPU side of the M4 protocol: submit a token, publish it with a Release \
         store, poll with an Acquire load, then flip only after it is visible. A headless gate run \
         has no GPU, so this is a lower bound on a real device fence; M4-05/M4-16 replace it. \
         Each edit is charged its own batch, so one fence is paid per edit; a frame that coalesces \
         N edits amortizes that fixed cost rather than multiplying it. The fence and flip rows sit \
         at the timer's resolution and describe boundary bookkeeping, not GPU completion time."
            .to_owned(),
        "COW is ChunkDag::apply_edits on the fixture chunk. Light is the M1-29 reference \
         full_pass over the edited block cell plus its one-cell apron (27 cells); the edited cell \
         uses the material just written and the apron samples the current DAG. A single-voxel \
         edit is charged one dirty cell. Sky exposure is the chunk-local heightmap: a chunk \
         directly above is not consulted and the apron is treated as open sky. M2 replaces this \
         reference with the incremental pass. The M2-24 scrub row below uses the production \
         incremental pass."
            .to_owned(),
        "Scrub (M2-24) = compaction + cow + dirty + remesh + light + probe. Dirty is the M2-22 per-block \
         bitset plus one M2-23 scheduler batch; probe is M2-21 selection plus the production \
         rebake; light is the M2-15 incremental LightState edit; remesh is the production greedy \
         mesher over the batch's 18³ scratches (filled from the dense chunk and the six-neighbour \
         apron). It runs on every measured edit and is compared with the 4 ms scrub budget, \
         separately from the pre-registered 8 ms edit budget. Startup (states, aprons, light seed \
         and the initial probe bake) is outside every timer."
            .to_owned(),
        "The frozen fixture pairs two non-air materials on every edit. That swap is \
         occupancy-neutral, so the M2-22 batch is always the voxel's own block (mark_edit's \
         solid→solid fast path) and no probe's baked visibility can change: the probe term is \
         M2-21 selection only (0 affected probes) and the rebake half is measured by the probe \
         gate. The apron is built once from resident neighbours; cross-chunk dirtiness \
         (mark_apron_cell) belongs to the streaming layer and is out of scope here."
            .to_owned(),
        "Sky exposure is the startup chunk-local heightmap and is not recomputed after an \
         opacity-changing edit. Light values therefore follow the same static-sky model as the \
         M1-29 reference and are not validated against a fresh bake; only the light latency is \
         used by this gate."
            .to_owned(),
        format!(
            "The fixture has {} live chunks; {} contain at least one non-air voxel and are the \
             edit targets. The {} all-air chunks are not padded with synthetic edits. A target \
             alternates between two real materials: another material present in its chunk when \
             one exists, else stone/cobblestone, so every measured edit changes the voxel.",
            grouped(fixture.chunks as u64),
            grouped(chunks_available as u64),
            grouped(fixture.chunks.saturating_sub(chunks_available) as u64),
        ),
        "No synthetic chunks or padding: every candidate is a real non-air voxel spread through a \
         live fixture chunk, and each measured edit writes a material different from the value \
         the voxel held, so no sample is a no-op. The fixture remains provisional until the first \
         real save."
            .to_owned(),
    ];

    Ok(EditsReport {
        schema_version: SCHEMA_VERSION,
        fixture,
        edit_count: edits.len(),
        warmup_edits: WARMUP_EDITS,
        chunks_edited: edited_chunks.iter().filter(|&&edited| edited).count(),
        chunks_available,
        terms,
        bytes,
        budget,
        scrub_terms,
        scrub_counts,
        scrub_budget,
        edits,
        notes,
    })
}

/// Builds the material-to-light table from the built-in block properties.
///
/// The fixture stores raw built-in ids, so the registry order is the fixture's
/// palette. `OPAQUE` blocks become fully absorbing even though the authoring
/// helper leaves `light_opacity` at zero for a uniform solid; that flag is what
/// the light reference means by a full blocker.
fn builtin_light_table() -> Vec<BlockLight> {
    let mut builder = BlockRegistryBuilder::with_capacity(BlockKind::ALL.len());
    for kind in BlockKind::ALL.into_iter().skip(1) {
        builder
            .register(kind.key(), kind.default_properties())
            .expect("built-in keys are unique and the table has room");
    }
    let registry = builder.freeze().expect("built-in blocks fit the registry");
    (0..registry.len())
        .map(|raw| {
            if raw == AIR as usize {
                return BlockLight::AIR;
            }
            let hot = registry.hot(MaterialId::from_raw(raw as u16));
            if hot.flags().contains(BlockFlags::OPAQUE) {
                BlockLight::new(hot.light_emission(), 15)
            } else {
                BlockLight::from_hot(hot)
            }
        })
        .collect()
}

/// Up to [`CANDIDATES_PER_CHUNK`] non-air voxels spread evenly through a chunk.
fn candidates(dense: &[u16]) -> Vec<Candidate> {
    let mut palette: Vec<u16> = dense
        .iter()
        .copied()
        .filter(|&material| material != AIR)
        .collect();
    palette.sort_unstable();
    palette.dedup();

    let window = CHUNK_VOLUME.div_ceil(CANDIDATES_PER_CHUNK);
    let mut out = Vec::with_capacity(CANDIDATES_PER_CHUNK);
    for slot in 0..CANDIDATES_PER_CHUNK {
        let start = slot * window;
        let end = (start + window).min(CHUNK_VOLUME);
        if start >= end {
            break;
        }
        let Some((offset, &first)) = dense[start..end]
            .iter()
            .enumerate()
            .find(|&(_, &material)| material != AIR)
        else {
            continue;
        };
        let second = palette
            .iter()
            .copied()
            .find(|&material| material != first)
            .unwrap_or({
                // A monochrome chunk still needs a contrasting real material so
                // the edit changes the voxel instead of rewriting its value.
                if first == crate::fixture::mat::COBBLESTONE {
                    crate::fixture::mat::STONE
                } else {
                    crate::fixture::mat::COBBLESTONE
                }
            });
        out.push(Candidate {
            index: (start + offset) as u32,
            first,
            second,
        });
    }
    out
}

/// Chunk-local sky exposure for every light cell.
///
/// A cell is exposed when the chunk has no fully opaque voxel above its origin
/// column. The top layer is open sky; a chunk directly above this one is not
/// consulted, which is documented as a limitation of the reference.
fn sky_cells(dense: &[u16], light: &[BlockLight]) -> Vec<bool> {
    let edge = LIGHT_CELLS_PER_EDGE;
    let mut sky = vec![false; LIGHT_CELLS_PER_CHUNK];
    for cz in 0..edge {
        for cy in 0..edge {
            for cx in 0..edge {
                let x = (cx * BLOCK_DIM) as u32;
                let z = (cz * BLOCK_DIM) as u32;
                let mut exposed = true;
                for y in ((cy + 1) * BLOCK_DIM)..CHUNK_DIM {
                    let material = dense[local_index(x, y as u32, z) as usize];
                    if is_opaque(light, material) {
                        exposed = false;
                        break;
                    }
                }
                sky[cx + cy * edge + cz * edge * edge] = exposed;
            }
        }
    }
    sky
}

/// The chunk sharing `face` with `pos`.
fn neighbour_pos(pos: ChunkPos, face: Face) -> ChunkPos {
    match face {
        Face::NegX => ChunkPos::new(pos.x - 1, pos.y, pos.z),
        Face::PosX => ChunkPos::new(pos.x + 1, pos.y, pos.z),
        Face::NegY => ChunkPos::new(pos.x, pos.y - 1, pos.z),
        Face::PosY => ChunkPos::new(pos.x, pos.y + 1, pos.z),
        Face::NegZ => ChunkPos::new(pos.x, pos.y, pos.z - 1),
        Face::PosZ => ChunkPos::new(pos.x, pos.y, pos.z + 1),
    }
}

/// Chunk-local coordinate of the neighbour voxel on `face`'s plane at `(u, v)`.
///
/// This is the inverse of [`Face::offset`]: the plane lies outside this chunk,
/// so the coordinate is the touching border voxel of the neighbour.
fn face_local(face: Face, u: u32, v: u32) -> (u32, u32, u32) {
    let last = CHUNK_DIM as u32 - 1;
    match face {
        Face::NegX => (last, v, u),
        Face::PosX => (0, v, u),
        Face::NegY => (v, last, u),
        Face::PosY => (v, 0, u),
        Face::NegZ => (u, v, last),
        Face::PosZ => (u, v, 0),
    }
}

/// Builds the six face planes from the scanned neighbours, or air where absent.
fn build_apron(
    pos: ChunkPos,
    scanned: &[(ChunkPos, Vec<u16>)],
    index: &HashMap<ChunkPos, usize>,
) -> Apron {
    let mut apron = Apron::new();
    for face in Face::ALL {
        let Some(&slot) = index.get(&neighbour_pos(pos, face)) else {
            continue;
        };
        let dense = &scanned[slot].1;
        for v in 0..CHUNK_DIM as u32 {
            for u in 0..CHUNK_DIM as u32 {
                let (x, y, z) = face_local(face, u, v);
                apron.set(face, u, v, dense[local_index(x, y, z) as usize]);
            }
        }
    }
    apron
}

/// Production incremental light state (M2-15) over the chunk's cell grid.
///
/// Outside the chunk is air and open sky, matching the reference fill's
/// documented limitation: a chunk directly above is not consulted.
fn build_light_state(dense: &[u16], sky: &[bool], light: &[BlockLight]) -> LightState {
    let edge = LIGHT_CELLS_PER_EDGE as i32;
    let block = BLOCK_DIM as u32;
    LightState::new(
        LIGHT_CELLS_PER_EDGE,
        |x, y, z| {
            if x < 0 || y < 0 || z < 0 || x >= edge || y >= edge || z >= edge {
                return MeshBlockLight::AIR;
            }
            let material =
                dense[local_index(x as u32 * block, y as u32 * block, z as u32 * block) as usize];
            let entry = light_for(light, material);
            MeshBlockLight::new(entry.emission, entry.opacity)
        },
        |x, y, z| {
            if x < 0 || y < 0 || z < 0 || x >= edge || y >= edge || z >= edge {
                return true;
            }
            sky[(x + y * edge + z * edge * edge) as usize]
        },
    )
}

/// Runs the M1-29 reference fill over the dirty cell and its apron.
fn relight(
    state: &ChunkState,
    voxel: u32,
    material: u16,
    light: &[BlockLight],
) -> (u64, LightFill) {
    let (x, y, z) = local_xyz(voxel);
    let edge = LIGHT_CELLS_PER_EDGE as i32;
    let block = BLOCK_DIM as u32;
    let cx = (x / block) as i32;
    let cy = (y / block) as i32;
    let cz = (z / block) as i32;

    let fill = full_pass(
        1,
        |dx, dy, dz| {
            let gx = cx + dx;
            let gy = cy + dy;
            let gz = cz + dz;
            if gx < 0 || gx >= edge || gy < 0 || gy >= edge || gz < 0 || gz >= edge {
                return BlockLight::AIR;
            }
            if dx == 0 && dy == 0 && dz == 0 {
                return light_for(light, material);
            }
            let vx = gx as u32 * block;
            let vy = gy as u32 * block;
            let vz = gz as u32 * block;
            light_for(light, state.dag.get(vx, vy, vz))
        },
        |dx, dy, dz| {
            let gx = cx + dx;
            let gy = cy + dy;
            let gz = cz + dz;
            if gx < 0 || gx >= edge || gy < 0 || gy >= edge || gz < 0 || gz >= edge {
                return true;
            }
            state.sky[(gx + gy * edge + gz * edge * edge) as usize]
        },
    );

    (fill.padded_cells().len() as u64, fill)
}

fn light_for(light: &[BlockLight], material: u16) -> BlockLight {
    light
        .get(material as usize)
        .copied()
        .unwrap_or(BlockLight::OPAQUE)
}

fn is_opaque(light: &[BlockLight], material: u16) -> bool {
    light_for(light, material).is_fully_opaque()
}

fn elapsed_ns(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn term_uses_nearest_rank_and_keeps_the_sum() {
        let mut values = [5, 1, 4, 2, 3];
        let term = Term::from_values(&mut values);
        assert_eq!(term.p50_ns, 3);
        assert_eq!(term.p95_ns, 5);
        assert_eq!(term.max_ns, 5);
        assert_eq!(term.mean_ns, 3);
        assert_eq!(term.total_ns, 15);
    }

    #[test]
    fn upload_copies_only_the_appended_runs_and_level_start() {
        let dense = crate::patterns::terrain(0, 0);
        let mut dag = ChunkDag::from_dense(&dense);
        let before = BandSnapshot::capture(&dag);
        let stats = dag.apply_edits([(local_index(1, 2, 3), 4)]);
        let mut device = UploadDevice::new(1);
        let uploaded = device.upload(&dag, &before);

        assert_eq!(
            uploaded.node_bytes,
            stats.node_bytes(),
            "uploaded node bytes are exactly the COW node bytes"
        );
        assert_eq!(uploaded.leaf_bytes, stats.leaf_bytes);
        assert_eq!(
            uploaded.level_start_bytes,
            (BAND_COUNT * core::mem::size_of::<Node>()) as u64
        );
        assert_eq!(
            uploaded.total(),
            stats.bytes + uploaded.level_start_bytes,
            "no padding beyond the real run bytes and levelStart"
        );
        assert_eq!(
            device.staging.len() as u64 * 8,
            uploaded.node_bytes + uploaded.level_start_bytes
        );
        assert_eq!(device.leaves.len() as u64 * 2, uploaded.leaf_bytes);
    }

    fn chunk_state(dense: Vec<u16>) -> ChunkState {
        let light = builtin_light_table();
        let sky = sky_cells(&dense, &light);
        let pos = ChunkPos::new(0, 0, 0);
        let light_state = build_light_state(&dense, &sky, &light);
        let probe =
            voxel_mesh::probe::bake(pos, ProbeGrid::for_chunk(ProbeLod::Near), |x, y, z| {
                dense[local_index(x, y, z) as usize] != AIR
            });
        ChunkState {
            pos,
            dag: ChunkDag::from_dense(&dense),
            dense,
            apron: Apron::new(),
            candidates: Vec::new(),
            sky,
            light_state,
            probe,
        }
    }

    #[test]
    fn one_edit_relights_one_cell_and_its_apron() {
        let dense = crate::patterns::terrain(0, 0);
        let light = builtin_light_table();
        let state = chunk_state(dense);
        let (cells, fill) = relight(&state, local_index(1, 2, 3), 1, &light);
        assert_eq!(cells, 27);
        assert_eq!(fill.padded_cells().len(), 27);
        assert_eq!(fill.edge_cells(), 1);
    }

    #[test]
    fn apron_reads_the_facing_neighbour_border() {
        let last = CHUNK_DIM as u32 - 1;
        let mut west = vec![AIR; CHUNK_VOLUME];
        let mut east = vec![AIR; CHUNK_VOLUME];
        west[local_index(last, 5, 9) as usize] = 3;
        east[local_index(0, 5, 9) as usize] = 4;
        let scanned = vec![
            (ChunkPos::new(0, 0, 0), west),
            (ChunkPos::new(1, 0, 0), east),
        ];
        let index = scanned
            .iter()
            .enumerate()
            .map(|(slot, (pos, _))| (*pos, slot))
            .collect();

        // East's -X plane (u = z, v = y) reads west's last x column.
        let apron = build_apron(ChunkPos::new(1, 0, 0), &scanned, &index);
        assert_eq!(apron.get(Face::NegX, 9, 5), 3);
        // West's +X plane reads east's first x column.
        let apron = build_apron(ChunkPos::new(0, 0, 0), &scanned, &index);
        assert_eq!(apron.get(Face::PosX, 9, 5), 4);
    }

    #[test]
    fn candidates_are_real_non_air_voxels_from_the_chunk_palette() {
        let dense = crate::patterns::terrain(0, 0);
        let candidates = candidates(&dense);
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            assert_ne!(dense[candidate.index as usize], AIR);
            assert_ne!(candidate.first, AIR);
            assert_ne!(candidate.second, AIR);
            assert_ne!(
                candidate.first, candidate.second,
                "the replacement must change the voxel"
            );
            assert_eq!(dense[candidate.index as usize], candidate.first);
        }
    }

    #[test]
    fn fence_waits_for_the_matching_token() {
        let mut fence = Fence::new();
        let first = fence.submit();
        let second = fence.submit();
        assert!(second > first);
        fence.signal(first);
        fence.wait(first);
        fence.signal(second);
        fence.wait(second);
    }

    #[test]
    fn the_builtin_light_table_blocks_solid_materials() {
        let light = builtin_light_table();
        assert!(!light[0].is_fully_opaque(), "air is transparent");
        assert!(light[crate::fixture::mat::STONE as usize].is_fully_opaque());
        assert!(!light[crate::fixture::mat::WATER as usize].is_fully_opaque());
    }
}
