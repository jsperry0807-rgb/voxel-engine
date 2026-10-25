//! Result types and the Markdown/JSON report.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// One memory measurement.
#[derive(Clone, Debug, Serialize)]
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
#[derive(Clone, Debug, Serialize)]
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
#[derive(Clone, Debug, Serialize)]
pub struct LatencyRecord {
    pub case: String,
    pub iters: u64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub max_us: f64,
    pub note: String,
}

/// Process-level memory and CPU readings for the whole run.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ProcessReport {
    pub rss_start: Option<u64>,
    pub rss_end: Option<u64>,
    pub peak_rss: Option<u64>,
    pub wall_ns: u64,
    pub cpu_ns: Option<u64>,
    pub cpu_percent: Option<f64>,
}

/// Run metadata.
#[derive(Clone, Debug, Serialize)]
pub struct Meta {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub cpu_model: Option<String>,
    pub timestamp_unix: u64,
    pub note: String,
}

/// Everything the software lenses produced.
#[derive(Debug, Serialize)]
pub struct Report {
    pub meta: Meta,
    pub process: ProcessReport,
    pub memory: Vec<MemoryRecord>,
    pub cpu: Vec<CpuRecord>,
    pub latency: Vec<LatencyRecord>,
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
