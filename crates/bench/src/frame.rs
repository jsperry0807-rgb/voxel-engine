//! Frame-time lens (M3-06, M3-09 and M3-10).
//!
//! The viewer writes a frame trace (`VOXEL_FRAME_PROFILE=<path>`); this module
//! turns it into the same kind of report as the other benchmark lenses. A
//! frame trace is the only evidence that can check the reference machine's
//! rule: average 60 fps, 1% low 45 fps, and no frame above 33.3 ms. An average
//! alone hides a hitch, and a hitch alone hides a run that is uniformly slow.
//!
//! Run it standalone:
//!
//! ```text
//! cargo run --release -p voxel-bench -- frame --input target/frames.csv
//! ```
//!
//! or attach it to the regular report:
//!
//! ```text
//! cargo run --release -p voxel-bench -- all --frames target/frames.csv
//! ```

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use voxel_profiling::{FrameStats, HITCH_NS, ONE_PERCENT_LOW_FPS, TARGET_FPS, parse_trace};

/// The thresholds a frame trace is judged against.
#[derive(Clone, Copy, Debug)]
pub struct FrameTarget {
    /// Minimum average frame rate.
    pub avg_fps: f64,
    /// Minimum 1% low frame rate.
    pub one_percent_low_fps: f64,
    /// A frame longer than this is a hitch.
    pub hitch_ns: u64,
}

impl Default for FrameTarget {
    fn default() -> Self {
        Self {
            avg_fps: TARGET_FPS,
            one_percent_low_fps: ONE_PERCENT_LOW_FPS,
            hitch_ns: HITCH_NS,
        }
    }
}

/// One pass/fail row with a positive margin when the check passes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameCheck {
    /// Human-readable check name.
    pub metric: String,
    /// Measured value in `unit`.
    pub measured: f64,
    /// Limit in `unit`.
    pub limit: f64,
    /// `fps` or `ms`.
    pub unit: String,
    /// `measured - limit` for lower-bound checks, `limit - measured` for
    /// upper-bound checks. Positive means the check passes.
    pub margin: f64,
    /// Whether the measured value is inside the limit.
    pub passed: bool,
}

/// One frame-time report over a viewer trace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameReport {
    /// Trace path as given to the command.
    pub trace: String,
    /// Frames discarded from the front of the trace.
    pub warmup_frames: u64,
    /// Frames measured after warm-up.
    pub frames: u64,
    /// Sum of the measured frame intervals.
    pub duration_ns: u64,
    /// Mean frame interval.
    pub mean_ns: u64,
    /// Nearest-rank median frame interval.
    pub p50_ns: u64,
    /// Nearest-rank 95th percentile frame interval.
    pub p95_ns: u64,
    /// Nearest-rank 99th percentile frame interval.
    pub p99_ns: u64,
    /// Longest measured frame interval.
    pub max_ns: u64,
    /// Average frame rate.
    pub avg_fps: f64,
    /// Frame rate of the slowest `ceil(1%)` frames.
    pub one_percent_low_fps: f64,
    /// Frames slower than the 60 Hz budget for the requested average.
    pub over_budget: u64,
    /// Frames slower than the requested hitch threshold.
    pub hitches: u64,
    /// The 60 Hz budget implied by `target_avg_fps`.
    pub frame_budget_ns: u64,
    /// The hitch threshold this report was judged against.
    pub hitch_threshold_ns: u64,
    /// The average frame-rate target.
    pub target_avg_fps: f64,
    /// The 1% low target.
    pub target_one_percent_low_fps: f64,
    /// The three decision rows.
    pub checks: Vec<FrameCheck>,
    /// True only when every check passed.
    pub passed: bool,
}

impl FrameReport {
    /// Reads `path`, drops `warmup` samples and applies `target`.
    pub fn from_trace(path: &Path, warmup: usize, target: FrameTarget) -> io::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            io::Error::new(error.kind(), format!("read {}: {error}", path.display()))
        })?;
        let mut intervals = parse_trace(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let samples = intervals.len();
        if warmup >= samples {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} contains {samples} frame(s), so a {warmup}-frame warm-up leaves nothing to measure",
                    path.display()
                ),
            ));
        }
        intervals.drain(..warmup);
        let stats = FrameStats::from_intervals(&intervals);
        let frame_budget_ns = (1e9 / target.avg_fps).round() as u64;
        let over_budget = intervals.iter().filter(|&&ns| ns > frame_budget_ns).count() as u64;
        let hitches = intervals.iter().filter(|&&ns| ns > target.hitch_ns).count() as u64;

        let average = FrameCheck {
            metric: "Average".to_owned(),
            measured: stats.avg_fps,
            limit: target.avg_fps,
            unit: "fps".to_owned(),
            margin: stats.avg_fps - target.avg_fps,
            passed: stats.avg_fps >= target.avg_fps,
        };
        let one_percent_low = FrameCheck {
            metric: "1% low".to_owned(),
            measured: stats.one_percent_low_fps,
            limit: target.one_percent_low_fps,
            unit: "fps".to_owned(),
            margin: stats.one_percent_low_fps - target.one_percent_low_fps,
            passed: stats.one_percent_low_fps >= target.one_percent_low_fps,
        };
        let max_frame = FrameCheck {
            metric: "Max frame".to_owned(),
            measured: stats.max_ns as f64 / 1e6,
            limit: target.hitch_ns as f64 / 1e6,
            unit: "ms".to_owned(),
            margin: (target.hitch_ns as f64 - stats.max_ns as f64) / 1e6,
            passed: stats.max_ns <= target.hitch_ns,
        };
        let checks = vec![average, one_percent_low, max_frame];
        let passed = checks.iter().all(|check| check.passed);

        Ok(Self {
            trace: path.display().to_string(),
            warmup_frames: warmup as u64,
            frames: stats.frames,
            duration_ns: stats.duration_ns,
            mean_ns: stats.mean_ns,
            p50_ns: stats.p50_ns,
            p95_ns: stats.p95_ns,
            p99_ns: stats.p99_ns,
            max_ns: stats.max_ns,
            avg_fps: stats.avg_fps,
            one_percent_low_fps: stats.one_percent_low_fps,
            over_budget,
            hitches,
            frame_budget_ns,
            hitch_threshold_ns: target.hitch_ns,
            target_avg_fps: target.avg_fps,
            target_one_percent_low_fps: target.one_percent_low_fps,
            checks,
            passed,
        })
    }

    /// Writes `frame.md` and `frame.json` into `dir`.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| io::Error::other(error.to_string()))?;
        std::fs::write(dir.join("frame.json"), format!("{json}\n"))?;
        std::fs::write(dir.join("frame.md"), self.markdown())?;
        Ok(())
    }

    /// Renders the standalone report.
    pub fn markdown(&self) -> String {
        let mut out = String::from("# Frame-time report\n\n");
        let _ = writeln!(out, "**Trace:** `{}`\n", self.trace);
        out.push_str(&self.body());
        out
    }

    /// Renders the section embedded in the main benchmark report.
    pub fn section(&self) -> String {
        let mut out = String::from("## Frame time (M3-09/M3-10)\n\n");
        let _ = writeln!(out, "**Trace:** `{}`\n", self.trace);
        out.push_str(&self.body());
        out
    }

    fn body(&self) -> String {
        let mut out = String::new();
        let total = self.frames + self.warmup_frames;
        let _ = writeln!(
            out,
            "**Samples:** {total} in the trace, {} warm-up discarded, {} measured over {}.\n",
            self.warmup_frames,
            self.frames,
            duration(self.duration_ns),
        );
        let _ = writeln!(
            out,
            "Targets from `docs/reference-machine.md`: average >= {:.1} fps, \
             1% low >= {:.1} fps, and no frame above {}.\n",
            self.target_avg_fps,
            self.target_one_percent_low_fps,
            millis(self.hitch_threshold_ns),
        );

        out.push_str("| Check | Measured | Target | Margin | Verdict |\n");
        out.push_str("|---|---:|---:|---:|---|\n");
        for check in &self.checks {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                check.metric,
                measured(check.measured, &check.unit),
                limit(check.limit, &check.unit),
                signed_margin(check.margin, &check.unit),
                if check.passed { "PASS" } else { "FAIL" },
            );
        }
        out.push('\n');

        out.push_str("| Metric | Value |\n|---|---:|\n");
        let _ = writeln!(out, "| Mean frame | {} |", millis(self.mean_ns));
        let _ = writeln!(out, "| p50 frame | {} |", millis(self.p50_ns));
        let _ = writeln!(out, "| p95 frame | {} |", millis(self.p95_ns));
        let _ = writeln!(out, "| p99 frame | {} |", millis(self.p99_ns));
        let _ = writeln!(out, "| Max frame | {} |", millis(self.max_ns));
        let _ = writeln!(out, "| Average rate | {:.1} fps |", self.avg_fps);
        let _ = writeln!(out, "| 1% low rate | {:.1} fps |", self.one_percent_low_fps);
        let _ = writeln!(
            out,
            "| Over {} | {} |",
            millis(self.frame_budget_ns),
            self.over_budget,
        );
        let _ = writeln!(
            out,
            "| Hitches > {} | {} |",
            millis(self.hitch_threshold_ns),
            self.hitches,
        );
        out.push('\n');

        let failed = self.checks.iter().filter(|check| !check.passed).count();
        if self.passed {
            out.push_str("**Verdict: PASS** — all three checks are inside target.\n");
        } else {
            let _ = writeln!(
                out,
                "**Verdict: FAIL** — {failed} of {} checks missed target.\n",
                self.checks.len(),
            );
        }
        out
    }
}

fn measured(value: f64, unit: &str) -> String {
    match unit {
        "fps" => format!("{value:.1} fps"),
        "ms" => format!("{value:.2} ms"),
        _ => format!("{value:.2} {unit}"),
    }
}

fn limit(value: f64, unit: &str) -> String {
    match unit {
        "fps" => format!(">= {value:.1} fps"),
        "ms" => format!("<= {value:.2} ms"),
        _ => format!("<= {value:.2} {unit}"),
    }
}

fn signed_margin(value: f64, unit: &str) -> String {
    let sign = if value >= 0.0 { "+" } else { "" };
    match unit {
        "fps" => format!("{sign}{value:.1} fps"),
        "ms" => format!("{sign}{value:.2} ms"),
        _ => format!("{sign}{value:.2} {unit}"),
    }
}

fn millis(ns: u64) -> String {
    format!("{:.2} ms", ns as f64 / 1e6)
}

fn duration(ns: u64) -> String {
    let seconds = ns as f64 / 1e9;
    if seconds >= 60.0 {
        format!("{:.1} min", seconds / 60.0)
    } else {
        format!("{seconds:.2} s")
    }
}

/// The output paths used by the standalone command.
pub fn output_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("frame.md"), dir.join("frame.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn trace_path(name: &str, body: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "voxel-frame-{name}-{}-{nanos}.csv",
            std::process::id()
        ));
        std::fs::write(&path, body).expect("write trace");
        path
    }

    #[test]
    fn passing_trace_has_no_hitches() {
        let mut body = String::from("frame,t_ns,frame_ns\n");
        for frame in 1..=120u64 {
            body.push_str(&format!("{frame},{},16600000\n", frame * 16_600_000));
        }
        let path = trace_path("pass", &body);
        let report = FrameReport::from_trace(&path, 20, FrameTarget::default()).unwrap();
        std::fs::remove_file(path).ok();

        assert_eq!(report.frames, 100);
        assert_eq!(report.hitches, 0);
        assert!(report.passed, "{report:#?}");
        assert!(report.avg_fps >= 60.0);
        assert!(report.one_percent_low_fps >= 45.0);
        assert_eq!(report.checks.len(), 3);
    }

    #[test]
    fn a_single_hitch_fails_the_one_percent_low_and_max_checks() {
        let mut body = String::from("frame,t_ns,frame_ns\n");
        for frame in 1..=100u64 {
            body.push_str(&format!("{frame},{},16600000\n", frame * 16_600_000));
        }
        body.push_str("101,1660000000,40000000\n");
        let path = trace_path("hitch", &body);
        let report = FrameReport::from_trace(&path, 0, FrameTarget::default()).unwrap();
        std::fs::remove_file(path).ok();

        assert_eq!(report.hitches, 1);
        assert!(!report.passed);
        assert!(!report.checks[1].passed, "1% low must include the hitch");
        assert!(!report.checks[2].passed, "max frame must include the hitch");
        assert!(report.markdown().contains("**Verdict: FAIL**"));
    }

    #[test]
    fn warmup_must_leave_at_least_one_sample() {
        let path = trace_path("short", "frame,t_ns,frame_ns\n1,0,16600000\n");
        let error = FrameReport::from_trace(&path, 1, FrameTarget::default()).unwrap_err();
        std::fs::remove_file(path).ok();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("warm-up"));
    }
}
