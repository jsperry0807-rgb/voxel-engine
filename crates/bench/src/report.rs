//! Result types and the Markdown/JSON report.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// One memory measurement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub case: String,
    /// Bytes the result keeps alive.
    pub resident: u64,
    /// Bytes allocated over the workload, scratch included.
    pub allocated: u64,
    /// Allocation calls over the workload.
    pub allocs: u64,
    /// High-water mark above the pre-workload baseline.
    pub peak: u64,
    pub note: String,
}

/// One CPU/throughput measurement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CpuRecord {
    pub case: String,
    pub iters: u64,
    pub wall_ns: u64,
    pub cpu_ns: Option<u64>,
    pub ns_per_op: f64,
    pub mops_per_sec: f64,
    pub cpu_percent: Option<f64>,
    pub allocated: u64,
    pub allocs: u64,
    pub peak: u64,
    pub bytes_per_op: f64,
    pub allocs_per_op: f64,
    pub note: String,
}

/// One latency distribution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LatencyRecord {
    pub case: String,
    pub iters: u64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub note: String,
}

/// One COW batch measured on a fixture (M1-18).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CowRecord {
    /// Distinct edited voxel paths in the batch.
    pub paths: u64,
    /// Internal sibling runs appended.
    pub runs: u64,
    /// Leaf runs appended.
    pub leaf_runs: u64,
    /// Internal node bytes appended, including full-run alignment padding.
    pub node_bytes: u64,
    /// Leaf material bytes appended.
    pub leaf_bytes: u64,
    /// Total bytes appended, internal plus leaf.
    pub total_bytes: u64,
    /// Internal bytes per edited path.
    pub node_bytes_per_path: f64,
    /// Total bytes per edited path, including leaf materials.
    pub bytes_per_path: f64,
}

/// Storage cost and dedup measurements for one fixture (M1-18).
///
/// Sizes include the banded chunk's 8 B internal words and full-run padding.
/// `old_*` describe the superseded v1 `u32` pool, which dedups individual
/// nodes; the banded layout dedups whole sibling runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutRecord {
    /// Fixture name, shared with the memory chunks.
    pub fixture: String,
    /// Internal arena bytes, including full-run alignment padding.
    pub internal_bytes: u64,
    /// Reachable internal nodes (deduplicated).
    pub nodes: u64,
    /// Alignment padding in the internal arena.
    pub padding_bytes: u64,
    /// Internal bytes per reachable node, padding included.
    pub bytes_per_node: f64,
    /// Padding as a fraction of `internal_bytes`.
    pub padding_ratio: f64,
    /// Distinct live internal sibling runs.
    pub internal_runs: u64,
    /// Distinct live leaf runs.
    pub leaf_runs: u64,
    /// Live runs referenced by more than one parent.
    pub shared_runs: u64,
    /// Internal nodes if every sibling position were stored separately.
    pub expanded_nodes: u64,
    /// `expanded_nodes / nodes`: dedup achieved by run sharing.
    pub dedup_new: f64,
    /// `expanded_nodes / old_nodes`: dedup achieved by per-node sharing.
    pub dedup_old: f64,
    /// Unique internal nodes in the old `u32` pool.
    pub old_nodes: u64,
    /// Serialized old-encoding bytes (8 B header + 4 B words).
    pub old_bytes: u64,
    /// Serialized banded bytes (header + internal arena + leaf arena).
    pub new_bytes: u64,
    /// `old_bytes / new_bytes`; above 1 means the banded layout is smaller.
    pub size_ratio: f64,
    /// COW costs for the measured edit batches.
    pub cow: Vec<CowRecord>,
}

/// Process-level memory and CPU readings for the whole run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProcessReport {
    pub rss_start: Option<u64>,
    pub rss_end: Option<u64>,
    pub peak_rss: Option<u64>,
    pub wall_ns: u64,
    pub cpu_ns: Option<u64>,
    pub cpu_percent: Option<f64>,
}

/// Run metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub cpu_model: Option<String>,
    pub timestamp_unix: u64,
    pub note: String,
}

/// Everything the software lenses produced.
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub meta: Meta,
    pub process: ProcessReport,
    pub memory: Vec<MemoryRecord>,
    pub cpu: Vec<CpuRecord>,
    pub latency: Vec<LatencyRecord>,
    /// Layout cost report (M1-18). Defaulted so archived reports written before
    /// the section existed still deserialize.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout: Vec<LayoutRecord>,
}

impl Report {
    /// Writes `report.json` and `report.md` into `dir`.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(dir.join("report.json"), json)?;
        std::fs::write(dir.join("report.md"), self.markdown())?;
        Ok(())
    }

    /// Renders the human-readable report.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Voxel benchmark report\n");
        let _ = writeln!(
            out,
            "`{}` on {} / {}{}. Measured {}.\n",
            self.meta.version,
            self.meta.os,
            self.meta.arch,
            self.meta
                .cpu_model
                .as_deref()
                .map(|cpu| format!(" ({cpu})"))
                .unwrap_or_default(),
            self.meta.note,
        );

        out.push_str("## Process\n\n");
        out.push_str("| Metric | Value |\n|---|---:|\n");
        let _ = writeln!(out, "| Wall time | {} |", duration(self.process.wall_ns));
        let _ = writeln!(
            out,
            "| CPU time | {} |",
            self.process
                .cpu_ns
                .map(duration)
                .unwrap_or_else(|| "n/a".into()),
        );
        let _ = writeln!(
            out,
            "| CPU utilization | {} |",
            percent(self.process.cpu_percent),
        );
        let _ = writeln!(
            out,
            "| RSS at start | {} |",
            opt_bytes(self.process.rss_start)
        );
        let _ = writeln!(out, "| RSS at end | {} |", opt_bytes(self.process.rss_end));
        let _ = writeln!(out, "| Peak RSS | {} |", opt_bytes(self.process.peak_rss));
        out.push('\n');

        if !self.memory.is_empty() {
            out.push_str("## Memory\n\n");
            out.push_str("| Case | Resident | Peak | Allocated | Allocs | Note |\n");
            out.push_str("|---|---:|---:|---:|---:|---|\n");
            for m in &self.memory {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {} | {} | {} | {} |",
                    m.case,
                    bytes(m.resident),
                    bytes(m.peak),
                    bytes(m.allocated),
                    m.allocs,
                    m.note,
                );
            }
            out.push('\n');
        }

        if !self.layout.is_empty() {
            out.push_str("## Layout cost (M1-18)\n\n");
            out.push_str(
                "Internal bytes are the banded arena's 8 B words plus the padding that aligns \
                 full runs to 64 B. Dedup is the expanded node count divided by the unique \
                 node count; `old` is the superseded per-node `u32` pool and `new` is the \
                 banded run layout. `Old/new` above 1 means the banded serialization is \
                 smaller. `B/node` and every byte figure include padding.\n\n",
            );
            out.push_str(
                "| Fixture | Internal bytes | B/node incl. pad | Padding | Internal runs | Leaf runs | Shared runs | Dedup new | Dedup old | Old bytes | New bytes | Old/new |\n\
                 |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n",
            );
            for l in &self.layout {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {:.1} | {} ({:.1}%) | {} | {} | {} | {:.1}x | {:.1}x | {} | {} | {:.2}x |",
                    l.fixture,
                    bytes(l.internal_bytes),
                    l.bytes_per_node,
                    bytes(l.padding_bytes),
                    l.padding_ratio * 100.0,
                    l.internal_runs,
                    l.leaf_runs,
                    l.shared_runs,
                    l.dedup_new,
                    l.dedup_old,
                    bytes(l.old_bytes),
                    bytes(l.new_bytes),
                    l.size_ratio,
                );
            }
            out.push('\n');
            out.push_str(
                "### COW cost\n\n\
                 A batch appends one internal run per touched level and one leaf run per \
                 touched level-1 parent. `Internal B/path` includes full-run padding; \
                 `Total B/path` also counts leaf materials.\n\n",
            );
            out.push_str(
                "| Fixture | Paths | Runs | Leaf runs | Internal B/path | Total B/path |\n\
                 |---|---:|---:|---:|---:|---:|\n",
            );
            for l in &self.layout {
                for cow in &l.cow {
                    let _ = writeln!(
                        out,
                        "| `{}` | {} | {} | {} | {:.1} | {:.1} |",
                        l.fixture,
                        cow.paths,
                        cow.runs,
                        cow.leaf_runs,
                        cow.node_bytes_per_path,
                        cow.bytes_per_path,
                    );
                }
            }
            out.push('\n');
        }

        if !self.cpu.is_empty() {
            out.push_str("## CPU and throughput\n\n");
            out.push_str(
                "| Case | ns/op | Rate | CPU % | Allocs/op | Bytes/op | Peak heap | Note |\n",
            );
            out.push_str("|---|---:|---:|---:|---:|---:|---:|---|\n");
            for c in &self.cpu {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {} | {} | {:.2} | {} | {} | {} |",
                    c.case,
                    ns(c.ns_per_op),
                    rate(c.mops_per_sec * 1e6),
                    percent(c.cpu_percent),
                    c.allocs_per_op,
                    bytes(c.bytes_per_op as u64),
                    bytes(c.peak),
                    c.note,
                );
            }
            out.push('\n');
        }

        if !self.latency.is_empty() {
            out.push_str("## Latency\n\n");
            out.push_str("| Case | p50 | p95 | p99 | max | Note |\n");
            out.push_str("|---|---:|---:|---:|---:|---|\n");
            for l in &self.latency {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {} | {} | {} | {} |",
                    l.case,
                    micros(l.p50_us),
                    micros(l.p95_us),
                    micros(l.p99_us),
                    micros(l.max_us),
                    l.note,
                );
            }
            out.push('\n');
        }

        out.push_str("## Other lenses\n\n");
        out.push_str(
            "These software readings are one half of the picture. Run the rest with:\n\n\
             ```sh\n\
             scripts/bench.sh          # all of the below, with graceful skips\n\
             cargo bench --workspace   # Criterion wall clock and throughput\n\
             cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches\n\
             perf stat -e cycles,instructions,cache-misses,branch-misses \\\n\
             \x20 target/release/voxel-bench cpu                  # hardware counters\n\
             ```\n",
        );
        out
    }
}

/// Builds the report metadata for this build.
pub fn meta() -> Meta {
    Meta {
        version: format!("voxel-bench {}", env!("CARGO_PKG_VERSION")),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        cpu_model: crate::proc::cpu_model(),
        timestamp_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        note: "heap bytes come from the counting allocator; RSS is process-wide".to_owned(),
    }
}

pub(crate) fn bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let n = n as f64;
    if n >= GIB {
        format!("{:.2} GiB", n / GIB)
    } else if n >= MIB {
        format!("{:.2} MiB", n / MIB)
    } else if n >= KIB {
        format!("{:.1} KiB", n / KIB)
    } else {
        format!("{n:.0} B")
    }
}

fn opt_bytes(value: Option<u64>) -> String {
    value.map(bytes).unwrap_or_else(|| "n/a".into())
}

fn rate(ops_per_sec: f64) -> String {
    if ops_per_sec >= 1e9 {
        format!("{:.2} Gops/s", ops_per_sec / 1e9)
    } else if ops_per_sec >= 1e6 {
        format!("{:.2} Mops/s", ops_per_sec / 1e6)
    } else if ops_per_sec >= 1e3 {
        format!("{:.2} Kops/s", ops_per_sec / 1e3)
    } else {
        format!("{ops_per_sec:.2} ops/s")
    }
}

fn ns(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("{:.2} ms", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.2} µs", value / 1_000.0)
    } else {
        format!("{value:.1} ns")
    }
}

fn micros(value: f64) -> String {
    if value >= 1_000.0 {
        format!("{:.2} ms", value / 1_000.0)
    } else {
        format!("{value:.2} µs")
    }
}

fn duration(value: u64) -> String {
    ns(value as f64)
}

fn percent(value: Option<f64>) -> String {
    value
        .map(|p| format!("{p:.1}%"))
        .unwrap_or_else(|| "n/a".into())
}
