//! Benchmarks for the voxel engine.
//!
//! The suite measures four lenses, because no single number answers "is this
//! fast?":
//!
//! - **Wall clock** (`benches/*.rs`, Criterion): statistical nanosecond
//!   timings and throughput for hot code paths.
//! - **Memory** ([`mem`], [`suite`]): live heap bytes, allocation churn and
//!   peak heap for every structure, plus process RSS and peak RSS.
//! - **CPU** ([`proc`], [`suite`]): process user+system time per operation,
//!   CPU utilization, and allocations per operation.
//! - **Hardware counters** (`benches/iai.rs`): deterministic instruction,
//!   cache-miss and branch-miss counts through Valgrind/Callgrind.
//!
//! Run all software lenses with:
//!
//! ```text
//! cargo run --release -p voxel-bench -- all
//! ```
//!
//! or use `scripts/bench.sh` to also drive Criterion, iai-callgrind and
//! `perf stat`.
//!
//! [`save_scan`] is the M1-27 reference reader for a save directory; it is not
//! a benchmark and does not run as part of `all`. [`fixture`] is the M1-28
//! frozen world that reader and the M1 gate measure; `fixture verify` checks
//! its hashes and rebuilds it from the manifest recipe.

pub mod fixture;
pub mod fixtures;
pub mod history;
pub mod layout;
pub mod mem;
pub mod patterns;
pub mod proc;
pub mod report;
pub mod save_scan;
pub mod suite;

pub use mem::CountingAllocator;
