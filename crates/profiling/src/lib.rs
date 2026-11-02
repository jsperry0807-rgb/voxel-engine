//! Frame-time sampling shared by the viewer and the benchmark suite.
//!
//! A frame-time trace is the only measurement that can answer the M3-09/M3-10
//! questions: an average frame rate hides a 200 ms hitch, and a single maximum
//! hides a run that spends most of its time at 20 fps. The viewer writes one
//! interval per presented frame; `voxel-bench frame` reads the same file and
//! applies the targets frozen in `docs/reference-machine.md`.
//!
//! # Trace format
//!
//! The viewer writes [`TRACE_HEADER`] followed by one row per presented frame:
//!
//! ```text
//! frame,t_ns,frame_ns
//! 1,16650000,16650000
//! 2,33300000,16650000
//! ```
//!
//! `frame` starts at 1, `t_ns` is monotonic time since profiling began, and
//! `frame_ns` is the interval since the previous frame start. Blank lines and
//! lines beginning with `#` are ignored. A one-column file of frame intervals
//! is also accepted, so a trace captured by another tool can be analysed
//! without conversion.
//!
//! The statistics mirror the rule on the reference machine: average 60 fps,
//! 1% low 45 fps, and any frame over 33.3 ms is a hitch.

use std::fmt;

/// The average frame rate the reference machine must sustain.
pub const TARGET_FPS: f64 = 60.0;
/// The 1% low floor from the reference machine.
pub const ONE_PERCENT_LOW_FPS: f64 = 45.0;
/// The 60 Hz frame budget in nanoseconds.
pub const FRAME_BUDGET_NS: u64 = 16_666_667;
/// A frame slower than this is a hitch: two 60 Hz budgets (33.3 ms).
pub const HITCH_NS: u64 = 33_333_333;
/// The header the viewer writes for a frame trace.
pub const TRACE_HEADER: &str = "frame,t_ns,frame_ns";

/// Summary of one continuous frame trace.
///
/// Percentiles use nearest rank, so a short trace still has a defined 1% low.
/// The 1% low is the frame rate of the slowest `ceil(1%)` frames, not the p99
/// frame time; the two are reported separately because both are useful.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// Measured frames after any warm-up samples were removed.
    pub frames: u64,
    /// Sum of every measured interval.
    pub duration_ns: u64,
    /// Arithmetic mean frame interval.
    pub mean_ns: u64,
    /// Nearest-rank median frame interval.
    pub p50_ns: u64,
    /// Nearest-rank 95th percentile frame interval.
    pub p95_ns: u64,
    /// Nearest-rank 99th percentile frame interval.
    pub p99_ns: u64,
    /// Longest frame interval.
    pub max_ns: u64,
    /// `1e9 / mean_ns`; zero for an empty trace.
    pub avg_fps: f64,
    /// Frame rate of the slowest `ceil(1%)` frames; zero for an empty trace.
    pub one_percent_low_fps: f64,
    /// Frames slower than the 60 Hz budget.
    pub over_budget: u64,
    /// Frames slower than [`HITCH_NS`].
    pub hitches: u64,
}

impl FrameStats {
    /// Summarizes raw frame intervals in nanoseconds.
    ///
    /// Zero-length intervals are treated as invalid samples by [`parse_trace`]
    /// and are not expected here; callers that build a trace by hand should
    /// filter them before calling this function.
    pub fn from_intervals(intervals: &[u64]) -> Self {
        if intervals.is_empty() {
            return Self::default();
        }
        let mut sorted = intervals.to_vec();
        sorted.sort_unstable();
        let frames = sorted.len() as u64;
        let duration_ns = sorted.iter().copied().sum::<u64>();
        let mean_ns = duration_ns / frames;
        let p50_ns = percentile(&sorted, 50);
        let p95_ns = percentile(&sorted, 95);
        let p99_ns = percentile(&sorted, 99);
        let max_ns = *sorted.last().expect("non-empty trace");
        let worst = (sorted.len() as f64 * 0.01).ceil() as usize;
        let worst = worst.clamp(1, sorted.len());
        let worst_mean_ns = sorted[sorted.len() - worst..]
            .iter()
            .map(|&ns| ns as f64)
            .sum::<f64>()
            / worst as f64;
        Self {
            frames,
            duration_ns,
            mean_ns,
            p50_ns,
            p95_ns,
            p99_ns,
            max_ns,
            avg_fps: fps(mean_ns as f64),
            one_percent_low_fps: fps(worst_mean_ns),
            over_budget: sorted.iter().filter(|&&ns| ns > FRAME_BUDGET_NS).count() as u64,
            hitches: sorted.iter().filter(|&&ns| ns > HITCH_NS).count() as u64,
        }
    }
}

fn fps(interval_ns: f64) -> f64 {
    if interval_ns > 0.0 {
        1e9 / interval_ns
    } else {
        0.0
    }
}

/// Nearest-rank percentile: the smallest value whose rank reaches `p`.
fn percentile(sorted: &[u64], p: u32) -> u64 {
    debug_assert!(!sorted.is_empty());
    let rank = (sorted.len() * p as usize).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

/// A malformed frame trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceError {
    /// One-based source line.
    pub line: usize,
    /// What was wrong on that line.
    pub message: String,
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for TraceError {}

/// Parses a frame trace into non-zero frame intervals in nanoseconds.
///
/// A header containing `frame_ns` (or `interval_ns`) selects that column; a
/// headerless row with several fields uses the last field, and a single-column
/// file uses that field. Zero intervals are dropped: they are the first frame
/// of a capture, not a frame that took no time.
pub fn parse_trace(text: &str) -> Result<Vec<u64>, TraceError> {
    let mut intervals = Vec::new();
    let mut column: Option<usize> = None;
    let mut first_row = true;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let row = raw.trim();
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = row.split(',').map(str::trim).collect();

        if first_row {
            first_row = false;
            if let Some(index) = fields
                .iter()
                .position(|field| matches!(*field, "frame_ns" | "interval_ns"))
            {
                column = Some(index);
                continue;
            }
            if fields.iter().any(|field| field.parse::<u64>().is_err()) {
                return Err(TraceError {
                    line,
                    message: "header has no `frame_ns` or `interval_ns` column".to_owned(),
                });
            }
            // Headerless data: a single column is the interval itself; with
            // several columns prefer the last, matching the documented layout.
            column = Some(fields.len().saturating_sub(1));
        }

        let index = column.expect("the first data row selects a column");
        let Some(field) = fields.get(index) else {
            return Err(TraceError {
                line,
                message: format!(
                    "expected at least {} column(s), found {}",
                    index + 1,
                    fields.len()
                ),
            });
        };
        let ns = field.parse::<u64>().map_err(|_| TraceError {
            line,
            message: format!("`{field}` is not a frame interval in nanoseconds"),
        })?;
        if ns > 0 {
            intervals.push(ns);
        }
    }

    Ok(intervals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_cover_small_samples() {
        let stats = FrameStats::from_intervals(&[10, 20, 30, 40]);
        assert_eq!(stats.frames, 4);
        assert_eq!(stats.p50_ns, 20);
        assert_eq!(stats.p95_ns, 40);
        assert_eq!(stats.p99_ns, 40);
        assert_eq!(stats.max_ns, 40);
    }

    #[test]
    fn one_percent_low_uses_the_slowest_frames() {
        // 100 samples, one 100 ms outlier: ceil(1%) = 1, so the 1% low is that
        // outlier, not a percentile interpolated away by the other 99 frames.
        let mut intervals = vec![16_000_000; 99];
        intervals.push(100_000_000);
        let stats = FrameStats::from_intervals(&intervals);
        assert_eq!(stats.one_percent_low_fps, 10.0);
        assert_eq!(stats.p99_ns, 16_000_000);
        assert_eq!(stats.max_ns, 100_000_000);
        assert_eq!(stats.hitches, 1);
        assert_eq!(stats.over_budget, 1);
    }

    #[test]
    fn empty_trace_has_zero_fps() {
        let stats = FrameStats::from_intervals(&[]);
        assert_eq!(stats.frames, 0);
        assert_eq!(stats.avg_fps, 0.0);
        assert_eq!(stats.one_percent_low_fps, 0.0);
    }

    #[test]
    fn parses_the_documented_header_and_ignores_the_first_frame() {
        let trace = "\
# captured by VOXEL_FRAME_PROFILE
frame,t_ns,frame_ns
0,0,0
1,16600000,16600000
2,33200000,16600000
";
        assert_eq!(parse_trace(trace).unwrap(), vec![16_600_000, 16_600_000]);
    }

    #[test]
    fn accepts_a_one_column_trace() {
        let trace = "16600000\n17000000\n";
        assert_eq!(parse_trace(trace).unwrap(), vec![16_600_000, 17_000_000]);
    }

    #[test]
    fn accepts_a_headerless_row_and_uses_the_last_column() {
        let trace = "1,16600000\n2,17000000\n";
        assert_eq!(parse_trace(trace).unwrap(), vec![16_600_000, 17_000_000]);
    }

    #[test]
    fn rejects_a_header_without_an_interval_column() {
        let error = parse_trace("frame,t_ns\n1,16600000\n").unwrap_err();
        assert_eq!(error.line, 1);
        assert!(error.message.contains("frame_ns"));
    }

    #[test]
    fn rejects_a_malformed_interval() {
        let error = parse_trace("frame,t_ns,frame_ns\n1,16600000,nope\n").unwrap_err();
        assert_eq!(error.line, 2);
        assert!(error.message.contains("not a frame interval"));
    }
}
