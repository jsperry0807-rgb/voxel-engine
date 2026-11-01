//! The macro tile generator: seeded fields sampled at 128 m.
//!
//! One [`MacroGenerator`] is a seed plus three noise streams (continentalness,
//! erosion, climate). A tile is a pure function of `(seed, position)`: the
//! same seed regenerates the same bytes on every worker and every machine.
//!
//! # Fields
//!
//! | Field | Source | Range |
//! |---|---|---|
//! | continentalness | 4-octave fbm, 400 km base wavelength, gain and bias calibrated for ~35% ocean | `[-1, 1]` |
//! | erosion | 3-octave fbm, 180 km base wavelength | `[0, 1]` |
//! | surface | [`surface_height`](crate::splines::surface_height) | 0–12,000 m |
//! | temperature, humidity | [`Climate`](crate::climate::Climate) | `u8` |
//!
//! # Ocean distance before hydrology
//!
//! Humidity wants a distance to the nearest ocean. The real distance field is
//! a product of hydrology (M2.5-06/07), which runs after the macro map. Until
//! then this generator uses a **continentalness ramp**: `distance = max(0, c) *
//! 512 km`. It is monotone, so coasts are wet and interiors dry, and every
//! tile is self-contained (no neighbouring tiles needed). M2.5-06 refines the
//! channel in place and bumps the macro version.
//!
//! # Flags
//!
//! `WATER` is set at or below sea level, `COAST` in the 16 m band above it.
//! `RIVER` and `LAKE` wait for hydrology and stay clear.
//!
//! # Content hash
//!
//! [`MacroTile::content_hash`](crate::tile::MacroTile::content_hash) hashes the
//! identity header and the payload. The CLI aggregates the 4,096 tile hashes
//! into one world hash so a full generation run can be compared across
//! machines without storing 384 MB.

use crate::climate::{Climate, unit_to_u8};
use crate::fixed::{self, Coord, ONE};
use crate::macro_grid::{
    CELL_SIZE_M, Cell, TILE_CELLS_USIZE, TileFlags, TilePos, WORLD_EXTENT_M, height_from_q16,
};
use crate::noise::Noise;
use crate::splines::{SEA_LEVEL, surface_height};
use crate::tile::{MacroTile, TileError, TileHeader};

/// Version of the field generator, stored in every tile.
pub const GENERATOR_VERSION: u16 = 1;

/// Version of the macro field set and its meaning.
pub const MACRO_VERSION: u16 = 1;

/// Version of the content definitions (biomes, surfaces, flora).
pub const CONTENT_VERSION: u16 = 1;

/// Salt for the continentalness stream.
const CONTINENTAL_SALT: u64 = 0x434F_4E54_0001;
/// Salt for the erosion stream.
const EROSION_SALT: u64 = 0x4552_4F53_0001;

/// Continentalness base wavelength: 400 km, in Q16.16 cells per metre.
const CONTINENTAL_SCALE: i32 = ((ONE as i64 * ONE as i64) / 400_000) as i32;

/// Erosion base wavelength: 180 km.
const EROSION_SCALE: i32 = ((ONE as i64 * ONE as i64) / 180_000) as i32;

/// Gain applied to the raw continentalness fbm.
const CONTINENTAL_GAIN: i32 = ONE * 2;

/// Bias applied after the gain; calibrated so ~35% of the world is ocean.
const CONTINENTAL_BIAS: i32 = ONE * 13 / 100;

/// Half the world, the ocean-distance ramp's maximum, in metres.
const HALF_WORLD_M: i64 = WORLD_EXTENT_M as i64 / 2;

/// Sea level in the tile's 0.25 m units.
pub const SEA_LEVEL_UNITS: u16 = height_from_q16(SEA_LEVEL);

/// Width of the coastal band above sea level, in 0.25 m units.
pub const COAST_BAND_UNITS: u16 = height_from_q16(16 << 16);

/// The seeded macro-field generator.
#[derive(Clone, Copy, Debug)]
pub struct MacroGenerator {
    seed: u64,
    continentalness: Noise,
    erosion: Noise,
    climate: Climate,
}

impl MacroGenerator {
    /// Create a generator from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            continentalness: Noise::new(seed ^ CONTINENTAL_SALT),
            erosion: Noise::new(seed ^ EROSION_SALT),
            climate: Climate::new(seed),
        }
    }

    /// The seed this generator was created with.
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Continentalness at a Q16.16 world position, in `[-ONE, ONE]`.
    pub fn continentalness(&self, x: Coord, z: Coord) -> i32 {
        let raw = self.continentalness.fbm_2d(
            fixed::coord_scale(x, CONTINENTAL_SCALE),
            fixed::coord_scale(z, CONTINENTAL_SCALE),
            4,
            2 * ONE,
            ONE / 2,
        );
        fixed::clamp_unit(fixed::mul(raw, CONTINENTAL_GAIN) + CONTINENTAL_BIAS)
    }

    /// Erosion at a Q16.16 world position, in `[0, ONE]`.
    pub fn erosion(&self, x: Coord, z: Coord) -> i32 {
        let raw = self.erosion.fbm_2d(
            fixed::coord_scale(x, EROSION_SCALE),
            fixed::coord_scale(z, EROSION_SCALE),
            3,
            2 * ONE,
            ONE / 2,
        );
        (ONE / 2 + raw).clamp(0, ONE)
    }

    /// Surface elevation at a Q16.16 world position, in Q16.16 metres.
    pub fn surface(&self, x: Coord, z: Coord) -> i32 {
        surface_height(self.continentalness(x, z), self.erosion(x, z))
    }

    /// Temperature at a position, from the same climate stream the tiles use.
    pub fn temperature(&self, x: Coord, z: Coord, surface: i32) -> u8 {
        self.climate.temperature(x, z, surface)
    }

    /// Humidity at a position and continentalness.
    ///
    /// The ocean distance is the pre-hydrology continentalness ramp; see the
    /// module docs.
    pub fn humidity(&self, x: Coord, z: Coord, continentalness: i32) -> u8 {
        self.climate
            .humidity(x, z, Self::ocean_distance(continentalness))
    }

    /// The pre-hydrology ocean distance for a continentalness value, in
    /// Q16.16 metres.
    pub fn ocean_distance(continentalness: i32) -> Coord {
        // Q16.16 unitless × metres leaves Q16.16 metres: no shift needed.
        continentalness.max(0) as i64 * HALF_WORLD_M
    }

    /// Generate one tile. The result depends only on the seed and `pos`.
    pub fn generate_tile(&self, pos: TilePos) -> Result<MacroTile, TileError> {
        let mut tile = MacroTile::new(TileHeader::new(
            pos,
            self.seed,
            GENERATOR_VERSION,
            MACRO_VERSION,
            CONTENT_VERSION,
        ))?;
        let (origin_x, origin_z) = pos.origin_m();
        let half_cell = CELL_SIZE_M as i64 / 2;

        for cell_z in 0..TILE_CELLS_USIZE {
            // Cell centres, not corners: a 128 m cell's value represents its
            // middle, and centring avoids a half-cell shift at tile borders.
            let world_z = ((origin_z + cell_z as i64 * CELL_SIZE_M as i64) + half_cell) << 16;
            for cell_x in 0..TILE_CELLS_USIZE {
                let world_x = ((origin_x + cell_x as i64 * CELL_SIZE_M as i64) + half_cell) << 16;
                let continentalness = self.continentalness(world_x, world_z);
                let erosion = self.erosion(world_x, world_z);
                let surface = surface_height(continentalness, erosion);
                let height = height_from_q16(surface);
                let temperature = self.temperature(world_x, world_z, surface);
                let humidity = self.humidity(world_x, world_z, continentalness);
                tile.set_cell(
                    cell_x,
                    cell_z,
                    Cell {
                        height,
                        temperature,
                        humidity,
                        erosion: unit_to_u8(erosion),
                        flags: flags_for(height),
                    },
                );
            }
        }
        Ok(tile)
    }
}

/// Water and coast flags for a surface height in tile units.
pub fn flags_for(height: u16) -> TileFlags {
    if height <= SEA_LEVEL_UNITS {
        TileFlags::WATER
    } else if height - SEA_LEVEL_UNITS <= COAST_BAND_UNITS {
        TileFlags::COAST
    } else {
        TileFlags::NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_generator_flags_follow_sea_level() {
        assert_eq!(flags_for(0), TileFlags::WATER);
        assert_eq!(flags_for(SEA_LEVEL_UNITS), TileFlags::WATER);
        assert_eq!(flags_for(SEA_LEVEL_UNITS + 1), TileFlags::COAST);
        assert_eq!(
            flags_for(SEA_LEVEL_UNITS + COAST_BAND_UNITS),
            TileFlags::COAST
        );
        assert_eq!(
            flags_for(SEA_LEVEL_UNITS + COAST_BAND_UNITS + 1),
            TileFlags::NONE
        );
        assert_eq!(flags_for(u16::MAX), TileFlags::NONE);
    }

    #[test]
    fn macro_generator_ocean_distance_is_a_ramp() {
        assert_eq!(MacroGenerator::ocean_distance(-ONE), 0);
        assert_eq!(MacroGenerator::ocean_distance(0), 0);
        assert_eq!(
            MacroGenerator::ocean_distance(ONE),
            (WORLD_EXTENT_M as i64 / 2) << 16
        );
        assert!(MacroGenerator::ocean_distance(ONE / 2) < MacroGenerator::ocean_distance(ONE));
    }
}
