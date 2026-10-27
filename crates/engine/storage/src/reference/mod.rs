//! Slow, exact reference implementations used as test oracles.
//!
//! These modules are deliberately written for correctness and readability, not
//! throughput. M2 and M5 compare their production passes against them; a
//! reference is allowed to take seconds where the production path takes
//! milliseconds.
//!
//! - [`cull`] — hierarchical frustum traversal (M1-32).
//! - [`light`] — full-pass block-light flood fill (M1-29).
//! - [`mesh`] — count-only greedy mesher (M1-31).
//! - [`occlusion`] — camera-seeded block-occlusion flood fill (M1-33).
//! - [`probe`] — deterministic SH4 probe bake (M1-30).

pub mod cull;
pub mod light;
pub mod mesh;
pub mod occlusion;
pub mod probe;
