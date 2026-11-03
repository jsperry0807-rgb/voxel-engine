//! Frame-time capture for the viewer.
//!
//! The viewer always logs an average / 1%-low / max-hitch summary at `info`
//! when it exits. Set `VOXEL_FRAME_PROFILE` to `1` to also log the per-second
//! line at `info`, or to a path (`VOXEL_FRAME_PROFILE=frames.csv`) to write one
//! row per presented frame. The bench reads that file:
//!
//! ```text
//! VOXEL_FRAME_PROFILE=target/frames.csv cargo run --release -p voxel-viewer
//! cargo run --release -p voxel-bench -- frame --input target/frames.csv
//! ```
//!
//! The collector records only frame-to-frame intervals, so it is cheap enough
//! to stay enabled while measuring. Percentiles and the 1% low are computed
//! once, on exit, not on the frame path.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use voxel_profiling::{FrameStats, TRACE_HEADER};

/// Collects frame intervals from an environment variable.
pub struct FrameProfiler {
    enabled: bool,
    logs_interval: bool,
    file: Option<BufWriter<File>>,
    started: Instant,
    frame: u64,
    intervals: Vec<u64>,
}

impl Default for FrameProfiler {
    fn default() -> Self {
        Self::disabled()
    }
}

impl FrameProfiler {
    /// A profiler that records nothing.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            logs_interval: false,
            file: None,
            started: Instant::now(),
            frame: 0,
            intervals: Vec::new(),
        }
    }

    /// Reads `VOXEL_FRAME_PROFILE`.
    ///
    /// The exit summary runs unless the variable is explicitly `0`, `false` or
    /// `off`. `1`, `true`, `yes` and `on` also log the one-second FPS line at
    /// `info`; any other value is a CSV path and implies the same verbose
    /// logging. A path that cannot be created leaves the summary enabled
    /// rather than disabling the measurement.
    pub fn from_env() -> Self {
        let Some(value) = std::env::var_os("VOXEL_FRAME_PROFILE") else {
            return Self {
                enabled: true,
                ..Self::disabled()
            };
        };
        let value = value.to_string_lossy();
        let value = value.trim();
        if value == "0" || matches!(value, "false" | "off") {
            return Self::disabled();
        }

        let mut profiler = Self {
            enabled: true,
            logs_interval: true,
            ..Self::disabled()
        };
        if value.is_empty() || matches!(value, "1" | "true" | "yes" | "on") {
            log::info!(
                "frame profile: per-second timing on; the summary is logged on exit, \
                 VOXEL_FRAME_PROFILE=<path> also writes a CSV"
            );
            return profiler;
        }

        let path = PathBuf::from(value);
        match File::create(&path) {
            Ok(file) => {
                let mut file = BufWriter::new(file);
                if let Err(error) = writeln!(file, "{TRACE_HEADER}") {
                    log::warn!("frame profile: cannot write {}: {error}", path.display());
                } else {
                    profiler.file = Some(file);
                    log::info!("frame profile: writing {}", path.display());
                }
            }
            Err(error) => log::warn!(
                "frame profile: cannot create {}: {error}; keeping the summary only",
                path.display()
            ),
        }
        profiler
    }

    /// Whether the profiler was enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the one-second FPS line should be logged at `info`.
    pub fn logs_interval(&self) -> bool {
        self.logs_interval
    }

    /// Records one presented frame.
    ///
    /// A zero interval is the first frame of the capture and is ignored.
    pub fn record(&mut self, interval: Duration) {
        if !self.enabled || interval.is_zero() {
            return;
        }
        self.frame = self.frame.saturating_add(1);
        let ns = interval.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.intervals.push(ns);
        if let Some(file) = &mut self.file {
            let t_ns = self.started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            if let Err(error) = writeln!(file, "{},{t_ns},{ns}", self.frame) {
                log::warn!("frame profile: write failed: {error}");
                self.file = None;
            }
        }
    }

    /// Flushes buffered CSV rows; called once per logging interval.
    pub fn flush(&mut self) {
        if let Some(file) = &mut self.file
            && let Err(error) = file.flush()
        {
            log::warn!("frame profile: flush failed: {error}");
            self.file = None;
        }
    }

    /// Flushes the trace, returns the final summary and stops collecting.
    pub fn finish(&mut self) -> Option<FrameStats> {
        if !self.enabled {
            return None;
        }
        self.flush();
        self.enabled = false;
        let stats = FrameStats::from_intervals(&self.intervals);
        (stats.frames > 0).then_some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_the_first_frame_and_zero_intervals() {
        let mut profiler = FrameProfiler::disabled();
        profiler.enabled = true;
        profiler.record(Duration::ZERO);
        profiler.record(Duration::from_millis(16));
        profiler.record(Duration::from_millis(17));
        let stats = profiler.finish().expect("two samples");
        assert_eq!(stats.frames, 2);
        assert_eq!(stats.mean_ns, 16_500_000);
    }

    #[test]
    fn disabled_profiler_records_nothing() {
        let mut profiler = FrameProfiler::disabled();
        profiler.record(Duration::from_millis(16));
        assert!(profiler.finish().is_none());
    }
}
