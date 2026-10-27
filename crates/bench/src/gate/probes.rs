//! Probe-update latency on the frozen fixture (M1-38).
//!
//! The pre-registered probe budget is `p95 <= 4 ms` per edited chunk
//! (`pre-registration.md` §3). ADR-005 fixes the update rule: an edit rebakes
//! only the probes whose rays reach the dirty region, not the whole grid. This
//! module measures that rule end to end:
//!
//! | Term | Components | What is measured |
//! | --- | --- | --- |
//! | **Update** | select, rebake, upload | Find the probes whose rays reach the edited voxel, cast their 64-ray hemisphere again, copy the changed 12 B records and validity bytes to staging |
//!
//! The near grid has one probe per block: **64 probes at 64³**, each casting
//! **64** upper-hemisphere rays. A full rebake is 4,096 rays; an edit pays only
//! for affected probes. Selection traces from the pre-edit state because a ray
//! that stops at a solid voxel today will continue after a solid→air edit, and
//! a ray that passes through an air voxel today will stop after air→solid.
//! A probe whose centre is the edited voxel is always selected: its validity
//! can change even though it casts no ray while invalid.
//!
//! # Workload
//!
//! Every candidate is a **real solid voxel that is the first solid on a probe
//! ray** in the fixture. That is the class of edit that can change probe
//! radiance; an edit buried behind solid geometry has no visible ray and would
//! report zero work. Candidates are spread over probes and rays, and each edit
//! alternates solid→air (mine) and air→solid (place), so the measured update is
//! never a material swap that leaves every ray's occlusion unchanged. No chunk
//! is generated, padded or carved: the fixture's own voxels are edited and
//! restored.
//!
//! # What is real and what is a model
//!
//! The bake and selection are the M1-30 reference implementation, shared with
//! the test oracle. The upload is a CPU-side staging copy of exactly the changed
//! records and validity bytes; the headless gate has no GPU, so device transfer
//! and the M4 fence/flip are excluded, just as the edit gate excludes GPU time.
//! The fixture is **provisional until the first real save**; the report carries
//! that limitation verbatim.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use voxel_core::content_constants::PROBE_BYTES;
use voxel_storage::reference::probe::{
    PROBE_RAYS, ProbeGrid, ProbeLod, ProbeVolume, bake, first_solid_on_ray, hemisphere_ray,
    probe_rotation, probe_seed, probes_reaching,
};
use voxel_storage::scan::scan_save;
use voxel_storage::serialize;
use voxel_storage::{
    AIR, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME, ChunkDag, ChunkPos, local_index, local_xyz,
};

use crate::fixture::{self, FixtureStatus, Manifest};
use crate::save_scan::Distribution;

use super::memory::FixtureInfo;
use super::stat::{
    BudgetCheck, Term, format_duration, format_signed_duration, grouped, mean, term,
};

/// Schema version of the probe-update report itself.
pub const SCHEMA_VERSION: u32 = 1;

/// Measured edits. The gate reports a distribution over edited chunks, not a
/// microbenchmark with a mutable sample count.
pub const EDITS: usize = 1_000;

/// Untimed updates before the measured run, so cold paths do not set the tail.
pub const WARMUP_EDITS: usize = 100;

/// Pre-registered probe-update budget: p95 ≤ 4 ms (`pre-registration.md` §3).
pub const BUDGET_NS: u64 = 4_000_000;

/// Candidate edits per fixture chunk.
const CANDIDATES_PER_CHUNK: usize = 16;

/// The three timed components and the aggregate update term.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeTerms {
    /// Find the probes whose rays reach the dirty voxel.
    pub select: Term,
    /// Recast the affected probes' hemisphere and encode SH4 RGB8.
    pub rebake: Term,
    /// Stage the changed records and validity bytes.
    pub upload: Term,
    /// `select + rebake + upload`.
    pub update: Term,
}

/// One measured probe update, including the edit that triggered it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeSample {
    /// Zero-based index in the measured run.
    pub edit: u32,
    /// Fixture chunk whose probes were updated.
    pub chunk: [i32; 3],
    /// Chunk-local voxel index that was edited.
    pub voxel: u32,
    /// Material written by the edit.
    pub material: u16,
    /// Material the voxel held immediately before this edit.
    pub previous_material: u16,
    /// True when the edit removed a solid voxel; false when it restored one.
    pub solid_to_air: bool,
    /// Probes selected for rebake.
    pub affected_probes: u32,
    /// Selected probes whose centre is air after the edit.
    pub valid_probes: u32,
    /// Rays cast while selecting (valid probes only).
    pub rays_tested: u32,
    /// Selection rays that reach the dirty voxel.
    pub rays_reaching: u32,
    /// Rays cast by the rebake; `valid_probes × 64`.
    pub rays_baked: u32,
    /// SH bytes staged for changed probes (`affected_probes × 12`).
    pub probe_bytes: u64,
    /// Validity bytes staged, one per affected probe.
    pub validity_bytes: u64,
    /// Selection latency.
    pub select_ns: u64,
    /// Rebake latency.
    pub bake_ns: u64,
    /// Staging-copy latency.
    pub upload_ns: u64,
    /// `select_ns + bake_ns + upload_ns`.
    pub update_ns: u64,
}

/// Probe-work distribution and totals behind the timing rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCounts {
    /// Affected probes per edited chunk.
    pub affected: Distribution,
    /// Selection rays tested per edit.
    pub rays_tested: Distribution,
    /// Selection rays reaching the dirty voxel per edit.
    pub rays_reaching: Distribution,
    /// Rays rebaked per edit.
    pub rays_baked: Distribution,
    /// Sum of affected probes over the run.
    pub affected_total: u64,
    /// Mean affected probes per edit.
    pub affected_per_edit: u64,
    /// Mean selection rays tested per edit.
    pub rays_tested_per_edit: u64,
    /// Mean selection rays that reached the edit.
    pub rays_reaching_per_edit: u64,
    /// Sum of rebaked rays over the run.
    pub rays_baked_total: u64,
    /// Mean rebaked rays per edit.
    pub rays_baked_per_edit: u64,
    /// Edits that selected no probe.
    pub zero_affected_edits: u64,
    /// Edits that removed a solid voxel.
    pub solid_to_air_edits: u64,
    /// Edits that restored one.
    pub air_to_solid_edits: u64,
}

/// Bytes copied to the probe staging buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeBytes {
    /// Sum of `ProbeSample::probe_bytes`.
    pub probe_bytes: u64,
    /// Sum of `ProbeSample::validity_bytes`.
    pub validity_bytes: u64,
    /// Sum of both ranges.
    pub upload_bytes: u64,
    /// Mean SH bytes per edit.
    pub probe_bytes_per_edit: u64,
    /// Mean validity bytes per edit.
    pub validity_bytes_per_edit: u64,
    /// Mean total upload bytes per edit.
    pub upload_bytes_per_edit: u64,
}

/// The complete M1-38 probe-update report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbesReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Fixture identity and provenance.
    pub fixture: FixtureInfo,
    /// Measured edits.
    pub edit_count: usize,
    /// Untimed edits before the measured run.
    pub warmup_edits: usize,
    /// Fixture chunks that received a measured update.
    pub chunks_edited: usize,
    /// Fixture chunks available as edit targets.
    pub chunks_available: usize,
    /// Probes in the near grid for the compiled chunk edge.
    pub probes_per_chunk: usize,
    /// Hemisphere rays per valid probe.
    pub rays_per_probe: usize,
    /// Component and aggregate terms.
    pub terms: ProbeTerms,
    /// Probe and ray distribution.
    pub counts: ProbeCounts,
    /// Staged byte totals.
    pub bytes: ProbeBytes,
    /// Budget comparison.
    pub budget: BudgetCheck,
    /// Every measured update, in run order.
    pub updates: Vec<ProbeSample>,
    /// Definitions and caveats a reader needs to interpret the table.
    pub notes: Vec<String>,
}

impl ProbesReport {
    /// True while the measured fixture is the synthetic M1 placeholder.
    pub fn is_provisional(&self) -> bool {
        self.fixture.provisional_until_first_real_save
    }

    /// Renders the report as a Markdown table plus provenance and notes.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate probes (M1-38)\n").unwrap();
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
            "**Edits:** {} measured + {} warm-up, across {} edited chunks \
             ({} targets of {} live)\n",
            grouped(self.edit_count as u64),
            grouped(self.warmup_edits as u64),
            grouped(self.chunks_edited as u64),
            grouped(self.chunks_available as u64),
            grouped(self.fixture.chunks as u64),
        )
        .unwrap();
        writeln!(
            out,
            "**Grid:** {} near probes/chunk × {} hemisphere rays = {} rays for a full rebake\n",
            self.probes_per_chunk,
            self.rays_per_probe,
            grouped((self.probes_per_chunk * self.rays_per_probe) as u64),
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
        for (component, value) in [
            ("Select", &self.terms.select),
            ("Rebake", &self.terms.rebake),
            ("Upload", &self.terms.upload),
            ("**Select + rebake + upload**", &self.terms.update),
        ] {
            writeln!(
                out,
                "| Update | {component} | {} | {} | {} | {} |",
                format_duration(value.p50_ns),
                format_duration(value.p95_ns),
                format_duration(value.max_ns),
                format_duration(value.total_ns),
            )
            .unwrap();
        }

        writeln!(
            out,
            "\n**Affected probes:** p50 {} / p95 {} / max {} of {}; {} edits touched zero probes\n",
            optional(self.counts.affected.p50),
            optional(self.counts.affected.p95),
            optional(self.counts.affected.max),
            self.probes_per_chunk,
            grouped(self.counts.zero_affected_edits),
        )
        .unwrap();
        writeln!(
            out,
            "**Rays:** {} tested/edit, {} reached the edit, {} rebaked/edit; {} rebaked in total\n",
            self.counts.rays_tested_per_edit,
            self.counts.rays_reaching_per_edit,
            self.counts.rays_baked_per_edit,
            grouped(self.counts.rays_baked_total),
        )
        .unwrap();
        writeln!(
            out,
            "**Upload:** {} B/edit ({} SH + {} validity); {} B total\n",
            self.bytes.upload_bytes_per_edit,
            self.bytes.probe_bytes_per_edit,
            self.bytes.validity_bytes_per_edit,
            grouped(self.bytes.upload_bytes),
        )
        .unwrap();
        writeln!(
            out,
            "**Edits:** {} solid→air, {} air→solid\n",
            grouped(self.counts.solid_to_air_edits),
            grouped(self.counts.air_to_solid_edits),
        )
        .unwrap();

        for note in &self.notes {
            writeln!(out, "\n- {note}").unwrap();
        }
        out
    }
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

/// Measures the default fixture manifest.
pub fn default_report() -> Result<ProbesReport, String> {
    report(&fixture::default_manifest())
}

/// Measures the fixture named by `manifest_path`.
///
/// The manifest is checked for schema, chunk edge and chunk version before the
/// save is read, matching the other gate harnesses. A save edited after freezing
/// is an error, not a silently different report.
pub fn report(manifest_path: &Path) -> Result<ProbesReport, String> {
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
    material: u16,
}

/// A fixture chunk with its current DAG and probe volume.
struct ChunkState {
    pos: ChunkPos,
    dag: ChunkDag,
    volume: ProbeVolume,
    candidates: Vec<Candidate>,
}

/// CPU-side staging buffer for the changed probe records and validity bytes.
struct ProbeStaging {
    records: Vec<u8>,
    validity: Vec<u8>,
}

/// Bytes copied for one update.
#[derive(Clone, Copy, Debug, Default)]
struct UploadBytes {
    probe: u64,
    validity: u64,
}

impl ProbeStaging {
    fn new(probes: usize) -> Self {
        Self {
            records: Vec::with_capacity(probes * PROBE_BYTES),
            validity: Vec::with_capacity(probes),
        }
    }

    /// Copies exactly the selected probe records and the validity byte that
    /// contains each one. No whole-volume transfer is charged.
    fn upload(&mut self, volume: &ProbeVolume, affected: &[usize]) -> UploadBytes {
        self.records.clear();
        self.validity.clear();
        for &index in affected {
            self.records
                .extend_from_slice(&volume.probe(index).to_bytes());
            self.validity.push(volume.validity_bytes()[index / 8]);
        }
        UploadBytes {
            probe: self.records.len() as u64,
            validity: self.validity.len() as u64,
        }
    }
}

fn measure(
    manifest_path: &Path,
    save_dir: &Path,
    manifest: Manifest,
) -> Result<ProbesReport, String> {
    let grid = ProbeGrid::for_chunk(ProbeLod::Near);
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let mut states = Vec::with_capacity(manifest.observed.live_chunks);
    let mut total_voxels = 0u64;
    let mut overlay_entries = 0u64;

    let summary = scan_save(save_dir, |chunk| {
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }

        total_voxels += dense.iter().filter(|&&material| material != AIR).count() as u64;
        overlay_entries += chunk.overlay.len() as u64;

        let dag = ChunkDag::from_dense(&dense);
        let candidates = candidates(chunk.pos, grid, &dense);
        if candidates.is_empty() {
            return;
        }
        let volume = bake(chunk.pos, grid, |x, y, z| dag.get(x, y, z) != AIR);
        states.push(ChunkState {
            pos: chunk.pos,
            dag,
            volume,
            candidates,
        });
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
    if states.is_empty() {
        return Err(
            "fixture has no chunk with a solid voxel on a probe ray; the gate needs real targets"
                .to_string(),
        );
    }

    let chunks_available = states.len();
    let mut edited_chunks = vec![false; chunks_available];
    let mut staging = ProbeStaging::new(grid.len());
    let mut affected = Vec::with_capacity(grid.len());
    let mut updates = Vec::with_capacity(EDITS);

    for step in 0..(WARMUP_EDITS + EDITS) {
        let record = step >= WARMUP_EDITS;
        let state_index = step % chunks_available;
        let visit = step / chunks_available;
        let state = &mut states[state_index];
        let candidate = state.candidates[visit % state.candidates.len()];
        let (vx, vy, vz) = local_xyz(candidate.index);
        let previous_material = state.dag.get(vx, vy, vz);
        let solid_to_air = previous_material != AIR;
        let material = if solid_to_air {
            AIR
        } else {
            candidate.material
        };

        // Selection must see the pre-edit state: a solid→air edit affects the
        // rays that stop at the voxel today; an air→solid edit affects the rays
        // that pass through it today.
        let select_start = Instant::now();
        let reach = probes_reaching(
            state.pos,
            grid,
            [vx, vy, vz],
            |x, y, z| state.dag.get(x, y, z) != AIR,
            &mut affected,
        );
        let select_ns = elapsed_ns(select_start);
        black_box(reach);

        // The COW edit is the trigger, not probe work; M1-37 charges it.
        let stats = state.dag.apply_edits([(candidate.index, material)]);
        black_box(&stats);

        let bake_start = Instant::now();
        let rebaked = {
            let dag = &state.dag;
            state
                .volume
                .rebake(state.pos, &affected, |x, y, z| dag.get(x, y, z) != AIR)
        };
        let bake_ns = elapsed_ns(bake_start);

        let upload_start = Instant::now();
        let uploaded = staging.upload(&state.volume, &affected);
        let upload_ns = elapsed_ns(upload_start);
        black_box(staging.records.as_slice());
        black_box(staging.validity.as_slice());

        if record {
            edited_chunks[state_index] = true;
            let update_ns = select_ns + bake_ns + upload_ns;
            updates.push(ProbeSample {
                edit: (step - WARMUP_EDITS) as u32,
                chunk: [state.pos.x, state.pos.y, state.pos.z],
                voxel: candidate.index,
                material,
                previous_material,
                solid_to_air,
                affected_probes: affected.len() as u32,
                valid_probes: rebaked.valid as u32,
                rays_tested: reach.rays_tested as u32,
                rays_reaching: reach.rays_reaching as u32,
                rays_baked: rebaked.rays as u32,
                probe_bytes: uploaded.probe,
                validity_bytes: uploaded.validity,
                select_ns,
                bake_ns,
                upload_ns,
                update_ns,
            });
        }
    }

    let terms = ProbeTerms {
        select: term(&updates, |sample| sample.select_ns),
        rebake: term(&updates, |sample| sample.bake_ns),
        upload: term(&updates, |sample| sample.upload_ns),
        update: term(&updates, |sample| sample.update_ns),
    };
    let counts = ProbeCounts {
        affected: distribution(updates.iter().map(|sample| sample.affected_probes as u64)),
        rays_tested: distribution(updates.iter().map(|sample| sample.rays_tested as u64)),
        rays_reaching: distribution(updates.iter().map(|sample| sample.rays_reaching as u64)),
        rays_baked: distribution(updates.iter().map(|sample| sample.rays_baked as u64)),
        affected_total: updates
            .iter()
            .map(|sample| sample.affected_probes as u64)
            .sum(),
        affected_per_edit: mean(updates.iter().map(|sample| sample.affected_probes as u64)),
        rays_tested_per_edit: mean(updates.iter().map(|sample| sample.rays_tested as u64)),
        rays_reaching_per_edit: mean(updates.iter().map(|sample| sample.rays_reaching as u64)),
        rays_baked_total: updates.iter().map(|sample| sample.rays_baked as u64).sum(),
        rays_baked_per_edit: mean(updates.iter().map(|sample| sample.rays_baked as u64)),
        zero_affected_edits: updates
            .iter()
            .filter(|sample| sample.affected_probes == 0)
            .count() as u64,
        solid_to_air_edits: updates.iter().filter(|sample| sample.solid_to_air).count() as u64,
        air_to_solid_edits: updates.iter().filter(|sample| !sample.solid_to_air).count() as u64,
    };
    let bytes = ProbeBytes {
        probe_bytes: updates.iter().map(|sample| sample.probe_bytes).sum(),
        validity_bytes: updates.iter().map(|sample| sample.validity_bytes).sum(),
        upload_bytes: updates
            .iter()
            .map(|sample| sample.probe_bytes + sample.validity_bytes)
            .sum(),
        probe_bytes_per_edit: mean(updates.iter().map(|sample| sample.probe_bytes)),
        validity_bytes_per_edit: mean(updates.iter().map(|sample| sample.validity_bytes)),
        upload_bytes_per_edit: mean(
            updates
                .iter()
                .map(|sample| sample.probe_bytes + sample.validity_bytes),
        ),
    };
    let budget = BudgetCheck::new(BUDGET_NS, terms.update.p95_ns);

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
        "Update = select + rebake + upload. Every component is timed on its own and the \
         aggregate is the per-edit sum, so sub-timer overhead is charged to the aggregate and \
         the rows reconcile."
            .to_owned(),
        "Select tests valid probes ray by ray, stopping at the first ray that reaches the dirty \
         voxel, and traces only rays whose AABB intersects it. It uses the pre-edit DAG, so a ray \
         that terminates at the edited voxel is selected before a solid→air edit and a ray that \
         passes through it is selected before air→solid. A probe whose centre is the edited voxel \
         is selected even though it casts no ray while invalid. This is the M1 reference; \
         M2-18/M2-21 replace it with the production bake and budget check."
            .to_owned(),
        format!(
            "Rebake is the M1-30 reference shared with the test oracle: only affected probes are \
             recast, {PROBE_RAYS} rays each, then projected to SH4 RGB8. A full near-grid rebake \
             would be {} rays; the measured mean is {} rays/edit because the edit is bounded to \
             the probes its voxel can reach.",
            grouped((grid.len() * PROBE_RAYS) as u64),
            grouped(counts.rays_baked_per_edit),
        ),
        format!(
            "Upload stages exactly the changed probe records (12 B each) and one validity byte \
             per affected probe, so {} B/edit. A real device transfer and the M4 fence/flip are \
             excluded, matching the headless edit gate; M4-03/M4-05 replace the model.",
            bytes.upload_bytes_per_edit,
        ),
        format!(
            "Every candidate is a real fixture voxel that is the first solid on a probe ray, so \
             the edit can change at least one ray's radiance; edits are spread over probes and \
             rays and alternate solid→air ({} mine) and air→solid ({} place). No chunk is \
             generated, padded or carved. {} of {} measured edits selected zero probes after \
             other candidates in the chunk changed the ray; those are reported, not hidden.",
            grouped(counts.solid_to_air_edits),
            grouped(counts.air_to_solid_edits),
            grouped(counts.zero_affected_edits),
            grouped(updates.len() as u64),
        ),
        format!(
            "The fixture has {} live chunks; {} offer at least one real solid voxel on a probe ray \
             and are the edit targets. Chunks with no valid probe or no solid hit are not padded \
             with synthetic edits. The fixture remains provisional until the first real save.",
            grouped(fixture.chunks as u64),
            grouped(chunks_available as u64),
        ),
    ];

    Ok(ProbesReport {
        schema_version: SCHEMA_VERSION,
        fixture,
        edit_count: updates.len(),
        warmup_edits: WARMUP_EDITS,
        chunks_edited: edited_chunks.iter().filter(|&&edited| edited).count(),
        chunks_available,
        probes_per_chunk: grid.len(),
        rays_per_probe: PROBE_RAYS,
        terms,
        counts,
        bytes,
        budget,
        updates,
        notes,
    })
}

/// Collects up to [`CANDIDATES_PER_CHUNK`] real solid voxels that are the first
/// solid on a probe ray.
///
/// This is setup, not measured. The probe/ray order is deterministic and spread
/// so candidates do not all come from the first probe.
fn candidates(chunk_pos: ChunkPos, grid: ProbeGrid, dense: &[u16]) -> Vec<Candidate> {
    let mut out = Vec::with_capacity(CANDIDATES_PER_CHUNK);
    let mut seen = HashSet::with_capacity(CANDIDATES_PER_CHUNK);
    let scene_edge = grid.scene_edge_voxels();
    let solid_at = |x: u32, y: u32, z: u32| dense[local_index(x, y, z) as usize] != AIR;

    'search: for slot in 0..(grid.len() * PROBE_RAYS) {
        let probe = (slot * 37) % grid.len();
        let ray = (slot * 13 + slot / grid.len()) % PROBE_RAYS;
        let centre = grid.centre(probe);
        let centre_cell = [
            centre[0].floor() as u32,
            centre[1].floor() as u32,
            centre[2].floor() as u32,
        ];
        if solid_at(centre_cell[0], centre_cell[1], centre_cell[2]) {
            continue;
        }
        let seed = probe_seed(chunk_pos, grid.lod(), probe as u32);
        let direction = hemisphere_ray(ray, probe_rotation(seed));
        let Some([x, y, z]) = first_solid_on_ray(scene_edge, centre, direction, solid_at) else {
            continue;
        };
        let index = local_index(x, y, z);
        if !seen.insert(index) {
            continue;
        }
        out.push(Candidate {
            index,
            material: dense[index as usize],
        });
        if out.len() == CANDIDATES_PER_CHUNK {
            break 'search;
        }
    }

    out
}

fn distribution(values: impl IntoIterator<Item = u64>) -> Distribution {
    let mut values: Vec<u64> = values.into_iter().collect();
    Distribution::from_values(&mut values)
}

fn elapsed_ns(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patterns;
    use voxel_core::constants::BLOCK_DIM;

    #[test]
    fn candidates_are_real_solid_voxels_on_a_probe_ray() {
        let dense = patterns::terrain(0, 0);
        let grid = ProbeGrid::for_chunk(ProbeLod::Near);
        let candidates = candidates(ChunkPos::new(0, 0, 0), grid, &dense);

        assert!(!candidates.is_empty());
        assert!(candidates.len() <= CANDIDATES_PER_CHUNK);
        let mut seen = HashSet::new();
        for candidate in &candidates {
            assert!(seen.insert(candidate.index), "candidate indices are unique");
            let (x, y, z) = local_xyz(candidate.index);
            assert_ne!(
                dense[candidate.index as usize], AIR,
                "candidate is a real non-air voxel"
            );
            assert_eq!(
                dense[candidate.index as usize], candidate.material,
                "the stored material is the fixture value"
            );
            assert_eq!(candidate.index, local_index(x, y, z));
        }
    }

    #[test]
    fn a_first_solid_candidate_is_seen_by_at_least_one_probe() {
        let dense = patterns::terrain(1, -1);
        let grid = ProbeGrid::for_chunk(ProbeLod::Near);
        let chunk = ChunkPos::new(1, -1, 0);
        let candidates = candidates(chunk, grid, &dense);

        assert!(!candidates.is_empty());
        let mut affected = Vec::new();
        let mut visible = 0;
        for candidate in &candidates {
            let (x, y, z) = local_xyz(candidate.index);
            let reach = probes_reaching(
                chunk,
                grid,
                [x, y, z],
                |vx, vy, vz| dense[local_index(vx, vy, vz) as usize] != AIR,
                &mut affected,
            );
            if reach.affected > 0 {
                visible += 1;
            }
        }
        assert_eq!(
            visible,
            candidates.len(),
            "every first-solid candidate is selected by the same ray geometry it came from"
        );
    }

    #[test]
    fn a_fixture_update_equals_a_full_bake_in_both_directions() {
        let dense = patterns::terrain(0, 0);
        let grid = ProbeGrid::for_chunk(ProbeLod::Near);
        let chunk = ChunkPos::new(0, 0, 0);
        let candidates = candidates(chunk, grid, &dense);
        assert!(!candidates.is_empty());

        let mut dag = ChunkDag::from_dense(&dense);
        let mut volume = bake(chunk, grid, |x, y, z| dag.get(x, y, z) != AIR);
        let mut affected = Vec::new();

        for candidate in candidates.iter().take(4) {
            let (x, y, z) = local_xyz(candidate.index);

            let reach = probes_reaching(
                chunk,
                grid,
                [x, y, z],
                |vx, vy, vz| dag.get(vx, vy, vz) != AIR,
                &mut affected,
            );
            assert!(reach.affected > 0, "candidate is on a probe ray");
            dag.apply_edits([(candidate.index, AIR)]);
            {
                let dag = &dag;
                volume.rebake(chunk, &affected, |vx, vy, vz| dag.get(vx, vy, vz) != AIR);
            }
            let full = bake(chunk, grid, |vx, vy, vz| dag.get(vx, vy, vz) != AIR);
            assert_eq!(volume.sh_bytes(), full.sh_bytes(), "solid→air");
            assert_eq!(volume.validity_bytes(), full.validity_bytes(), "solid→air");

            let reach = probes_reaching(
                chunk,
                grid,
                [x, y, z],
                |vx, vy, vz| dag.get(vx, vy, vz) != AIR,
                &mut affected,
            );
            assert!(reach.affected > 0, "the restored voxel is on a probe ray");
            dag.apply_edits([(candidate.index, candidate.material)]);
            {
                let dag = &dag;
                volume.rebake(chunk, &affected, |vx, vy, vz| dag.get(vx, vy, vz) != AIR);
            }
            let full = bake(chunk, grid, |vx, vy, vz| dag.get(vx, vy, vz) != AIR);
            assert_eq!(volume.sh_bytes(), full.sh_bytes(), "air→solid");
            assert_eq!(volume.validity_bytes(), full.validity_bytes(), "air→solid");
        }
    }

    #[test]
    fn staging_copies_exactly_the_selected_records() {
        let dense = patterns::terrain(0, 0);
        let grid = ProbeGrid::for_chunk(ProbeLod::Near);
        let chunk = ChunkPos::new(0, 0, 0);
        let dag = ChunkDag::from_dense(&dense);
        let volume = bake(chunk, grid, |x, y, z| dag.get(x, y, z) != AIR);
        let affected = [0usize, 7, 63];
        let mut staging = ProbeStaging::new(grid.len());
        let uploaded = staging.upload(&volume, &affected);

        assert_eq!(uploaded.probe, affected.len() as u64 * PROBE_BYTES as u64);
        assert_eq!(uploaded.validity, affected.len() as u64);
        assert_eq!(uploaded.probe + uploaded.validity, 3 * 12 + 3);
        assert_eq!(staging.records.len(), affected.len() * PROBE_BYTES);
        assert_eq!(staging.validity.len(), affected.len());
    }

    #[test]
    fn the_near_grid_is_one_probe_per_block() {
        let edge = CHUNK_DIM / BLOCK_DIM;
        assert_eq!(ProbeGrid::for_chunk(ProbeLod::Near).len(), edge.pow(3));
        assert_eq!(
            edge,
            1usize << (CHUNK_BITS - 4),
            "near probes are one per block"
        );
    }
}
