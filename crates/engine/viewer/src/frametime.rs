//! M3-10: the 60 Hz measurement.
//!
//! `VOXEL_FRAMETIME=path.json` makes the viewer record the interval between
//! presented frames once the ring is resident, then write a JSON report with
//! the average frame rate and the 1% / 0.1% lows and exit 0.
//!
//! The interval is the wall time between two `Queue::present` calls, so it
//! includes everything the application and the driver do per frame — draw,
//! submit, present and the swap chain. It is the number a player sees, not a
//! sum of stages. The CPU submit time and the GPU pass time from M3-06 are
//! recorded alongside it so a miss can be attributed.
//!
//! **1% low.** The report uses the common frame-time form: the 99th percentile
//! frame time, inverted to a frame rate (`1000 / p99_ms`). It also records the
//! mean of the slowest 1% of frame rates, because the two definitions disagree
//! on long-tail captures and reviewers ask which one was used.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::camera::Camera;
use crate::pose::View;
use crate::stats::percentile;
use crate::streaming::RING_CHUNKS;

/// Path of the JSON report to write, or unset for a normal run.
pub const PATH_ENV: &str = "VOXEL_FRAMETIME";
/// Recorded frames after the ring is resident.
pub const FRAMES_ENV: &str = "VOXEL_FRAMETIME_FRAMES";
/// Presented frames to discard after the ring loads, before recording.
pub const SETTLE_ENV: &str = "VOXEL_FRAMETIME_SETTLE";
/// Optional `x,y,z` eye override, in world units, for a reproducible gate pose.
pub const EYE_ENV: &str = "VOXEL_FRAMETIME_EYE";
/// Optional `x,y,z` look-at override, in world units.
pub const TARGET_ENV: &str = "VOXEL_FRAMETIME_TARGET";
/// Recorded frames when [`FRAMES_ENV`] is unset: 20 s at 60 Hz.
pub const DEFAULT_FRAMES: u64 = 1_200;
/// Default settle frames.
pub const DEFAULT_SETTLE_FRAMES: u32 = 12;

/// One frame's timings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    /// Wall time since the previous present.
    interval: Duration,
    /// CPU submit time for the frame (M3-06).
    cpu_submit: Duration,
    /// GPU pass time, when the adapter has timestamps.
    gpu_pass: Option<Duration>,
}

/// Collects frame intervals and writes one report.
#[derive(Debug)]
pub struct Recorder {
    path: PathBuf,
    frames: u64,
    settle: u32,
    settled: u32,
    samples: Vec<Sample>,
    last_present: Option<Instant>,
    /// A pinned gate pose, so the measured view does not depend on startup
    /// pointer events.
    view: Option<View>,
}

impl Recorder {
    /// Reads [`PATH_ENV`], [`FRAMES_ENV`] and [`SETTLE_ENV`].
    ///
    /// A malformed value is a startup error: a run that silently records the
    /// wrong window is worse than a failed launch.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os(PATH_ENV)?;
        let frames = parse_env(FRAMES_ENV).unwrap_or(DEFAULT_FRAMES).max(1);
        let settle = parse_env(SETTLE_ENV)
            .unwrap_or(u64::from(DEFAULT_SETTLE_FRAMES))
            .min(u64::from(u32::MAX)) as u32;
        Some(Self {
            path: PathBuf::from(path),
            frames,
            settle,
            settled: 0,
            samples: Vec::with_capacity(frames as usize),
            last_present: None,
            view: View::from_env(EYE_ENV, TARGET_ENV)
                .unwrap_or_else(|error| panic!("frametime pose: {error}")),
        })
    }

    /// The file this recorder will write.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Frames to record.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// The pinned gate pose, or `None` to use the interactive camera.
    pub fn camera(&self, base: &Camera) -> Option<Camera> {
        self.view.as_ref().map(|view| view.camera(base))
    }

    /// Frames already recorded.
    pub fn recorded(&self) -> usize {
        self.samples.len()
    }

    /// True once the recorded window is full.
    pub fn finished(&self) -> bool {
        self.samples.len() as u64 >= self.frames
    }

    /// Records one presented frame.
    ///
    /// `loaded` is the ring's resident chunk count; nothing is recorded until
    /// the whole ring is resident, so a run never measures the load phase.
    pub fn sample(&mut self, loaded: usize, cpu_submit: Duration, gpu_pass: Option<Duration>) {
        let now = Instant::now();
        let last = self.last_present.replace(now);
        if loaded < RING_CHUNKS || self.finished() {
            return;
        }
        let Some(last) = last else {
            // The first present after the ring loads has no usable interval.
            return;
        };
        if self.settled < self.settle {
            self.settled += 1;
            return;
        }
        self.samples.push(Sample {
            interval: now.duration_since(last),
            cpu_submit,
            gpu_pass,
        });
    }

    /// Builds the report for the recorded window.
    pub fn summary(&self, context: Context) -> Summary {
        let mut intervals: Vec<Duration> = self.samples.iter().map(|s| s.interval).collect();
        intervals.sort_unstable();
        let mut cpu: Vec<Duration> = self.samples.iter().map(|s| s.cpu_submit).collect();
        cpu.sort_unstable();
        let mut gpu: Vec<Duration> = self.samples.iter().filter_map(|s| s.gpu_pass).collect();
        gpu.sort_unstable();

        // The window's duration is the sum of its intervals: that is exactly the
        // time the recorded frames cover, and it makes the average independent
        // of when the recorder happened to start.
        let elapsed: Duration = intervals.iter().sum();
        let frames = self.samples.len() as u64;
        let average_fps = if elapsed.is_zero() {
            0.0
        } else {
            frames as f64 / elapsed.as_secs_f64()
        };
        let p99 = percentile(&intervals, 0.99);
        let p999 = percentile(&intervals, 0.999);
        Summary {
            context,
            frames,
            elapsed,
            average_fps,
            one_percent_low_fps: slowest_mean_fps(&intervals, 0.01),
            point_one_percent_low_fps: slowest_mean_fps(&intervals, 0.001),
            slowest_one_percent_mean_fps: slowest_mean_fps(&intervals, 0.01),
            frame_p50: percentile(&intervals, 0.50),
            frame_p95: percentile(&intervals, 0.95),
            frame_p99: p99,
            frame_p999: p999,
            frame_max: intervals.last().copied().unwrap_or_default(),
            cpu_p50: percentile(&cpu, 0.50),
            cpu_p99: percentile(&cpu, 0.99),
            gpu_p50: (!gpu.is_empty()).then(|| percentile(&gpu, 0.50)),
            gpu_p99: (!gpu.is_empty()).then(|| percentile(&gpu, 0.99)),
            recorded: self.samples.len(),
        }
    }

    /// Writes the JSON report and returns it.
    pub fn finish(&self, context: Context) -> Result<Summary, String> {
        let summary = self.summary(context);
        let json = summary.to_json();
        let text = serde_json::to_string_pretty(&json)
            .map_err(|error| format!("serialize the frame-time report: {error}"))?;
        std::fs::write(&self.path, text + "\n")
            .map_err(|error| format!("write {}: {error}", self.path.display()))?;
        Ok(summary)
    }
}

/// What the run measured, for the report header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// Adapter name.
    pub adapter: String,
    /// Backend, for example `Vulkan`.
    pub backend: String,
    /// Device type, for example `DiscreteGpu`.
    pub device_type: String,
    /// Drawable width in pixels.
    pub width: u32,
    /// Drawable height in pixels.
    pub height: u32,
    /// Scene seed.
    pub seed: u32,
    /// Chunk edge in voxels.
    pub chunk_dim: u32,
    /// Resident chunks.
    pub resident: usize,
}

/// The recorded window's statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// The run's context.
    pub context: Context,
    /// Recorded frames.
    pub frames: u64,
    /// Wall time covered by the recorded frames.
    pub elapsed: Duration,
    /// `frames / elapsed`.
    pub average_fps: f64,
    /// Mean rate of the slowest 1% of frames (the 1% low).
    pub one_percent_low_fps: f64,
    /// Mean rate of the slowest 0.1% of frames.
    pub point_one_percent_low_fps: f64,
    /// Same as [`Self::one_percent_low_fps`], kept explicit in the report.
    pub slowest_one_percent_mean_fps: f64,
    /// Frame interval percentiles.
    pub frame_p50: Duration,
    /// 95th percentile frame interval.
    pub frame_p95: Duration,
    /// 99th percentile frame interval.
    pub frame_p99: Duration,
    /// 99.9th percentile frame interval.
    pub frame_p999: Duration,
    /// Slowest frame interval.
    pub frame_max: Duration,
    /// CPU submit percentiles.
    pub cpu_p50: Duration,
    /// 99th percentile CPU submit.
    pub cpu_p99: Duration,
    /// GPU pass percentiles, when measurable.
    pub gpu_p50: Option<Duration>,
    /// 99th percentile GPU pass.
    pub gpu_p99: Option<Duration>,
    /// Frames recorded (equals `frames`).
    pub recorded: usize,
}

impl Summary {
    /// The one-line summary the viewer logs.
    pub fn line(&self) -> String {
        format!(
            "frametime: {} frames over {:.2} s | avg {:.1} fps, 1% low {:.1} fps ({:.2} ms), 0.1% low {:.1} fps | frame p50 {:.2}, p99 {:.2}, max {:.2} ms | cpu p99 {:.2} ms, gpu p99 {}",
            self.frames,
            self.elapsed.as_secs_f64(),
            self.average_fps,
            self.one_percent_low_fps,
            ms(self.frame_p99),
            self.point_one_percent_low_fps,
            ms(self.frame_p50),
            ms(self.frame_p99),
            ms(self.frame_max),
            ms(self.cpu_p99),
            self.gpu_p99
                .map_or_else(|| "n/a".to_owned(), |gpu| format!("{:.2} ms", ms(gpu))),
        )
    }

    /// True when both the average and the 1% low clear 60 Hz.
    pub fn clears_sixty_hertz(&self) -> bool {
        self.average_fps >= 60.0
            && self.one_percent_low_fps >= 60.0
            && self.point_one_percent_low_fps >= 60.0
    }

    /// The report as JSON.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "adapter": self.context.adapter,
            "backend": self.context.backend,
            "device_type": self.context.device_type,
            "width": self.context.width,
            "height": self.context.height,
            "seed": format!("0x{:08X}", self.context.seed),
            "chunk_dim": self.context.chunk_dim,
            "resident": self.context.resident,
            "frames": self.frames,
            "elapsed_s": self.elapsed.as_secs_f64(),
            "average_fps": self.average_fps,
            "low_1_percent_fps": self.one_percent_low_fps,
            "low_0_1_percent_fps": self.point_one_percent_low_fps,
            "slowest_1_percent_mean_fps": self.slowest_one_percent_mean_fps,
            "frame_p50_ms": ms(self.frame_p50),
            "frame_p95_ms": ms(self.frame_p95),
            "frame_p99_ms": ms(self.frame_p99),
            "frame_p999_ms": ms(self.frame_p999),
            "frame_max_ms": ms(self.frame_max),
            "cpu_p50_ms": ms(self.cpu_p50),
            "cpu_p99_ms": ms(self.cpu_p99),
            "gpu_p50_ms": self.gpu_p50.map(ms),
            "gpu_p99_ms": self.gpu_p99.map(ms),
            "clears_60hz": self.clears_sixty_hertz(),
        })
    }
}

/// Mean rate of the slowest `fraction` of frames, from ascending frame times.
///
/// This is the frame-rate form of a low percentile: the recorded rates of the
/// slowest bucket (`ceil(n * fraction)` frames, at least one) averaged back to
/// one frame rate.
fn slowest_mean_fps(intervals: &[Duration], fraction: f64) -> f64 {
    if intervals.is_empty() {
        return 0.0;
    }
    let count = ((intervals.len() as f64 * fraction).ceil() as usize).max(1);
    let slowest = &intervals[intervals.len() - count..];
    let total: f64 = slowest.iter().map(|frame| frame.as_secs_f64()).sum();
    if total > 0.0 {
        count as f64 / total
    } else {
        0.0
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// Parses a positive integer environment variable.
fn parse_env(name: &str) -> Option<u64> {
    let text = std::env::var(name).ok()?;
    match text.trim().parse::<u64>() {
        Ok(value) => Some(value),
        Err(_) => panic!("{name}={text:?} is not a non-negative integer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder(intervals_ms: &[f64]) -> Recorder {
        let samples: Vec<Sample> = intervals_ms
            .iter()
            .map(|ms| Sample {
                interval: Duration::from_secs_f64(ms / 1000.0),
                cpu_submit: Duration::from_secs_f64(0.001),
                gpu_pass: Some(Duration::from_secs_f64(0.002)),
            })
            .collect();
        let frames = samples.len() as u64;
        Recorder {
            path: PathBuf::from("/tmp/frametime.json"),
            frames,
            settle: 0,
            settled: 0,
            samples,
            last_present: None,
            view: None,
        }
    }

    #[test]
    fn the_one_percent_low_is_the_slowest_hundredth_of_frames() {
        // 100 frames: 99 at 5 ms, one 50 ms hitch.
        let mut intervals = vec![5.0; 99];
        intervals.push(50.0);
        let summary = recorder(&intervals).summary(Context::default());
        assert_eq!(summary.frames, 100);
        // 1 frame is 1% of 100: the 1% low is that frame's 20 fps.
        assert!((summary.one_percent_low_fps - 20.0).abs() < 1e-9);
        assert!((summary.slowest_one_percent_mean_fps - 20.0).abs() < 1e-9);
        // The statistical p99 of 100 samples is the 99th value, so it reports
        // the 5 ms body; the low is the worst frame.
        assert_eq!(summary.frame_p99, Duration::from_millis(5));
        assert!(
            summary.average_fps > 60.0,
            "a single hitch does not move the average"
        );
        assert!(!summary.clears_sixty_hertz(), "the 1% low fails the gate");
    }

    #[test]
    fn a_steady_sixty_hertz_run_clears_the_gate() {
        let summary = recorder(&vec![16.0; 120]).summary(Context::default());
        assert!((summary.average_fps - 62.5).abs() < 1e-6);
        assert!((summary.one_percent_low_fps - 62.5).abs() < 1e-6);
        assert!(summary.clears_sixty_hertz());
    }

    #[test]
    fn the_slowest_mean_uses_the_rounded_up_one_percent() {
        let mut intervals = vec![4.0; 250];
        intervals.push(40.0);
        let summary = recorder(&intervals).summary(Context::default());
        // ceil(251 * 0.01) = 3 frames, but only one is slow; the mean of the
        // three slowest is (40 + 4 + 4) / 3 = 16 ms.
        assert!(
            (summary.slowest_one_percent_mean_fps - 62.5).abs() < 1e-6,
            "{}",
            summary.slowest_one_percent_mean_fps
        );
    }

    #[test]
    fn the_report_carries_the_context() {
        let context = Context {
            adapter: "Test GPU".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "DiscreteGpu".to_owned(),
            width: 1920,
            height: 1080,
            seed: 0x5EED_1234,
            chunk_dim: 128,
            resident: 256,
        };
        let json = recorder(&[16.0; 60]).summary(context).to_json();
        assert_eq!(json["adapter"], "Test GPU");
        assert_eq!(json["width"], 1920);
        assert_eq!(json["seed"], "0x5EED1234");
        assert_eq!(json["clears_60hz"], true);
        assert!(json["gpu_p99_ms"].as_f64().is_some());
    }
}
