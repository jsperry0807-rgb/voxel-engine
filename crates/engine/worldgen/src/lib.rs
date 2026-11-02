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
//! - [`tile_store`] — streaming tile cache and seamless bilinear sampling.
//! - [`biomes`] — the RON biome catalogue and its validation.
//! - [`weights`] — softmax biome weights over the climate and shape axes.
//! - [`surface`] — the top-block rules: soil, beach, seabed, cliff and altitude.
//! - [`transitions`] — 1 m material dither and two-material + debris templates.
//! - [`caves`] — the 3D cave field: tunnels, chambers, shafts, galleries, fluids.
//! - [`cave_depth`] — local-surface depth and underground biome assignment.
//! - [`overhangs`] — occasional 3D shelves and arches, with floating-rock pruning.
//! - [`ores`] — depth-tiered ore veins.
//! - [`climate`] — latitude, altitude and ocean-distance climate fields.
//! - [`generator`] — the seeded generator that fills a tile.
//! - [`hydro`] — priority-flood depression fill, D8 flow and accumulation.
//! - [`rivers`] — vector river segments, width/depth/order and delta detection.
//! - [`lakes`] — lake components, spill levels and shorelines.
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

pub mod biomes;
pub mod cave_depth;
pub mod caves;
pub mod climate;
pub mod fixed;
pub mod generator;
pub mod hash;
pub mod hydro;
pub mod lakes;
pub mod macro_grid;
pub mod noise;
pub mod ores;
pub mod overhangs;
pub mod rivers;
pub mod splines;
pub mod surface;
pub mod tile;
pub mod tile_store;
pub mod transitions;
pub mod weights;

pub use biomes::{BiomeCatalog, BiomeClass, catalog};
pub use cave_depth::{CaveDepth, UndergroundClimate};
pub use caves::{CaveContext, CaveField, CaveFill, CaveShape, CaveVoxel};
pub use climate::{Climate, ClimateSample};
pub use fixed::{Coord, ONE};
pub use generator::MacroGenerator;
pub use hydro::{FlowDir, Hydrology};
pub use lakes::{Lake, Lakes};
pub use macro_grid::{Cell, TileFlags, TilePos};
pub use noise::Noise;
pub use ores::{OreField, OreKind};
pub use overhangs::OverhangField;
pub use rivers::{Mouth, River, RiverNetwork, RiverPoint};
pub use splines::{PEAK, SEA_LEVEL, surface_height};
pub use surface::{SurfaceColumn, SurfaceKind, SurfaceSample};
pub use tile::{MacroTile, TileError, TileHeader};
pub use tile_store::{Channel, DirectorySource, TileSource, TileStore};
pub use transitions::{TransitionAxis, TransitionRole, TransitionTemplate};
pub use weights::{Axes, BiomeWeights};
