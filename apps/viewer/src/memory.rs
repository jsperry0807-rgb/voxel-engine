//! M3-11: the 256-chunk scene's memory report.
//!
//! [`fill_scene`] builds the real streaming scene headlessly, fills the ring and
//! returns the bytes it holds. [`Report`] turns that into the five rows the
//! milestone asks for — DAG, mesh, overlay, light, probes — plus the host side,
//! and checks the VRAM budget with its 1.5× margin.
//!
//! **What the rows mean in M3.** The viewer generates a dense volume per chunk
//! on the CPU, meshes it and uploads only the mesh plus the light, probe and
//! origin buffers. No SVDAG and no edit overlay is uploaded, so those two rows
//! are **zero by construction**, not unmeasured — the storage model's own DAG
//! and overlay are covered by the M1 gate (`gate memory`), and they will move
//! into VRAM with M4's versioned resources. The report says this in its notes so
//! a zero row cannot be mistaken for a missing measurement.
//!
//! **Host.** The streaming scene owns the workers' dense volume buffers and the
//! finished uploads waiting for a frame budget. Everything else (the atlas, the
//! depth target, the staging belt, the driver's own copies) is viewer overhead
//! and is reported separately by the caller.

use std::time::{Duration, Instant};

use wgpu::util::StagingBelt;
use wgpu::{Device, PollType, Queue};

use crate::camera::Camera;
use crate::chunk_lighting;
use crate::scene::Scene;
use crate::streaming::{self, MemoryUsage, PUMP_BUDGET, RING_CHUNKS, Streaming};
use crate::upload;

/// The 40% VRAM target on an 8 GiB card: 3.20 GiB.
pub const VRAM_TARGET_BYTES: u64 = 3_200 * 1024 * 1024;
/// The hard-fail line from the M1 gate: 4.80 GiB.
pub const VRAM_HARD_FAIL_BYTES: u64 = 4_800 * 1024 * 1024;
/// The margin the report must keep: usage × 1.5 still inside the target.
pub const MARGIN: f64 = 1.5;

/// Longest a headless fill may take before it reports an error.
const FILL_TIMEOUT: Duration = Duration::from_secs(180);

/// Fills the 256-chunk ring headlessly and returns what it holds.
///
/// No surface and no draw: this measures the streaming scene's allocations
/// (mesh pool pages, light/probe/origin buffers) and its host buffers, which is
/// exactly the M3-11 row set.
pub fn fill_scene(
    device: &Device,
    queue: &Queue,
    scene: Scene,
    upload_budget: usize,
) -> Result<(Streaming, MemoryUsage), String> {
    let layout = chunk_lighting::layout(device);
    let mut streaming = Streaming::new(device.clone(), layout, upload_budget, scene);
    let mut belt = StagingBelt::new(device.clone(), upload::STAGING_CHUNK_BYTES);
    let center = streaming::center_for(Camera::default().uniform().position);
    let deadline = Instant::now() + FILL_TIMEOUT;

    while streaming.loaded() < RING_CHUNKS {
        if Instant::now() > deadline {
            return Err(format!(
                "the ring did not fill: {}/{} chunks resident",
                streaming.loaded(),
                RING_CHUNKS
            ));
        }
        streaming.pump(center, PUMP_BUDGET);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("memory fill"),
        });
        streaming.install_frame(&mut encoder, &mut belt, upload_budget);
        belt.finish_and_recall_on_submit(&encoder);
        queue.submit([encoder.finish()]);
        let _ = device.poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        // The workers need wall time; a tight loop would spin on an empty
        // channel and starve them of the cores they need.
        std::thread::sleep(Duration::from_millis(2));
    }

    let usage = streaming.memory_usage();
    Ok((streaming, usage))
}

/// One row of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Row name.
    pub name: &'static str,
    /// VRAM bytes.
    pub gpu: u64,
    /// Host bytes.
    pub host: u64,
    /// Why the row is zero, when it is.
    pub note: &'static str,
}

/// The M3-11 report.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Resident chunks, seed, chunk edge and upload budget.
    pub chunks: usize,
    /// Scene seed.
    pub seed: u32,
    /// Chunk edge in voxels.
    pub chunk_dim: u32,
    /// Per-frame upload budget.
    pub upload_budget: usize,
    /// The five required rows plus origins.
    pub rows: Vec<Row>,
    /// Pool pages and their slack.
    pub pool_pages: usize,
    /// Bytes per pool page.
    pub pool_page_bytes: u64,
    /// Bytes inside live mesh allocations.
    pub pool_live: u64,
    /// Free bytes inside the committed pages.
    pub pool_slack: u64,
    /// Process heap live after the fill.
    pub heap_live: Option<u64>,
    /// Peak heap above the pre-fill baseline.
    pub heap_peak: Option<u64>,
    /// Process RSS after the fill.
    pub rss: Option<u64>,
    /// Peak process RSS.
    pub peak_rss: Option<u64>,
}

impl Report {
    /// Builds the report from a filled scene's usage.
    pub fn new(usage: MemoryUsage, seed: u32, chunk_dim: u32, upload_budget: usize) -> Self {
        let chunks = usage.chunks.max(1);
        let per_chunk = |bytes: u64| bytes / chunks as u64;
        let _ = per_chunk;
        let rows = vec![
            Row {
                name: "DAG",
                gpu: 0,
                host: usage.worker_volumes,
                note: "host dense volumes (2 B/voxel, one per worker); no SVDAG is uploaded (M4)",
            },
            Row {
                name: "mesh",
                gpu: usage.pool_committed,
                host: usage.upload_backlog_bytes as u64,
                note: "pool pages committed; host side is finished meshes waiting for a budget",
            },
            Row {
                name: "overlay",
                gpu: 0,
                host: 0,
                note: "no edit overlay in the M3 scene (M4)",
            },
            Row {
                name: "light",
                gpu: usage.light_bytes,
                host: 0,
                note: "one light-cell buffer per chunk; host bytes are transient",
            },
            Row {
                name: "probes",
                gpu: usage.probe_bytes,
                host: 0,
                note: "one near-grid SH4 buffer per chunk; host bytes are transient",
            },
            Row {
                name: "origins",
                gpu: usage.origin_bytes,
                host: 0,
                note: "per-chunk origin uniform",
            },
        ];
        Self {
            chunks: usage.chunks,
            seed,
            chunk_dim,
            upload_budget,
            rows,
            pool_pages: usage.pool_pages,
            pool_page_bytes: usage.pool_page_bytes,
            pool_live: usage.pool_live,
            pool_slack: usage.pool_slack,
            heap_live: None,
            heap_peak: None,
            rss: None,
            peak_rss: None,
        }
    }

    /// The scene's host buffers (worker volumes plus queued uploads).
    pub fn host_scene_bytes(&self, usage: &MemoryUsage) -> u64 {
        usage.host_total()
    }

    /// Total VRAM across the rows.
    pub fn gpu_total(&self) -> u64 {
        self.rows.iter().map(|row| row.gpu).sum()
    }

    /// Total host bytes across the rows.
    pub fn host_total(&self) -> u64 {
        self.rows.iter().map(|row| row.host).sum()
    }

    /// Total usage × [`MARGIN`] still inside the target.
    pub fn fits_with_margin(&self) -> bool {
        (self.gpu_total() as f64 * MARGIN) <= VRAM_TARGET_BYTES as f64
    }

    /// Room left under the target, as a multiplier of the measured usage.
    pub fn headroom(&self) -> f64 {
        if self.gpu_total() == 0 {
            return f64::INFINITY;
        }
        VRAM_TARGET_BYTES as f64 / self.gpu_total() as f64
    }

    /// Per-chunk VRAM across the rows.
    pub fn per_chunk(&self) -> u64 {
        self.gpu_total() / self.chunks.max(1) as u64
    }

    /// The report as a Markdown table.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "scene memory: {} chunks, seed 0x{:08X}, {}³, upload budget {} KiB\n\n",
            self.chunks,
            self.seed,
            self.chunk_dim,
            self.upload_budget / 1024
        ));
        out.push_str("| row | VRAM | host | per chunk | note |\n");
        out.push_str("|---|---:|---:|---:|---|\n");
        for row in &self.rows {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row.name,
                size(row.gpu),
                size(row.host),
                size(row.gpu / self.chunks.max(1) as u64),
                row.note
            ));
        }
        out.push_str(&format!(
            "| **total** | **{}** | **{}** | **{}** | |\n\n",
            size(self.gpu_total()),
            size(self.host_total()),
            size(self.per_chunk())
        ));
        out.push_str(&format!(
            "pool: {} pages × {} = {} committed ({} live, {} slack)\n",
            self.pool_pages,
            mib(self.pool_page_bytes),
            size(self.rows[1].gpu),
            size(self.pool_live),
            size(self.pool_slack),
        ));
        out.push_str(&format!(
            "budget: {} used of {} (40% of 8 GiB); headroom {:.1}×; {}×1.5 = {} -> {} (hard fail {})\n",
            mib(self.gpu_total()),
            mib(VRAM_TARGET_BYTES),
            self.headroom(),
            mib(self.gpu_total()),
            mib((self.gpu_total() as f64 * MARGIN) as u64),
            if self.fits_with_margin() { "PASS" } else { "FAIL" },
            mib(VRAM_HARD_FAIL_BYTES),
        ));
        if let (Some(live), Some(peak)) = (self.heap_live, self.heap_peak) {
            out.push_str(&format!(
                "host: heap live {}, peak above baseline {}, RSS {}, peak RSS {}\n",
                mib(live),
                mib(peak),
                optional_mib(self.rss),
                optional_mib(self.peak_rss),
            ));
        }
        out
    }

    /// The report as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "chunks": self.chunks,
            "seed": format!("0x{:08X}", self.seed),
            "chunk_dim": self.chunk_dim,
            "upload_budget_kib": self.upload_budget / 1024,
            "rows": self.rows.iter().map(|row| serde_json::json!({
                "name": row.name,
                "gpu_bytes": row.gpu,
                "host_bytes": row.host,
                "note": row.note,
            })).collect::<Vec<_>>(),
            "gpu_total_bytes": self.gpu_total(),
            "host_total_bytes": self.host_total(),
            "per_chunk_bytes": self.per_chunk(),
            "pool_pages": self.pool_pages,
            "pool_page_bytes": self.pool_page_bytes,
            "pool_live_bytes": self.pool_live,
            "pool_slack_bytes": self.pool_slack,
            "vram_target_bytes": VRAM_TARGET_BYTES,
            "vram_hard_fail_bytes": VRAM_HARD_FAIL_BYTES,
            "margin": MARGIN,
            "headroom": self.headroom(),
            "fits_with_margin": self.fits_with_margin(),
            "heap_live_bytes": self.heap_live,
            "heap_peak_bytes": self.heap_peak,
            "rss_bytes": self.rss,
            "peak_rss_bytes": self.peak_rss,
        })
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// Formats a byte count as KiB below a MiB and MiB above it.
fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        mib(bytes)
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

fn optional_mib(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "n/a".to_owned(), mib)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(chunks: usize) -> MemoryUsage {
        MemoryUsage {
            chunks,
            pool_pages: 13,
            pool_page_bytes: 8 * 1024 * 1024,
            pool_committed: 13 * 8 * 1024 * 1024,
            pool_live: 100 * 1024 * 1024,
            pool_slack: 4 * 1024 * 1024,
            light_bytes: chunks as u64 * 8192,
            probe_bytes: chunks as u64 * 6144,
            origin_bytes: chunks as u64 * 16,
            upload_backlog_bytes: 0,
            worker_volumes: 4 * 64 * 1024 * 1024,
            worker_count: 4,
        }
    }

    #[test]
    fn the_rows_are_the_five_the_milestone_asks_for() {
        let report = Report::new(usage(256), 1, 128, 512 * 1024);
        let names: Vec<&str> = report.rows.iter().map(|row| row.name).collect();
        assert_eq!(
            names,
            ["DAG", "mesh", "overlay", "light", "probes", "origins"]
        );
        assert_eq!(report.rows[0].gpu, 0, "no DAG is uploaded in M3");
        assert_eq!(report.rows[2].gpu, 0, "no overlay in M3");
        assert_eq!(report.rows[3].gpu, 256 * 8192);
        assert_eq!(report.rows[4].gpu, 256 * 6144);
    }

    #[test]
    fn the_total_is_the_sum_and_the_margin_is_checked() {
        let report = Report::new(usage(256), 1, 128, 512 * 1024);
        let expected = 13 * 8 * 1024 * 1024 + 256 * (8192 + 6144 + 16);
        assert_eq!(report.gpu_total(), expected);
        assert!(report.fits_with_margin());
        assert!(report.headroom() > 20.0);
        assert!(report.markdown().contains("PASS"));
    }

    #[test]
    fn a_scene_that_breaks_the_margin_fails() {
        let mut big = usage(256);
        big.pool_committed = 3_000 * 1024 * 1024;
        let report = Report::new(big, 1, 128, 512 * 1024);
        assert!(!report.fits_with_margin(), "3 GiB x 1.5 is over 3.20 GiB");
        assert!(report.markdown().contains("FAIL"));
    }

    #[test]
    fn the_host_rows_carry_the_worker_volumes_and_the_backlog() {
        let mut usage = usage(256);
        usage.upload_backlog_bytes = 3 * 1024 * 1024;
        let report = Report::new(usage, 1, 128, 512 * 1024);
        assert_eq!(report.rows[0].host, usage.worker_volumes, "dense volumes");
        assert_eq!(report.rows[1].host, 3 * 1024 * 1024, "queued meshes");
        assert_eq!(report.host_total(), usage.worker_volumes + 3 * 1024 * 1024);
    }

    #[test]
    fn per_chunk_is_the_row_total_over_the_resident_chunks() {
        let report = Report::new(usage(256), 1, 128, 512 * 1024);
        assert_eq!(
            report.per_chunk(),
            report.gpu_total() / 256,
            "per-chunk is the total split evenly"
        );
    }
}
