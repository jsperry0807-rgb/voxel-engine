//! M1 gate measurements.
//!
//! The gate tasks (M1-35…M1-43) share one rule: every number is measured on
//! the frozen fixture, not on a process-local synthetic pattern. Each module
//! exposes a serializable report and a CLI subcommand so the gate report
//! generator (M1-39) can consume the same data a human sees.
//!
//! - [`memory`] — per-chunk DAG, overlay, light, probe and mesh bytes (M1-35).
//! - [`residency`] — loaded chunk count for the configured view/band (M1-36).
//! - [`edits`] — per-edit latency split into fixed and dirty-region terms (M1-37).
//! - [`probes`] — incremental probe-update latency per edited chunk (M1-38).
//! - [`report`] — the twelve pre-registered rows, verdicts and fallback (M1-39).

pub mod edits;
pub mod memory;
pub mod probes;
pub mod report;
pub mod residency;
mod stat;
