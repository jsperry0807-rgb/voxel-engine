//! M3-06 per-frame instrumentation.
//!
//! One [`FrameStats`] per rendered frame: CPU submit time, GPU pass time (when
//! the adapter has timestamp queries), the visible/drawn chunk counts, the
//! geometry those chunks cover (blocks, quads, mesh bytes) and the bytes this
//! frame uploaded. [`StatsWindow`] turns a stream of frames into one log line
//! per second so a normal run reports values without drowning in output;
//! `Gpu::render` also logs the raw frame at `debug`.
//!
//! The GPU pass is measured with two timestamps around the render pass and a
//! resolved 16 B readback. The readback is mapped asynchronously and read on a
//! later frame, so the measurement never blocks the frame; while it is in
//! flight the timer reports the previous value.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Features, MapMode, PollType,
    QuerySet, QuerySetDescriptor, QueryType, Queue, RenderPass,
};

/// What one frame did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Resident chunks.
    pub resident: usize,
    /// Chunks inside the frustum, including empty ones.
    pub visible: usize,
    /// Chunks with a mesh drawn.
    pub drawn: usize,
    /// Blocks with visible faces across the drawn chunks.
    pub blocks: u64,
    /// Indexed quads across the drawn chunks.
    pub quads: u64,
    /// Vertex plus index bytes across the drawn chunks.
    pub mesh_bytes: u64,
    /// Meshes copied into the pool this frame.
    pub upload_meshes: usize,
    /// Mesh bytes copied into the pool this frame.
    pub upload_bytes: usize,
    /// Time from the start of frame encoding to `Queue::submit` returning.
    pub cpu_submit: Duration,
    /// Part of `cpu_submit` spent staging upload copies and installing them.
    pub cpu_upload: Duration,
    /// Part of `cpu_submit` spent building the frustum and recording the draw
    /// loop (the M3-04 cull and the M3-05 pass encoding).
    pub cpu_cull: Duration,
    /// Part of `cpu_submit` spent finishing the encoder and submitting it.
    pub cpu_finish: Duration,
    /// GPU time inside the render pass, from timestamp queries.
    pub gpu_pass: Option<Duration>,
}

/// Accumulates frames into a once-per-second summary.
#[derive(Debug)]
pub struct StatsWindow {
    started: Instant,
    frames: u64,
    cpu: Duration,
    cpu_upload: Duration,
    cpu_cull: Duration,
    cpu_finish: Duration,
    gpu: Duration,
    gpu_samples: u64,
    visible: u64,
    drawn: u64,
    blocks: u64,
    quads: u64,
    mesh_bytes: u64,
    upload_bytes: u64,
    upload_meshes: u64,
    resident: usize,
}

impl Default for StatsWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// Averages accumulated when the window closes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// Frames per second over the window.
    pub fps: f64,
    /// Frames counted.
    pub frames: u64,
    /// Mean CPU submit time (the sum of the three parts).
    pub cpu_submit: Duration,
    /// Mean upload staging time.
    pub cpu_upload: Duration,
    /// Mean cull and draw-recording time.
    pub cpu_cull: Duration,
    /// Mean encoder-finish and submit time.
    pub cpu_finish: Duration,
    /// Mean GPU pass time, or `None` when no sample was collected.
    pub gpu_pass: Option<Duration>,
    /// Mean visible chunks.
    pub visible: f64,
    /// Mean drawn chunks.
    pub drawn: f64,
    /// Mean blocks with visible faces.
    pub blocks: f64,
    /// Mean quads.
    pub quads: f64,
    /// Mean mesh bytes.
    pub mesh_bytes: f64,
    /// Mean uploaded bytes.
    pub upload_bytes: f64,
    /// Resident chunks at the end of the window.
    pub resident: usize,
}

impl StatsWindow {
    /// Reporting interval.
    pub const INTERVAL: Duration = Duration::from_secs(1);

    /// Creates an empty window.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            frames: 0,
            cpu: Duration::ZERO,
            cpu_upload: Duration::ZERO,
            cpu_cull: Duration::ZERO,
            cpu_finish: Duration::ZERO,
            gpu: Duration::ZERO,
            gpu_samples: 0,
            visible: 0,
            drawn: 0,
            blocks: 0,
            quads: 0,
            mesh_bytes: 0,
            upload_bytes: 0,
            upload_meshes: 0,
            resident: 0,
        }
    }

    /// Records one frame. Returns the summary when the interval elapses.
    pub fn record(&mut self, stats: FrameStats) -> Option<Summary> {
        self.frames += 1;
        self.cpu += stats.cpu_submit;
        self.cpu_upload += stats.cpu_upload;
        self.cpu_cull += stats.cpu_cull;
        self.cpu_finish += stats.cpu_finish;
        if let Some(gpu) = stats.gpu_pass {
            self.gpu += gpu;
            self.gpu_samples += 1;
        }
        self.visible += stats.visible as u64;
        self.drawn += stats.drawn as u64;
        self.blocks += stats.blocks;
        self.quads += stats.quads;
        self.mesh_bytes += stats.mesh_bytes;
        self.upload_bytes += stats.upload_bytes as u64;
        self.upload_meshes += stats.upload_meshes as u64;
        self.resident = stats.resident;

        let elapsed = self.started.elapsed();
        if elapsed < Self::INTERVAL || self.frames == 0 {
            return None;
        }
        let frames = self.frames as f64;
        let summary = Summary {
            fps: frames / elapsed.as_secs_f64(),
            frames: self.frames,
            cpu_submit: self.cpu / self.frames as u32,
            cpu_upload: self.cpu_upload / self.frames as u32,
            cpu_cull: self.cpu_cull / self.frames as u32,
            cpu_finish: self.cpu_finish / self.frames as u32,
            gpu_pass: (self.gpu_samples > 0).then(|| self.gpu / self.gpu_samples as u32),
            visible: self.visible as f64 / frames,
            drawn: self.drawn as f64 / frames,
            blocks: self.blocks as f64 / frames,
            quads: self.quads as f64 / frames,
            mesh_bytes: self.mesh_bytes as f64 / frames,
            upload_bytes: self.upload_bytes as f64 / frames,
            resident: self.resident,
        };
        *self = Self::new();
        Some(summary)
    }

    /// Total uploaded meshes counted so far (before the window resets).
    pub fn upload_meshes(&self) -> u64 {
        self.upload_meshes
    }
}

impl Summary {
    /// Formats the one-line summary.
    pub fn line(&self) -> String {
        let gpu = self
            .gpu_pass
            .map_or_else(|| "n/a".to_owned(), |gpu| format!("{:.2} ms", ms(gpu)));
        format!(
            "frame: {:.0} fps | cpu submit {:.2} ms (cull {:.2}, upload {:.2}, finish {:.2}), gpu pass {gpu} | {}: {:.0} visible, {:.0} drawn | {:.0} blocks, {:.0} quads, {:.1} MiB | upload {:.0} KiB",
            self.fps,
            ms(self.cpu_submit),
            ms(self.cpu_cull),
            ms(self.cpu_upload),
            ms(self.cpu_finish),
            self.resident,
            self.visible,
            self.drawn,
            self.blocks,
            self.quads,
            self.mesh_bytes / (1024.0 * 1024.0),
            self.upload_bytes / 1024.0,
        )
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Nearest-rank percentile over an ascending slice, `p` in `0.0..=1.0`.
///
/// The rank is `round((n - 1) * p)`, so `p = 0.99` over 100 samples is the 99th
/// value and `p = 1.0` is the maximum. An empty slice is zero.
pub fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p.clamp(0.0, 1.0)).round() as usize;
    sorted[rank]
}

/// Timestamps around the render pass, read back a frame later.
#[derive(Debug)]
pub struct GpuPassTimer {
    query: QuerySet,
    /// Resolve destination. `MAP_READ` cannot be combined with
    /// `QUERY_RESOLVE`, so the result is copied into `readback`.
    resolve: Buffer,
    readback: Buffer,
    /// Nanoseconds per timestamp tick.
    period_ns: f64,
    last: Option<Duration>,
    mapped: Arc<AtomicBool>,
    in_flight: bool,
}

impl GpuPassTimer {
    /// Builds a timer, or `None` when the device lacks timestamp queries.
    pub fn new(device: &Device, queue: &Queue) -> Option<Self> {
        // The pass is bracketed by in-pass timestamps: both adapters expose
        // TIMESTAMP_QUERY_INSIDE_PASSES, while `CommandEncoder::write_timestamp`
        // needs the separate INSIDE_ENCODERS feature they do not have.
        let required = Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        if !device.features().contains(required) {
            log::info!("gpu pass timing off: the device has no timestamp queries");
            return None;
        }
        let query = device.create_query_set(&QuerySetDescriptor {
            label: Some("gpu pass timer"),
            ty: QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&BufferDescriptor {
            label: Some("gpu pass resolve"),
            size: 16,
            usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&BufferDescriptor {
            label: Some("gpu pass readback"),
            size: 16,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Some(Self {
            query,
            resolve,
            readback,
            period_ns: f64::from(queue.get_timestamp_period()),
            last: None,
            mapped: Arc::new(AtomicBool::new(false)),
            in_flight: false,
        })
    }

    /// True when a new measurement can start this frame.
    pub fn ready(&self) -> bool {
        !self.in_flight
    }

    /// Writes the opening timestamp at the top of the render pass.
    pub fn begin(&self, pass: &mut RenderPass<'_>) {
        pass.write_timestamp(&self.query, 0);
    }

    /// Writes the closing timestamp at the bottom of the render pass.
    pub fn end(&self, pass: &mut RenderPass<'_>) {
        pass.write_timestamp(&self.query, 1);
    }

    /// Resolves both timestamps and copies them into the mappable buffer.
    pub fn resolve(&self, encoder: &mut CommandEncoder) {
        encoder.resolve_query_set(&self.query, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, 16);
    }

    /// Starts the asynchronous readback; call after `Queue::submit`.
    pub fn submitted(&mut self) {
        self.in_flight = true;
        self.mapped.store(false, Ordering::Release);
        let mapped = Arc::clone(&self.mapped);
        self.readback
            .slice(..16)
            .map_async(MapMode::Read, move |result| {
                if result.is_ok() {
                    mapped.store(true, Ordering::Release);
                }
            });
    }

    /// Polls for the pending readback and returns the latest pass time.
    ///
    /// Never blocks: while the copy is in flight the previous value is
    /// returned (and a frame without a sample reports `None`).
    pub fn poll(&mut self, device: &Device) -> Option<Duration> {
        // Progresses map callbacks without waiting for the queue to drain.
        let _ = device.poll(PollType::Poll);
        if self.in_flight && self.mapped.load(Ordering::Acquire) {
            {
                let range = self.readback.slice(..16);
                if let Ok(view) = range.get_mapped_range() {
                    let mut begin = [0u8; 8];
                    let mut end = [0u8; 8];
                    begin.copy_from_slice(&view[0..8]);
                    end.copy_from_slice(&view[8..16]);
                    let begin = u64::from_le_bytes(begin);
                    let end = u64::from_le_bytes(end);
                    if end >= begin {
                        let ticks = (end - begin) as f64;
                        self.last = Some(Duration::from_secs_f64(ticks * self.period_ns / 1e9));
                    }
                }
            }
            self.readback.unmap();
            self.in_flight = false;
        }
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(cpu_ms: f64, gpu_ms: f64, visible: usize) -> FrameStats {
        FrameStats {
            resident: 256,
            visible,
            drawn: visible,
            blocks: 10,
            quads: 1_000,
            mesh_bytes: 44_000,
            upload_meshes: 1,
            upload_bytes: 1_024,
            cpu_submit: Duration::from_secs_f64(cpu_ms / 1000.0),
            cpu_upload: Duration::from_secs_f64(cpu_ms * 0.25 / 1000.0),
            cpu_cull: Duration::from_secs_f64(cpu_ms * 0.5 / 1000.0),
            cpu_finish: Duration::from_secs_f64(cpu_ms * 0.25 / 1000.0),
            gpu_pass: Some(Duration::from_secs_f64(gpu_ms / 1000.0)),
        }
    }

    #[test]
    fn a_window_reports_means_after_the_interval() {
        let mut window = StatsWindow::new();
        assert!(window.record(frame(1.0, 2.0, 90)).is_none());
        // Force the interval to elapse without sleeping a second.
        window.started -= StatsWindow::INTERVAL;
        let summary = window
            .record(frame(3.0, 4.0, 94))
            .expect("the interval elapsed");
        assert_eq!(summary.frames, 2);
        assert!((ms(summary.cpu_submit) - 2.0).abs() < 1e-6);
        assert!((ms(summary.cpu_cull) - 1.0).abs() < 1e-6);
        assert!((ms(summary.cpu_upload) - 0.5).abs() < 1e-6);
        assert!((ms(summary.cpu_finish) - 0.5).abs() < 1e-6);
        assert!((ms(summary.gpu_pass.expect("gpu samples")) - 3.0).abs() < 1e-6);
        assert!((summary.visible - 92.0).abs() < 1e-6);
        assert!((summary.quads - 1_000.0).abs() < 1e-6);
        assert!((summary.mesh_bytes - 44_000.0).abs() < 1e-6);
        assert_eq!(summary.resident, 256);
        let line = summary.line();
        assert!(line.contains("visible"), "{line}");
        assert!(line.contains("gpu pass 3.00 ms"), "{line}");
    }

    #[test]
    fn a_window_without_gpu_samples_reports_none() {
        let mut window = StatsWindow::new();
        let mut stats = frame(1.0, 2.0, 1);
        stats.gpu_pass = None;
        window.record(stats);
        window.started -= StatsWindow::INTERVAL;
        let summary = window.record(stats).expect("the interval elapsed");
        assert_eq!(summary.gpu_pass, None);
        assert!(summary.line().contains("gpu pass n/a"));
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let values: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&values, 0.0), Duration::from_millis(1));
        assert_eq!(percentile(&values, 0.5), Duration::from_millis(51));
        assert_eq!(percentile(&values, 0.99), Duration::from_millis(99));
        assert_eq!(percentile(&values, 1.0), Duration::from_millis(100));
        assert_eq!(percentile(&[], 0.5), Duration::ZERO);
    }

    #[test]
    fn the_window_resets_after_a_summary() {
        let mut window = StatsWindow::new();
        window.record(frame(1.0, 1.0, 10));
        window.started -= StatsWindow::INTERVAL;
        assert!(window.record(frame(1.0, 1.0, 10)).is_some());
        assert!(window.record(frame(1.0, 1.0, 10)).is_none());
    }
}
