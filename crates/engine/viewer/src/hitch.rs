//! M3-09: the streaming hitch test.
//!
//! Runs the real frame-thread streaming path — [`Streaming::pump`],
//! [`Streaming::install_frame`] (the M3-03 take, staging copy and install) and
//! the encoder finish plus submit — for a fixed number of frames while the
//! camera flies across the ring, and reports per-stage percentiles.
//!
//! The loop is headless: it creates no surface and records no render pass, so
//! it isolates the CPU-side streaming path from GPU draw cost. The GPU is still
//! real: meshes go through the same staging belt into the same pool pages, and
//! the queue is polled to completion each frame so the belt recycles exactly as
//! it does behind a presented frame.
//!
//! **What is measured.** `upload` is the M3-03 frame-thread upload work;
//! `pump` is the ring recenter plus install budget; `finish` is
//! `finish_and_recall_on_submit` plus `Queue::submit`; `total` is their sum.
//! The GPU wait after the submit is excluded — a hitch test bounds frame-thread
//! stalls, not GPU draw time (that is M3-10).
//!
//! **Thresholds.** [`UPLOAD_P99_BUDGET`] (4 ms) for the upload stage's p99 and
//! [`FRAME_MAX_BUDGET`] (8 ms) for the worst frame.

use std::time::{Duration, Instant};

use wgpu::util::StagingBelt;
use wgpu::{Device, PollType, Queue};

use crate::camera::Camera;
use crate::chunk_lighting;
use crate::scene::Scene;
use crate::streaming::{self, PUMP_BUDGET, RING_CHUNKS, Streaming};
use crate::upload;

/// The upload stage's p99 budget.
pub const UPLOAD_P99_BUDGET: Duration = Duration::from_millis(4);
/// No frame-thread frame may take longer than this.
pub const FRAME_MAX_BUDGET: Duration = Duration::from_millis(8);

/// How many frames the default run measures.
pub const DEFAULT_FRAMES: u64 = 1_000;
/// Simulated frame duration, so motion is comparable to a 60 Hz session.
pub const DEFAULT_FRAME_SECONDS: f32 = 1.0 / 60.0;
/// Camera speed in world units per second (a chunk is 8 units at 128³).
pub const DEFAULT_SPEED: f32 = 16.0;

/// One run's configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitchConfig {
    /// Measured frames after the ring is resident.
    pub frames: u64,
    /// Simulated seconds per frame; motion uses this, not wall time.
    pub frame_seconds: f32,
    /// Camera speed in world units per second.
    pub speed: f32,
    /// Mesh bytes one frame may upload.
    pub upload_budget: usize,
    /// Ring install budget per frame.
    pub pump_budget: Duration,
}

impl Default for HitchConfig {
    fn default() -> Self {
        Self {
            frames: DEFAULT_FRAMES,
            frame_seconds: DEFAULT_FRAME_SECONDS,
            speed: DEFAULT_SPEED,
            upload_budget: upload::DEFAULT_UPLOAD_BYTES,
            pump_budget: PUMP_BUDGET,
        }
    }
}

/// One measured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSample {
    /// Ring recenter and install budget.
    pub pump: Duration,
    /// Take, staging copy and install of finished meshes.
    pub upload: Duration,
    /// Encoder finish and `Queue::submit`.
    pub finish: Duration,
    /// The three above, summed.
    pub total: Duration,
    /// Mesh bytes copied this frame.
    pub uploaded_bytes: usize,
    /// Resident chunks after the frame.
    pub resident: usize,
    /// Desired chunks still missing after the frame.
    pub pending: usize,
}

/// Which stage a percentile is taken over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The upload step.
    Upload,
    /// The ring pump.
    Pump,
    /// Encoder finish and submit.
    Finish,
    /// Pump + upload + finish.
    Total,
}

impl FrameSample {
    /// The stage's duration.
    pub fn stage(&self, stage: Stage) -> Duration {
        match stage {
            Stage::Upload => self.upload,
            Stage::Pump => self.pump,
            Stage::Finish => self.finish,
            Stage::Total => self.total,
        }
    }
}

/// A finished run.
#[derive(Clone, Debug)]
pub struct HitchReport {
    samples: Vec<FrameSample>,
    /// Times the ring recentered during the measured frames.
    pub recenters: u64,
    /// Frames that moved bytes.
    pub upload_frames: u64,
    /// Total mesh bytes copied during the measured frames.
    pub uploaded_bytes: u64,
    /// Upload-queue budget overruns.
    pub overruns: u64,
    /// Pumps over the install budget.
    pub pump_hitches: u32,
    /// Frames the warm-up needed before the ring was resident.
    pub warmup_frames: u64,
    /// Slowest frame-thread frame during the warm-up (the initial fill).
    pub warmup_total_max: Duration,
    /// Slowest upload step during the warm-up.
    pub warmup_upload_max: Duration,
    /// Resident / pending chunks at the end of the run.
    pub resident: usize,
    pub pending: usize,
    /// The slowest frames by total, with their index, worst first.
    pub slowest: Vec<(u64, FrameSample)>,
}

impl HitchReport {
    /// Measured frames.
    pub fn frames(&self) -> u64 {
        self.samples.len() as u64
    }

    /// One stage's percentile, `p` in `0.0..=1.0`.
    pub fn percentile(&self, stage: Stage, p: f64) -> Duration {
        self.percentile_of(stage, p)
    }

    /// One stage's maximum.
    pub fn max(&self, stage: Stage) -> Duration {
        self.samples
            .iter()
            .map(|sample| sample.stage(stage))
            .max()
            .unwrap_or_default()
    }

    /// The acceptance check: upload p99 and worst frame inside budget.
    pub fn passed(&self) -> bool {
        self.percentile_of(Stage::Upload, 0.99) <= UPLOAD_P99_BUDGET
            && self.max(Stage::Total) <= FRAME_MAX_BUDGET
    }

    /// A Markdown table of the run.
    pub fn table(&self) -> String {
        let mut out = String::new();
        out.push_str("| stage | p50 | p95 | p99 | max |\n");
        out.push_str("|---|---:|---:|---:|---:|\n");
        for (name, stage) in [
            ("pump", Stage::Pump),
            ("upload", Stage::Upload),
            ("finish", Stage::Finish),
            ("total", Stage::Total),
        ] {
            out.push_str(&format!(
                "| {name} | {} | {} | {} | {} |\n",
                ms(self.percentile_of(stage, 0.50)),
                ms(self.percentile_of(stage, 0.95)),
                ms(self.percentile_of(stage, 0.99)),
                ms(self.max(stage)),
            ));
        }
        out
    }

    fn percentile_of(&self, stage: Stage, p: f64) -> Duration {
        let mut values: Vec<Duration> = self
            .samples
            .iter()
            .map(|sample| sample.stage(stage))
            .collect();
        values.sort_unstable();
        crate::stats::percentile(&values, p)
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

/// Drives the streaming path for one run.
///
/// The ring is filled at the start pose first (its frames are not measured),
/// then the camera flies in a straight diagonal line for `frames` frames.
pub fn run(device: &Device, queue: &Queue, scene: Scene, config: &HitchConfig) -> HitchReport {
    let layout = chunk_lighting::layout(device);
    let mut streaming = Streaming::new(device.clone(), layout, config.upload_budget, scene);
    let mut belt = StagingBelt::new(device.clone(), upload::STAGING_CHUNK_BYTES);

    let mut recenters = 0u64;
    let mut upload_frames = 0u64;
    let mut uploaded_bytes = 0u64;

    // Warm up: run the same frame path at the start pose until the ring is
    // resident, so the measured run is motion, not the initial fill. Frames are
    // paced to the simulated frame time, because the generator workers feed the
    // uploader in wall time: an unpaced loop would spin through a thousand
    // "frames" before the first chunk is meshed.
    let frame_time = Duration::from_secs_f32(config.frame_seconds.max(0.001));
    let start = Camera::default().position();
    let mut warmup_frames = 0u64;
    let mut warmup_upload_frames = 0u64;
    let mut warmup_uploaded_bytes = 0u64;
    let mut warmup_sample = FrameSample {
        pump: Duration::ZERO,
        upload: Duration::ZERO,
        finish: Duration::ZERO,
        total: Duration::ZERO,
        uploaded_bytes: 0,
        resident: 0,
        pending: 0,
    };
    let mut warmup_total_max = Duration::ZERO;
    let mut warmup_upload_max = Duration::ZERO;
    while streaming.loaded() < RING_CHUNKS && warmup_frames < config.frames.max(600) {
        let frame_started = Instant::now();
        pump_frame(
            device,
            queue,
            &mut streaming,
            &mut belt,
            config,
            start,
            &mut recenters,
            &mut warmup_upload_frames,
            &mut warmup_uploaded_bytes,
            Some(&mut warmup_sample),
        );
        warmup_total_max = warmup_total_max.max(warmup_sample.total);
        warmup_upload_max = warmup_upload_max.max(warmup_sample.upload);
        pace(frame_started, frame_time);
        warmup_frames += 1;
    }

    let mut samples = Vec::with_capacity(config.frames as usize);
    // A diagonal flight: both axes cross chunk edges instead of one column.
    let direction = glam::Vec3::new(0.6, 0.0, 0.8).normalize();
    for frame in 0..config.frames {
        let frame_started = Instant::now();
        let travelled = direction * (config.speed * config.frame_seconds * frame as f32);
        let mut sample = FrameSample {
            pump: Duration::ZERO,
            upload: Duration::ZERO,
            finish: Duration::ZERO,
            total: Duration::ZERO,
            uploaded_bytes: 0,
            resident: 0,
            pending: 0,
        };
        pump_frame(
            device,
            queue,
            &mut streaming,
            &mut belt,
            config,
            start + travelled,
            &mut recenters,
            &mut upload_frames,
            &mut uploaded_bytes,
            Some(&mut sample),
        );
        if sample.total > UPLOAD_P99_BUDGET * 2 {
            log::debug!(
                "hitch frame {frame}: pump {:.2} ms, upload {:.2} ms ({} KiB), finish {:.2} ms, total {:.2} ms",
                sample.pump.as_secs_f64() * 1000.0,
                sample.upload.as_secs_f64() * 1000.0,
                sample.uploaded_bytes / 1024,
                sample.finish.as_secs_f64() * 1000.0,
                sample.total.as_secs_f64() * 1000.0,
            );
        }
        samples.push(sample);
        pace(frame_started, frame_time);
    }

    // Keep the worst frames: a p99 can hide one 20 ms outlier, and the point of
    // a hitch test is to see it.
    let mut slowest: Vec<(u64, FrameSample)> = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| (index as u64, *sample))
        .collect();
    slowest.sort_by_key(|(_, sample)| std::cmp::Reverse(sample.total));
    slowest.truncate(5);

    let upload = streaming.upload_stats();
    HitchReport {
        samples,
        slowest,
        recenters,
        upload_frames,
        uploaded_bytes,
        overruns: upload.overruns,
        pump_hitches: streaming.hitches(),
        warmup_frames,
        warmup_total_max,
        warmup_upload_max,
        resident: streaming.loaded(),
        pending: streaming.pending(),
    }
}

/// Sleeps until `frame_time` has elapsed since `frame_started`.
///
/// The hitch test is a simulated 60 Hz session: motion is per frame, and the
/// generator workers run in wall time, so the measured frames have to be paced
/// for uploads to arrive during the run.
fn pace(frame_started: Instant, frame_time: Duration) {
    let elapsed = frame_started.elapsed();
    if elapsed < frame_time {
        std::thread::sleep(frame_time - elapsed);
    }
}

/// Encodes and submits one headless frame.
///
/// Without a surface there is nothing to present, so the queue is polled to
/// completion after the submit: that keeps the staging belt recycling and the
/// GPU queue at the depth a presented frame would have.
#[allow(clippy::too_many_arguments)]
fn pump_frame(
    device: &Device,
    queue: &Queue,
    streaming: &mut Streaming,
    belt: &mut StagingBelt,
    config: &HitchConfig,
    position: glam::Vec3,
    recenters: &mut u64,
    upload_frames: &mut u64,
    uploaded_bytes: &mut u64,
    sample: Option<&mut FrameSample>,
) {
    let center = streaming::center_for(position.extend(1.0).to_array());
    if streaming.center() != Some(center) && sample.is_some() {
        *recenters += 1;
    }

    let started = Instant::now();
    let stats = streaming.pump(center, config.pump_budget);
    let pump_elapsed = started.elapsed();

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hitch frame"),
    });
    let upload = streaming.install_frame(&mut encoder, belt, config.upload_budget);

    let finish_started = Instant::now();
    belt.finish_and_recall_on_submit(&encoder);
    queue.submit([encoder.finish()]);
    let finish_elapsed = finish_started.elapsed();
    let total = started.elapsed();

    if upload.meshes > 0 {
        *upload_frames += 1;
        *uploaded_bytes += upload.bytes as u64;
    }
    if let Some(sample) = sample {
        sample.pump = pump_elapsed;
        sample.upload = upload.elapsed;
        sample.finish = finish_elapsed;
        sample.total = total;
        sample.uploaded_bytes = upload.bytes;
        sample.resident = stats.loaded;
        sample.pending = stats.pending;
    }

    let _ = device.poll(PollType::Wait {
        submission_index: None,
        timeout: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(upload_ms: f64, total_ms: f64) -> FrameSample {
        FrameSample {
            pump: Duration::from_secs_f64(0.05 / 1000.0),
            upload: Duration::from_secs_f64(upload_ms / 1000.0),
            finish: Duration::from_secs_f64(0.5 / 1000.0),
            total: Duration::from_secs_f64(total_ms / 1000.0),
            uploaded_bytes: 1024,
            resident: 256,
            pending: 0,
        }
    }

    fn report(samples: Vec<FrameSample>) -> HitchReport {
        HitchReport {
            samples,
            recenters: 0,
            upload_frames: 0,
            uploaded_bytes: 0,
            overruns: 0,
            pump_hitches: 0,
            warmup_frames: 0,
            warmup_total_max: Duration::ZERO,
            warmup_upload_max: Duration::ZERO,
            resident: 256,
            pending: 0,
            slowest: Vec::new(),
        }
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let mut samples = Vec::new();
        for index in 1..=100 {
            samples.push(sample(f64::from(index), f64::from(index)));
        }
        let report = report(samples);
        assert_eq!(report.frames(), 100);
        // Nearest rank over 100 samples: p50 is the 51st value, p99 the 99th.
        assert_eq!(report.percentile(Stage::Upload, 0.50).as_millis(), 51);
        assert_eq!(report.percentile(Stage::Upload, 0.99).as_millis(), 99);
        assert_eq!(report.max(Stage::Total).as_millis(), 100);
    }

    #[test]
    fn a_run_passes_only_within_both_budgets() {
        let within = report((0..1000).map(|_| sample(2.0, 4.0)).collect());
        assert!(within.passed());

        // Upload p99 at 4.5 ms fails the 4 ms budget.
        let slow_upload = report((0..1000).map(|_| sample(4.5, 6.0)).collect());
        assert!(!slow_upload.passed());

        // One 9 ms frame fails the 8 ms ceiling even with a fast p99.
        let mut samples: Vec<FrameSample> = (0..999).map(|_| sample(2.0, 4.0)).collect();
        samples.push(sample(2.0, 9.0));
        assert!(!report(samples).passed());
    }

    #[test]
    fn the_table_names_every_stage() {
        let report = report(vec![sample(1.0, 2.0)]);
        let table = report.table();
        for name in ["pump", "upload", "finish", "total"] {
            assert!(table.contains(name), "{table}");
        }
    }
}
