//! The M1 gate report generator (M1-39).
//!
//! `gate report` assembles the four M1 harnesses into one decision table:
//!
//! | Harness | Row(s) |
//! | --- | --- |
//! | [`memory`] (M1-35) | VRAM projection, node budget, light, probes, mesh, dataset |
//! | [`residency`] (M1-36) | loaded chunk count |
//! | [`edits`] (M1-37) | edit p95 |
//! | [`probes`] (M1-38) | probe-update p95 |
//!
//! The rows are the twelve frozen rows in `docs/pre-registration.md` §3. Every
//! verdict is computed from a measured value and a frozen limit; no row is
//! typed `PASS` by hand. A row that the M1-35…M1-38 harnesses do not measure is
//! reported as `not_measured` with the evidence that still has to be attached,
//! not silently passed.
//!
//! # What is a gate run
//!
//! The report is generated on the reference machine named in
//! `docs/reference-machine.md`. The headless harness does not open a wgpu
//! adapter, so the GPU identity is quoted from that document and flagged as not
//! re-probed; the host CPU/OS/architecture are read from the running process.
//! The fixture is the frozen M1 placeholder until the first real save, so the
//! dataset row remains provisional and the whole report says so.
//!
//! # Memory projection
//!
//! The 40% VRAM target is 3,275 MiB and the 60% hard-fail line is 4,913 MiB
//! (the 1.5× safety factor). The gate compares the measured p95 per-chunk total
//! multiplied by the loaded chunk count at the edge this build actually uses
//! (N64 for the 64³ build, N128 for the 128³ build). It does not multiply the
//! hard-fail line again; the pre-registration already froze the 1.5× factor.
//!
//! # The rows that are not measured here
//!
//! - **Culling granularity** has the M1-34 instrumentation and its 100-pose
//!   test, but no archived save-level result is consumed by this generator.
//! - **Measurement dataset** is the synthetic fixture. It is reported as
//!   provisional until the first real save, as the pre-registration requires.
//!
//! Absent a measured failure, those two rows keep the overall gate `incomplete`,
//! not green. M1-41 attaches the real-save and granularity evidence before M1-42
//! records the decision.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use voxel_core::constants::{CHUNK_BITS, CHUNK_DIM, TARGET_CHUNK_BITS};
use voxel_core::content_constants::{
    LIGHT_BYTES_PER_CHUNK, LIGHT_CELLS_PER_CHUNK, PROBE_BYTES_PER_CHUNK, PROBES_PER_CHUNK,
};
use voxel_storage::reference::granularity::GRANULARITY_GATE_LIMIT;

use crate::proc;

use super::edits::{self, EditsReport};
use super::memory::{self, FixtureInfo, MemoryReport, NODE_BYTES};
use super::probes::{self, ProbesReport};
use super::residency::{self, Profile, ResidencyConfig, ResidencyReport};
use super::stat::{format_duration, format_signed_duration, grouped};

/// Schema version of the gate report itself.
pub const SCHEMA_VERSION: u32 = 1;

/// Number of pre-registered rows in `docs/pre-registration.md` §3.
pub const ROW_COUNT: usize = 12;

/// Total VRAM on the reference machine, in MiB.
pub const TOTAL_VRAM_MIB: u64 = 8_188;

/// Pre-registered voxel-data target, 40% of VRAM, in MiB.
pub const VOXEL_TARGET_MIB: u64 = 3_275;

/// Pre-registered hard-fail line, 60% of VRAM, in MiB.
pub const VOXEL_FAIL_MIB: u64 = 4_913;

/// Pre-registered safety factor on the voxel memory budget.
pub const SAFETY_FACTOR: f64 = 1.5;

/// SVDAG node cap at 64³ (`pre-registration.md` §3).
pub const NODE_BUDGET_64: u64 = 200_000;

/// SVDAG node cap at 128³ (`pre-registration.md` §3).
pub const NODE_BUDGET_128: u64 = 1_500_000;

/// Predicted lower bound for a 64³ mesh, in KiB.
pub const MESH_PREDICTED_MIN_KIB: u64 = 150;

/// Mesh cap at 64³, in KiB.
pub const MESH_BUDGET_64_KIB: u64 = 600;

/// Mesh cap at 128³, in KiB (2.4 MiB).
pub const MESH_BUDGET_128_KIB: u64 = 2_400;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

/// One of the twelve frozen pre-registration rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowId {
    /// 40% VRAM target and 60% hard-fail line.
    VramBudget,
    /// Loaded chunk count at the configured view/band.
    LoadedChunks,
    /// SVDAG unique-node cap per chunk.
    NodeBudget,
    /// Fixed block-light volume per chunk.
    LightVolume,
    /// Fixed near-LOD probe grid per chunk.
    ProbeGrid,
    /// Mesh buffer per chunk.
    MeshBuffer,
    /// Edit p95.
    EditLatency,
    /// Probe-update p95.
    ProbeUpdate,
    /// Traversed-but-invisible fraction at chunk boundaries.
    CullingGranularity,
    /// 1.5× safety factor on the memory budget.
    SafetyFactor,
    /// Real populated save with median/p95/max per chunk.
    MeasurementDataset,
    /// Pre-registered fallback order.
    FallbackTrigger,
}

impl RowId {
    /// Every row in pre-registration table order.
    pub const ALL: [Self; ROW_COUNT] = [
        Self::VramBudget,
        Self::LoadedChunks,
        Self::NodeBudget,
        Self::LightVolume,
        Self::ProbeGrid,
        Self::MeshBuffer,
        Self::EditLatency,
        Self::ProbeUpdate,
        Self::CullingGranularity,
        Self::SafetyFactor,
        Self::MeasurementDataset,
        Self::FallbackTrigger,
    ];

    /// One-based row number in the pre-registration table.
    pub const fn number(self) -> usize {
        match self {
            Self::VramBudget => 1,
            Self::LoadedChunks => 2,
            Self::NodeBudget => 3,
            Self::LightVolume => 4,
            Self::ProbeGrid => 5,
            Self::MeshBuffer => 6,
            Self::EditLatency => 7,
            Self::ProbeUpdate => 8,
            Self::CullingGranularity => 9,
            Self::SafetyFactor => 10,
            Self::MeasurementDataset => 11,
            Self::FallbackTrigger => 12,
        }
    }
}

/// Computed verdict for one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The measured value is inside the frozen limit.
    Pass,
    /// The measured value is outside the frozen limit.
    Fail,
    /// The required evidence is not part of this run.
    NotMeasured,
}

impl Verdict {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotMeasured => "NOT MEASURED",
        }
    }
}

/// Overall gate state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    /// All twelve rows passed.
    Pass,
    /// At least one row failed.
    Fail,
    /// No row failed, but at least one row has no admissible measurement.
    Incomplete,
}

impl GateStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

/// Machine-readable numbers behind one row.
///
/// The unit is part of the value so a consumer does not have to guess whether
/// `measured` is bytes, MiB, milliseconds or a count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateNumber {
    /// Measured value in `unit`.
    pub measured: f64,
    /// Frozen limit in `unit`.
    pub limit: f64,
    /// `limit - measured`; positive is headroom, negative is over budget.
    pub margin: f64,
    /// Unit name, e.g. `"MiB"`, `"nodes"`, `"ms"` or `"probes"`.
    pub unit: String,
}

/// One row of the generated gate report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateRow {
    /// Frozen row identity.
    pub id: RowId,
    /// Human-readable parameter name.
    pub parameter: String,
    /// Pre-registered value or rule.
    pub pre_registered: String,
    /// Measured value as displayed in the Markdown table.
    pub measured: String,
    /// Frozen limit as displayed in the Markdown table.
    pub limit: String,
    /// Signed margin as displayed in the Markdown table.
    pub margin: String,
    /// Computed verdict.
    pub verdict: Verdict,
    /// Harness or document that supplied the value.
    pub source: String,
    /// Why the row is pass/fail/not measured, and what it excludes.
    pub note: String,
    /// Optional numeric form for machine consumers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numbers: Option<GateNumber>,
}

/// The five free-text columns of a row, kept together so the constructor does
/// not take ten positional arguments.
struct RowText {
    pre_registered: String,
    measured: String,
    limit: String,
    margin: String,
    note: String,
}

impl RowText {
    fn new(
        pre_registered: impl Into<String>,
        measured: impl Into<String>,
        limit: impl Into<String>,
        margin: impl Into<String>,
        note: impl Into<String>,
    ) -> Self {
        Self {
            pre_registered: pre_registered.into(),
            measured: measured.into(),
            limit: limit.into(),
            margin: margin.into(),
            note: note.into(),
        }
    }
}

impl GateRow {
    fn new(
        id: RowId,
        parameter: impl Into<String>,
        text: RowText,
        verdict: Verdict,
        source: impl Into<String>,
        numbers: Option<GateNumber>,
    ) -> Self {
        Self {
            id,
            parameter: parameter.into(),
            pre_registered: text.pre_registered,
            measured: text.measured,
            limit: text.limit,
            margin: text.margin,
            verdict,
            source: source.into(),
            note: text.note,
            numbers,
        }
    }
}

/// Builds a row from the ten values the pre-registration table asks for.
macro_rules! gate_row {
    (
        $id:expr,
        $parameter:expr,
        $pre_registered:expr,
        $measured:expr,
        $limit:expr,
        $margin:expr,
        $verdict:expr,
        $source:expr,
        $note:expr,
        $numbers:expr $(,)?
    ) => {
        GateRow::new(
            $id,
            $parameter,
            RowText::new($pre_registered, $measured, $limit, $margin, $note),
            $verdict,
            $source,
            $numbers,
        )
    };
}

/// Counts behind the overall gate state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Number of rows.
    pub rows: usize,
    /// Rows whose measured value passed.
    pub pass: usize,
    /// Rows whose measured value failed.
    pub fail: usize,
    /// Rows without an admissible measurement.
    pub not_measured: usize,
    /// Overall status.
    pub status: GateStatus,
}

/// Pre-registered fallback selected by the measured rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fallback {
    /// True when a measured failure selects a pre-registration §4 rule.
    pub triggered: bool,
    /// §4 rule number (`1`, `2` or `3`), when triggered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<u8>,
    /// The action the frozen rule prescribes.
    pub action: String,
}

/// The reference machine named by `docs/reference-machine.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceMachine {
    /// Short name used in the report header.
    pub name: String,
    /// CPU model.
    pub cpu: String,
    /// GPU model.
    pub gpu: String,
    /// GPU memory in MiB.
    pub vram_mib: u64,
    /// System memory in GiB.
    pub ram_gib: u64,
    /// Display resolution.
    pub display: String,
    /// Primary OS.
    pub os: String,
    /// Source document.
    pub source: String,
}

impl ReferenceMachine {
    /// The frozen machine record.
    pub fn frozen() -> Self {
        Self {
            name: "RTX 4060 reference".to_owned(),
            cpu: "AMD Ryzen 5 7600, 6 cores / 12 threads".to_owned(),
            gpu: "NVIDIA GeForce RTX 4060".to_owned(),
            vram_mib: TOTAL_VRAM_MIB,
            ram_gib: 32,
            display: "1920 × 1080".to_owned(),
            os: "Linux (primary)".to_owned(),
            source: "docs/reference-machine.md".to_owned(),
        }
    }
}

/// The process that actually produced the run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// CPU model read from the running host, when the platform exposes it.
    pub cpu_model: Option<String>,
    /// Operating system from the standard library target.
    pub os: String,
    /// Architecture from the standard library target.
    pub arch: String,
}

/// Machine identity: the frozen reference plus the host that ran this report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// Frozen reference machine.
    pub reference: ReferenceMachine,
    /// Actual host.
    pub host: Host,
    /// True when the host CPU and OS match the reference record.
    pub host_is_reference: bool,
    /// False for the headless gate, which does not open a wgpu adapter.
    pub gpu_probed: bool,
}

impl Machine {
    fn current() -> Self {
        let cpu_model = proc::cpu_model();
        let os = std::env::consts::OS.to_owned();
        let host_is_reference = cpu_model
            .as_deref()
            .is_some_and(|cpu| cpu.contains("Ryzen 5 7600"))
            && os == "linux";
        Self {
            reference: ReferenceMachine::frozen(),
            host: Host {
                cpu_model,
                os,
                arch: std::env::consts::ARCH.to_owned(),
            },
            host_is_reference,
            gpu_probed: false,
        }
    }
}

/// The complete M1-39 gate report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Generation time.
    pub generated_unix: u64,
    /// Machine identity and host.
    pub machine: Machine,
    /// Gated profile.
    pub profile: String,
    /// True when `docs/reference-machine.md` gates this profile.
    pub profile_gated: bool,
    /// Fixture identity and provenance.
    pub fixture: FixtureInfo,
    /// The twelve rows, in pre-registration order.
    pub rows: Vec<GateRow>,
    /// Pass/fail counts and overall state.
    pub summary: Summary,
    /// Fallback selected by the measured rows.
    pub fallback: Fallback,
    /// Method and caveats a reader needs before quoting a row.
    pub notes: Vec<String>,
}

impl GateReport {
    /// Measures the default fixture and the Default profile.
    pub fn default_report() -> Result<Self, String> {
        Self::generate(&crate::fixture::default_manifest(), Profile::Default)
    }

    /// Measures `manifest` and assembles the rows for `profile`.
    pub fn generate(manifest: &Path, profile: Profile) -> Result<Self, String> {
        let memory = memory::report(manifest)?;
        let residency = residency::report(ResidencyConfig::from_profile(profile))?;
        let edits = edits::report(manifest)?;
        let probes = probes::report(manifest)?;
        Self::from_reports(&memory, &residency, &edits, &probes, profile)
    }

    /// Assembles the report from already-measured harnesses.
    ///
    /// This is the same path [`Self::generate`] uses, so a caller that already
    /// has the four reports gets the identical gate table without running the
    /// measurements again.
    pub fn from_reports(
        memory: &MemoryReport,
        residency: &ResidencyReport,
        edits: &EditsReport,
        probes: &ProbesReport,
        profile: Profile,
    ) -> Result<Self, String> {
        let machine = Machine::current();
        let mut rows = Vec::with_capacity(ROW_COUNT);
        rows.push(vram_row(memory, residency)?);
        rows.push(loaded_chunks_row(residency)?);
        rows.push(node_budget_row(memory)?);
        rows.push(light_row(memory)?);
        rows.push(probe_grid_row(memory, probes)?);
        rows.push(mesh_row(memory)?);
        rows.push(edit_latency_row(edits)?);
        rows.push(probe_update_row(probes)?);
        rows.push(granularity_row());
        rows.push(safety_factor_row());
        rows.push(dataset_row(memory));
        let fallback = fallback(&rows);
        rows.push(fallback_row(&fallback));
        debug_assert_eq!(rows.len(), ROW_COUNT);

        let summary = summarize(&rows);
        let notes = vec![
            "The twelve rows are the frozen table in docs/pre-registration.md §3. Every verdict is \
             computed from a measured value and its limit; no row is typed PASS. A row the M1-35…\
             M1-38 harnesses do not measure is NOT MEASURED, not green."
                .to_owned(),
            "The memory projection is the measured p95 per-chunk total multiplied by the loaded \
             chunk count at the edge this build uses. It applies the frozen 60% hard-fail line \
             (4,913 MiB); the 1.5× safety factor is already inside that line, so it is not applied \
             twice."
                .to_owned(),
            "The headless gate does not open a wgpu adapter. GPU identity is quoted from \
             docs/reference-machine.md and is not re-probed; the host CPU, OS and architecture are \
             read from the running process."
                .to_owned(),
            "The fixture is provisional until the first real save. Its distributions are a \
             rehearsal for the gate and must not be quoted as the M1 result; the dataset row stays \
             not measured until a real save replaces it."
                .to_owned(),
            "Culling granularity has the M1-34 reference and its 100-pose test, but this generator \
             consumes M1-35…M1-38 only. Attach a granularity run before M1-41 or that row keeps the \
             gate incomplete."
                .to_owned(),
            "The fallback is computed from the measured rows with the pre-registration §4 order. It \
             is a preview for M1-42, not a renegotiation of any threshold."
                .to_owned(),
        ];

        Ok(Self {
            schema_version: SCHEMA_VERSION,
            generated_unix: now_unix(),
            machine,
            profile: profile.name().to_owned(),
            profile_gated: profile.gated(),
            fixture: memory.fixture.clone(),
            rows,
            summary,
            fallback,
            notes,
        })
    }

    /// Renders the gate report as Markdown.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate report (M1-39)\n").unwrap();
        writeln!(
            out,
            "**Gate:** **{}** — {} pass, {} fail, {} not measured\n",
            self.summary.status.label(),
            self.summary.pass,
            self.summary.fail,
            self.summary.not_measured,
        )
        .unwrap();
        writeln!(
            out,
            "**Machine:** {} — {}, {}, {} MiB VRAM, {} GiB RAM, {}, {}\n",
            self.machine.reference.name,
            self.machine.reference.cpu,
            self.machine.reference.gpu,
            self.machine.reference.vram_mib,
            self.machine.reference.ram_gib,
            self.machine.reference.display,
            self.machine.reference.os,
        )
        .unwrap();
        writeln!(
            out,
            "**Host:** {} / {} / {} — reference host: {}; GPU probed: {}\n",
            self.machine
                .host
                .cpu_model
                .as_deref()
                .unwrap_or("unknown CPU"),
            self.machine.host.os,
            self.machine.host.arch,
            if self.machine.host_is_reference {
                "yes"
            } else {
                "no"
            },
            if self.machine.gpu_probed { "yes" } else { "no" },
        )
        .unwrap();
        writeln!(
            out,
            "**Profile:** `{}` ({})\n",
            self.profile,
            if self.profile_gated {
                "gated"
            } else {
                "not gated"
            },
        )
        .unwrap();
        writeln!(
            out,
            "**Fixture:** `{}` ({}) — provisional: {}\n",
            self.fixture.id,
            self.fixture.status,
            if self.fixture.provisional_until_first_real_save {
                "yes"
            } else {
                "no"
            },
        )
        .unwrap();
        writeln!(out, "**Generated:** unix {}\n", self.generated_unix).unwrap();

        writeln!(
            out,
            "| # | Pre-registered row | Requirement | Measured | Margin | Result |"
        )
        .unwrap();
        writeln!(out, "|---|---|---|---|---|---|").unwrap();
        for row in &self.rows {
            writeln!(
                out,
                "| {} | {} | {} | {} | {} | **{}** |",
                row.id.number(),
                row.parameter,
                row.limit,
                row.measured,
                row.margin,
                row.verdict.label(),
            )
            .unwrap();
        }

        writeln!(out, "\n## Row detail\n").unwrap();
        for row in &self.rows {
            writeln!(
                out,
                "{}. **{}** — pre-registered: {}. Source: {}. {}",
                row.id.number(),
                row.parameter,
                row.pre_registered,
                row.source,
                row.note,
            )
            .unwrap();
        }

        writeln!(out, "\n## Fallback\n").unwrap();
        writeln!(out, "{}\n", self.fallback.action).unwrap();

        writeln!(out, "## Notes\n").unwrap();
        for note in &self.notes {
            writeln!(out, "- {note}").unwrap();
        }
        out
    }
}

fn vram_row(memory: &MemoryReport, residency: &ResidencyReport) -> Result<GateRow, String> {
    let per_chunk = memory
        .components
        .total
        .p95
        .ok_or_else(|| "the fixture has no live chunk to project memory from".to_owned())?;
    let max_chunk = memory.components.total.max.unwrap_or(per_chunk);
    let loaded = loaded_chunks(residency);
    let projected = per_chunk
        .checked_mul(loaded)
        .ok_or_else(|| "projected voxel memory overflows u64".to_owned())?;
    let max_projected = max_chunk
        .checked_mul(loaded)
        .ok_or_else(|| "pessimistic voxel memory overflows u64".to_owned())?;
    let limit = VOXEL_FAIL_MIB * MIB;
    let target = VOXEL_TARGET_MIB * MIB;
    let verdict = at_most(projected, limit);
    let margin = limit as i64 - projected as i64;
    let target_margin = target as i64 - projected as i64;

    Ok(gate_row!(
        RowId::VramBudget,
        "VRAM budget for voxel data",
        "40% of 8,188 MiB = 3,275 MiB; hard fail at 60% = 4,913 MiB",
        format!(
            "{} (p95 {} × {} chunks)",
            human_bytes(projected),
            human_bytes(per_chunk),
            grouped(loaded),
        ),
        format!(
            "≤ {} hard fail ({} target)",
            human_bytes(limit),
            human_bytes(target),
        ),
        format!("{} to hard fail", signed_bytes(margin)),
        verdict,
        "gate memory (M1-35) + gate residency (M1-36)",
        format!(
            "Projected from the measured p95 total per chunk; the max chunk ({}) would project to \
             {}. The target row is {} ({}), the hard-fail row is {} ({}). The gate uses p95 by \
             pre-registration, not the mean, and does not apply the 1.5× factor a second time.",
            human_bytes(max_chunk),
            human_bytes(max_projected),
            human_bytes(target),
            signed_bytes(target_margin),
            human_bytes(limit),
            signed_bytes(margin),
        ),
        Some(GateNumber {
            measured: projected as f64 / MIB as f64,
            limit: VOXEL_FAIL_MIB as f64,
            margin: margin as f64 / MIB as f64,
            unit: "MiB".to_owned(),
        }),
    ))
}

fn loaded_chunks_row(residency: &ResidencyReport) -> Result<GateRow, String> {
    let n64 = residency.baseline.loaded_chunks;
    let n128 = residency.target.loaded_chunks;
    let Some(expected) = residency.pre_registered.as_ref() else {
        return Ok(gate_row!(
            RowId::LoadedChunks,
            "Target loaded chunk count",
            "Per profile and edge, table §2",
            format!(
                "N64 = {} at 64³; N128 = {} at 128³ equal volume",
                grouped(n64),
                grouped(n128),
            ),
            "no frozen row for this profile or customized view/band".to_owned(),
            "n/a".to_owned(),
            Verdict::NotMeasured,
            "gate residency (M1-36)",
            "The profile is not gated, or the view/band was customized, so the frozen §2 row does \
             not apply. The equal-volume identity still holds and is asserted by M1-36.",
            None,
        ));
    };

    let identity = &residency.equal_volume;
    let verdict = if identity.holds {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    let margin = identity.n64 as i64 - identity.factor as i64 * identity.n128 as i64;

    Ok(gate_row!(
        RowId::LoadedChunks,
        "Target loaded chunk count",
        format!(
            "Per profile and edge, table §2: N64 = {}, N128 = {}",
            grouped(expected.n64),
            grouped(expected.n128),
        ),
        format!(
            "N64 = {} at 64³; N128 = {} at 128³ equal volume",
            grouped(n64),
            grouped(n128),
        ),
        format!(
            "N64 = {} = {} × N128; table predicted N64 = {}, N128 = {}",
            grouped(n64),
            grouped(identity.factor),
            grouped(expected.n64),
            grouped(expected.n128),
        ),
        format!("{} chunks (equal-volume identity)", signed_count(margin)),
        verdict,
        "gate residency (M1-36)",
        format!(
            "The gate asserts the frozen model's equal-volume identity, N64 = {} × N128. The table's \
             own N64/N128 are a prediction: measured {} / {} against {} / {} (deltas {} / {}). Those \
             deltas are the table's nearest-integer rounding and are reported, not folded back into \
             the model. N64 feeds the memory projection.",
            grouped(identity.factor),
            grouped(n64),
            grouped(n128),
            grouped(expected.n64),
            grouped(expected.n128),
            signed_count(n64 as i64 - expected.n64 as i64),
            signed_count(n128 as i64 - expected.n128 as i64),
        ),
        Some(GateNumber {
            measured: n64 as f64,
            limit: n64 as f64,
            margin: margin as f64,
            unit: "chunks".to_owned(),
        }),
    ))
}

fn node_budget_row(memory: &MemoryReport) -> Result<GateRow, String> {
    let max_nodes = memory
        .chunks
        .iter()
        .map(|chunk| chunk.dag_node_bytes / NODE_BYTES)
        .max()
        .ok_or_else(|| "the fixture has no chunk for the node budget".to_owned())?;
    let p95_nodes = memory
        .components
        .dag
        .p95
        .map_or(0, |bytes| bytes / NODE_BYTES);
    let built_at_target = CHUNK_BITS == TARGET_CHUNK_BITS;
    let limit = if built_at_target {
        NODE_BUDGET_128
    } else {
        NODE_BUDGET_64
    };
    let edge = if built_at_target { "128³" } else { "64³" };
    let verdict = at_most(max_nodes, limit);
    let margin = limit as i64 - max_nodes as i64;

    Ok(gate_row!(
        RowId::NodeBudget,
        "SVDAG node budget/chunk",
        "64³ ≤ 200,000; 128³ ≤ 1,500,000 unique nodes",
        format!(
            "max {} nodes/chunk (p95 {}); {} build ({} voxels/edge)",
            grouped(max_nodes),
            grouped(p95_nodes),
            edge,
            CHUNK_DIM,
        ),
        format!("≤ {} nodes/chunk at {}", grouped(limit), edge),
        format!("{} nodes", signed_count(margin)),
        verdict,
        "gate memory (M1-35)",
        format!(
            "The node count is the banded internal-node arena (`dag_node_bytes / {NODE_BYTES}`), \
             so it is a hard per-chunk cap, not a p95. The other edge's budget is pre-registered but \
             not measured by this build; M1-40 supplies the 128³ projection before M1-42."
        ),
        Some(GateNumber {
            measured: max_nodes as f64,
            limit: limit as f64,
            margin: margin as f64,
            unit: "nodes".to_owned(),
        }),
    ))
}

fn light_row(memory: &MemoryReport) -> Result<GateRow, String> {
    let p50 = memory
        .components
        .light
        .p50
        .ok_or_else(|| "the fixture has no chunk for the light row".to_owned())?;
    let max = memory.components.light.max.unwrap_or(p50);
    let expected = LIGHT_BYTES_PER_CHUNK as u64;
    let cells = max / 16;
    let verdict = if p50 == expected && max == expected {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    let margin = -(expected as i64 - max as i64).abs();

    Ok(gate_row!(
        RowId::LightVolume,
        "Light volume/chunk",
        "64³: 64 cells = 1 KiB; 128³: 512 cells = 8 KiB",
        format!(
            "{} B/chunk ({} cells × 16 B); p50 {} B",
            grouped(max),
            grouped(cells),
            grouped(p50),
        ),
        format!(
            "= {} B/chunk at {}³ ({} cells)",
            grouped(expected),
            CHUNK_DIM,
            grouped(LIGHT_CELLS_PER_CHUNK as u64),
        ),
        format!("{} B", signed_count(margin)),
        verdict,
        "gate memory (M1-35); ADR-004",
        "One 16 B cell per block, fixed by the compiled chunk edge. The 128³ value is the frozen \
         pre-registration target and is not built in M1.",
        Some(GateNumber {
            measured: max as f64,
            limit: expected as f64,
            margin: margin as f64,
            unit: "bytes".to_owned(),
        }),
    ))
}

fn probe_grid_row(memory: &MemoryReport, probes: &ProbesReport) -> Result<GateRow, String> {
    let grid = probes.probes_per_chunk as u64;
    let bytes = memory
        .components
        .probes
        .max
        .ok_or_else(|| "the fixture has no chunk for the probe row".to_owned())?;
    let expected_grid = PROBES_PER_CHUNK as u64;
    let expected_bytes = PROBE_BYTES_PER_CHUNK as u64;
    let verdict = if grid == expected_grid && bytes == expected_bytes {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    let margin = -(expected_bytes as i64 - bytes as i64).abs();

    Ok(gate_row!(
        RowId::ProbeGrid,
        "Probe grid/chunk",
        "64³: 64 probes = 768 B; 128³: 512 probes = 6 KiB",
        format!(
            "{} probes/chunk, {} B; {} rays/full rebake",
            grouped(grid),
            grouped(bytes),
            grouped(grid * probes.rays_per_probe as u64),
        ),
        format!(
            "= {} probes = {} B/chunk at {}³",
            grouped(expected_grid),
            grouped(expected_bytes),
            CHUNK_DIM,
        ),
        format!("{} B", signed_count(margin)),
        verdict,
        "gate probes (M1-38) + gate memory (M1-35); ADR-005",
        "One 12 B SH4 RGB probe per block at the near LOD. The update row measures the latency; \
         this row checks both the fixed grid and the byte volume. The 128³ value is pre-registered, \
         not built in M1.",
        Some(GateNumber {
            measured: bytes as f64,
            limit: expected_bytes as f64,
            margin: margin as f64,
            unit: "bytes".to_owned(),
        }),
    ))
}

fn mesh_row(memory: &MemoryReport) -> Result<GateRow, String> {
    let p95 = memory
        .components
        .mesh
        .p95
        .ok_or_else(|| "the fixture has no chunk for the mesh row".to_owned())?;
    let max = memory.components.mesh.max.unwrap_or(p95);
    let built_at_target = CHUNK_BITS == TARGET_CHUNK_BITS;
    let limit_kib = if built_at_target {
        MESH_BUDGET_128_KIB
    } else {
        MESH_BUDGET_64_KIB
    };
    let limit = limit_kib * KIB;
    let verdict = at_most(p95, limit);
    let margin = limit as i64 - p95 as i64;

    Ok(gate_row!(
        RowId::MeshBuffer,
        "Mesh buffer/chunk",
        "64³: 150–600 KB; 128³: 600 KB–2.4 MB",
        format!(
            "p95 {} KiB (max {} KiB); greedy estimate × 76 B",
            p95 / KIB,
            max / KIB,
        ),
        format!("≤ {} KiB/chunk at {}³", limit_kib, CHUNK_DIM),
        signed_bytes(margin),
        verdict,
        "gate memory (M1-35); M1-31 reference mesher",
        format!(
            "The estimate is greedy quads × 76 B and excludes vertex/index buffer overhead. The \
             predicted lower bound is {} KiB; a lower estimate is not a failure, but it is not a \
             measured GPU buffer either. The gate uses p95 and reports max separately.",
            MESH_PREDICTED_MIN_KIB,
        ),
        Some(GateNumber {
            measured: p95 as f64 / KIB as f64,
            limit: limit_kib as f64,
            margin: margin as f64 / KIB as f64,
            unit: "KiB".to_owned(),
        }),
    ))
}

fn edit_latency_row(edits: &EditsReport) -> Result<GateRow, String> {
    let p95 = edits.terms.total.p95_ns;
    let limit = edits::BUDGET_NS;
    let margin = limit as i64 - p95 as i64;
    let verdict = at_most(p95, limit);

    Ok(gate_row!(
        RowId::EditLatency,
        "Edit latency",
        "p95 ≤ 8 ms (consolidate + upload + fence + flip + dirty light)",
        format!(
            "p95 {} (p50 {}, max {}); {} edits",
            format_duration(p95),
            format_duration(edits.terms.total.p50_ns),
            format_duration(edits.terms.total.max_ns),
            grouped(edits.edit_count as u64),
        ),
        format!("p95 ≤ {}", format_duration(limit)),
        format_signed_duration(margin),
        verdict,
        "gate edits (M1-37)",
        "Fixed = upload + fence + flip; dirty = COW + light. The fence and upload are the CPU side \
         of the M4 protocol; the headless gate has no GPU device time, so this is a lower bound on \
         a real device path, not a claim about GPU completion.",
        Some(GateNumber {
            measured: p95 as f64 / 1_000_000.0,
            limit: limit as f64 / 1_000_000.0,
            margin: margin as f64 / 1_000_000.0,
            unit: "ms".to_owned(),
        }),
    ))
}

fn probe_update_row(probes: &ProbesReport) -> Result<GateRow, String> {
    let p95 = probes.terms.update.p95_ns;
    let limit = probes::BUDGET_NS;
    let margin = limit as i64 - p95 as i64;
    let verdict = at_most(p95, limit);

    Ok(gate_row!(
        RowId::ProbeUpdate,
        "Probe-update budget",
        "p95 ≤ 4 ms per edited chunk",
        format!(
            "p95 {} (p50 {}, max {}); {} edited chunks",
            format_duration(p95),
            format_duration(probes.terms.update.p50_ns),
            format_duration(probes.terms.update.max_ns),
            grouped(probes.edit_count as u64),
        ),
        format!("p95 ≤ {} per edited chunk", format_duration(limit)),
        format_signed_duration(margin),
        verdict,
        "gate probes (M1-38)",
        "Update = select + rebake + upload. Only probes whose rays reach the dirty voxel are \
         rebaked; the upload is the CPU staging copy and excludes the device transfer and the M4 \
         fence/flip.",
        Some(GateNumber {
            measured: p95 as f64 / 1_000_000.0,
            limit: limit as f64 / 1_000_000.0,
            margin: margin as f64 / 1_000_000.0,
            unit: "ms".to_owned(),
        }),
    ))
}

fn granularity_row() -> GateRow {
    gate_row!(
        RowId::CullingGranularity,
        "Culling granularity",
        "≤ 15% traversed-but-invisible at chunk boundaries @ max view",
        "not measured by this run".to_owned(),
        format!(
            "≤ {:.0}% traversed-but-invisible",
            GRANULARITY_GATE_LIMIT * 100.0
        ),
        "n/a".to_owned(),
        Verdict::NotMeasured,
        "M1-34 reference; no archived gate result",
        "The reference instrumentation and its 100-pose test exist, but this generator consumes \
         M1-35…M1-38 only. A granularity run must be attached before M1-41, or this row keeps the \
         gate incomplete; it is not silently passed.",
        None,
    )
}

fn safety_factor_row() -> GateRow {
    let expected = (VOXEL_TARGET_MIB as f64 * SAFETY_FACTOR).ceil() as u64;
    let verdict = exactly(expected, VOXEL_FAIL_MIB);
    let margin = -(VOXEL_FAIL_MIB as i64 - expected as i64).abs();

    gate_row!(
        RowId::SafetyFactor,
        "Safety factor",
        "1.5× on the memory budget",
        format!(
            "{} MiB = ceil(1.5 × {} MiB)",
            grouped(VOXEL_FAIL_MIB),
            grouped(VOXEL_TARGET_MIB),
        ),
        format!("hard fail = {} MiB", grouped(VOXEL_FAIL_MIB)),
        format!("{} MiB", signed_count(margin)),
        verdict,
        "pre-registration §3; performance-targets §1.1",
        "This is an invariant on the frozen budget, not a second measurement. The factor survives \
         hardware changes because the fraction (40%/60%) does; applying it twice would make the \
         gate stricter than the pre-registration.",
        Some(GateNumber {
            measured: VOXEL_FAIL_MIB as f64,
            limit: expected as f64,
            margin: margin as f64,
            unit: "MiB".to_owned(),
        }),
    )
}

fn dataset_row(memory: &MemoryReport) -> GateRow {
    let provisional = memory.is_provisional();
    let verdict = if provisional {
        Verdict::NotMeasured
    } else {
        Verdict::Pass
    };
    let measured = if provisional {
        format!(
            "{} ({}; synthetic, provisional)",
            memory.fixture.id, memory.fixture.status
        )
    } else {
        format!("{} (real populated save)", memory.fixture.id)
    };
    let note = if provisional {
        format!(
            "{} The distributions are measured (p50/p95/max per chunk), but the dataset is not \
             admissible as the M1 gate result until a real populated save replaces it.",
            memory.fixture.limitation,
        )
    } else {
        "The measured dataset is a real populated save and carries the per-chunk median, p95 and \
         max the row requires."
            .to_owned()
    };

    gate_row!(
        RowId::MeasurementDataset,
        "Measurement dataset",
        "Real populated save; median, p95, max per chunk",
        measured,
        "real populated save".to_owned(),
        if provisional {
            "n/a"
        } else {
            "p50/p95/max present"
        }
        .to_owned(),
        verdict,
        "gate memory fixture (M1-28/M1-35)",
        note,
        None,
    )
}

fn fallback_row(fallback: &Fallback) -> GateRow {
    gate_row!(
        RowId::FallbackTrigger,
        "Fallback trigger",
        "See §4; pre-registered; no renegotiation inside M1",
        if fallback.triggered {
            format!(
                "rule {} triggered by the measured rows",
                fallback.rule.unwrap_or(0)
            )
        } else {
            "not triggered by the measured rows".to_owned()
        },
        "pre-registration §4 fallback order".to_owned(),
        "n/a".to_owned(),
        Verdict::Pass,
        "pre-registration §4",
        fallback.action.clone(),
        None,
    )
}

fn fallback(rows: &[GateRow]) -> Fallback {
    let failed = |id: RowId| {
        rows.iter()
            .any(|row| row.id == id && row.verdict == Verdict::Fail)
    };

    if failed(RowId::EditLatency) || failed(RowId::ProbeUpdate) || failed(RowId::CullingGranularity)
    {
        return Fallback {
            triggered: true,
            rule: Some(1),
            action:
                "Rule 1: edit latency, probe update or granularity failed at 64³. 128³ is dead; \
                     stay at 64³ and record the measured numbers. Do not renegotiate the row."
                    .to_owned(),
        };
    }
    if failed(RowId::VramBudget)
        || failed(RowId::NodeBudget)
        || failed(RowId::LightVolume)
        || failed(RowId::ProbeGrid)
        || failed(RowId::MeshBuffer)
    {
        return Fallback {
            triggered: true,
            rule: Some(2),
            action:
                "Rule 2: a voxel-memory row failed at 64³. Do not automatically stay at 64³. Run \
                     the M1-40 analytic 128³ projection; if 128³ clears, the failure is an argument \
                     for the upgrade, not against it."
                    .to_owned(),
        };
    }

    Fallback {
        triggered: false,
        rule: None,
        action: "No measured row triggers a §4 fallback. The 128³ projection (M1-40), the \
                 granularity row and the real-save dataset remain outstanding before the M1-42 \
                 decision. If the projection fails at 128³, rule 3 applies: mesh compression, then \
                 near radius, then vertical shell; never shrink the far clip below 8 km."
            .to_owned(),
    }
}

fn summarize(rows: &[GateRow]) -> Summary {
    let pass = rows
        .iter()
        .filter(|row| row.verdict == Verdict::Pass)
        .count();
    let fail = rows
        .iter()
        .filter(|row| row.verdict == Verdict::Fail)
        .count();
    let not_measured = rows
        .iter()
        .filter(|row| row.verdict == Verdict::NotMeasured)
        .count();
    let status = if fail > 0 {
        GateStatus::Fail
    } else if not_measured > 0 {
        GateStatus::Incomplete
    } else {
        GateStatus::Pass
    };
    Summary {
        rows: rows.len(),
        pass,
        fail,
        not_measured,
        status,
    }
}

fn loaded_chunks(residency: &ResidencyReport) -> u64 {
    if CHUNK_BITS == TARGET_CHUNK_BITS {
        residency.target.loaded_chunks
    } else {
        residency.baseline.loaded_chunks
    }
}

fn at_most(measured: u64, limit: u64) -> Verdict {
    if measured <= limit {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

fn exactly(measured: u64, limit: u64) -> Verdict {
    if measured == limit {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn human_bytes(bytes: u64) -> String {
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

fn signed_bytes(value: i64) -> String {
    if value < 0 {
        format!("-{}", human_bytes(value.unsigned_abs()))
    } else {
        format!("+{}", human_bytes(value as u64))
    }
}

fn signed_count(value: i64) -> String {
    if value > 0 {
        format!("+{}", grouped(value as u64))
    } else if value < 0 {
        format!("-{}", grouped(value.unsigned_abs()))
    } else {
        "0".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twelve_rows_are_defined_once_in_table_order() {
        assert_eq!(RowId::ALL.len(), ROW_COUNT);
        let mut numbers: Vec<usize> = RowId::ALL.iter().map(|id| id.number()).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=ROW_COUNT).collect::<Vec<_>>());
    }

    #[test]
    fn verdicts_are_computed_from_the_numbers_not_typed() {
        assert_eq!(at_most(8, 8), Verdict::Pass);
        assert_eq!(at_most(7, 8), Verdict::Pass);
        assert_eq!(at_most(9, 8), Verdict::Fail);
        assert_eq!(exactly(1_024, 1_024), Verdict::Pass);
        assert_eq!(exactly(1_025, 1_024), Verdict::Fail);
    }

    #[test]
    fn the_safety_factor_is_the_frozen_hard_fail_line() {
        assert_eq!(
            (VOXEL_TARGET_MIB as f64 * SAFETY_FACTOR).ceil() as u64,
            VOXEL_FAIL_MIB
        );
    }

    #[test]
    fn the_fallback_follows_the_failed_lens() {
        let memory = vec![gate_row!(
            RowId::MeshBuffer,
            "x",
            "x",
            "x",
            "x",
            "x",
            Verdict::Fail,
            "x",
            "x",
            None
        )];
        let selected = fallback(&memory);
        assert!(selected.triggered);
        assert_eq!(selected.rule, Some(2));

        let latency = vec![gate_row!(
            RowId::ProbeUpdate,
            "x",
            "x",
            "x",
            "x",
            "x",
            Verdict::Fail,
            "x",
            "x",
            None
        )];
        let selected = fallback(&latency);
        assert!(selected.triggered);
        assert_eq!(selected.rule, Some(1));
    }

    #[test]
    fn a_failure_makes_the_gate_fail_and_a_gap_makes_it_incomplete() {
        let failed = vec![gate_row!(
            RowId::EditLatency,
            "x",
            "x",
            "x",
            "x",
            "x",
            Verdict::Fail,
            "x",
            "x",
            None,
        )];
        assert_eq!(summarize(&failed).status, GateStatus::Fail);

        let gap = vec![gate_row!(
            RowId::CullingGranularity,
            "x",
            "x",
            "x",
            "x",
            "x",
            Verdict::NotMeasured,
            "x",
            "x",
            None,
        )];
        assert_eq!(summarize(&gap).status, GateStatus::Incomplete);
    }
}
