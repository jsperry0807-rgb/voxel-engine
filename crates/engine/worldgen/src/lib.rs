//! Deterministic world generation (M2.5).
//!
//! The generation path is **integer-only**: every coordinate, hash, gradient
//! dot product and interpolation step is computed with wrap-around integer
//! arithmetic, so the same seed produces the same bytes on every platform and
//! for every worker count. Floating point appears only at the boundary, in
//! [`fixed`] conversion helpers for callers and tests.
//!
//! Modules:
//!
//! - [`fixed`] — Q16.16 coordinates (48.16) and values (16.16).
//! - [`hash`] — `splitmix64` finaliser and lattice hashing; no ordered RNG.
//! - [`noise`] — 2D/3D gradient noise and fractal sums over it.
//! - [`splines`] — continentalness/erosion shape tables.
//! - [`macro_grid`] — 128 m grid geometry, cells and flags.
//! - [`tile`] — the 16 km tile binary format.
//! - [`climate`] — latitude, altitude and ocean-distance climate fields.
//! - [`generator`] — the seeded generator that fills a tile.
//!
//! # Example
//!
//! ```
//! use voxel_worldgen::fixed::{value_to_f32, coord_from_f32};
//! use voxel_worldgen::Noise;
//!
//! let noise = Noise::new(1);
//! // 12.5 m east, -3.25 m north, in fixed-point metres.
//! let x = coord_from_f32(12.5);
//! let y = coord_from_f32(-3.25);
//! let value = noise.value_2d(x, y);
//! assert!((-1.0..=1.0).contains(&value_to_f32(value)));
//! ```

pub mod climate;
pub mod fixed;
pub mod generator;
pub mod hash;
pub mod macro_grid;
pub mod noise;
pub mod splines;
pub mod tile;

pub use climate::{Climate, ClimateSample};
pub use fixed::{Coord, ONE};
pub use generator::MacroGenerator;
pub use macro_grid::{Cell, TileFlags, TilePos};
pub use noise::Noise;
pub use splines::{PEAK, SEA_LEVEL, surface_height};
pub use tile::{MacroTile, TileError, TileHeader};
