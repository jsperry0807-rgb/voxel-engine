//! Benchmarks for the voxel engine.
//!
//! The suite measures five lenses, because no single number answers "is this
//! fast?":
//!
//! - **Wall clock** (`benches/*.rs`, Criterion): statistical nanosecond
//!   timings and throughput for hot code paths.
//! - **Frame time** ([`frame`]): average, 1% low, p99 and maximum hitch over a
//!   viewer frame trace, judged against the 60 Hz target (M3-09/M3-10).
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
//! The frame-time lens needs a trace from a real viewer run:
//!
//! ```text
//! VOXEL_FRAME_PROFILE=target/frames.csv cargo run --release -p voxel-viewer
//! cargo run --release -p voxel-bench -- frame --input target/frames.csv
//! ```
//!
//! or use `scripts/bench.sh` to also drive Criterion, iai-callgrind and
//! `perf stat`; set `FRAMES=<path>` there to attach a trace to the main report.
//!
//! [`save_scan`] is the M1-27 reference reader for a save directory; it is not
//! a benchmark and does not run as part of `all`. [`fixture`] is the M1-28
//! frozen world that reader and the M1 gate measure; `fixture verify` checks
//! its hashes and rebuilds it from the manifest recipe. [`gate`] holds the
//! pre-registered M1 measurements over that fixture (M1-35 onward).

pub mod fixture;
pub mod fixtures;
pub mod frame;
pub mod gate;
pub mod history;
pub mod image_diff;
pub mod layout;
pub mod mem;
pub mod patterns;
pub mod proc;
pub mod report;
pub mod save_scan;
pub mod suite;

pub use mem::CountingAllocator;
