//! M1 gate measurements.
//!
//! The gate tasks (M1-35…M1-43) share one rule: every number is measured on
//! the frozen fixture, not on a process-local synthetic pattern. Each module
//! exposes a serializable report and a CLI subcommand so the gate report
//! generator (M1-39) can consume the same data a human sees.
//!
//! - [`memory`] — per-chunk DAG, overlay, light, probe and mesh bytes (M1-35).
//! - [`residency`] — loaded chunk count for the configured view/band (M1-36).

pub mod memory;
pub mod residency;
