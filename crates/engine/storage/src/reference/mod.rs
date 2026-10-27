//! Slow, exact reference implementations used as test oracles.
//!
//! These modules are deliberately written for correctness and readability, not
//! throughput. M2 and M5 compare their production passes against them; a
//! reference is allowed to take seconds where the production path takes
//! milliseconds.
//!
//! - [`light`] — full-pass block-light flood fill (M1-29).
//! - [`probe`] — deterministic SH4 probe bake (M1-30).

pub mod light;
pub mod probe;
