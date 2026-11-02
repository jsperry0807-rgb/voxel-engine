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
//! | continentalness | 2-octave 900 km continent fbm + 3-octave 400 km coastline fbm, gain and bias calibrated for ~35% ocean | `[-1, 1]` |
//! | erosion | 3-octave fbm, 180 km base wavelength | `[0, 1]` |
//! | surface | [`surface_height`](crate::splines::surface_height) plus 24 km and 8 km valley detail | 0–12,000 m |
//! | temperature, humidity | [`Climate`](crate::climate::Climate) | `u8` |
//!
//! # Length units
//!
//! Noise wavelengths are named in **kilometres** and the coordinate is divided
//! by 1,000 before scaling. A Q16.16 factor of `ONE / 400` encodes `1/400`, but
//! the same factor in metres (`ONE * ONE / 400_000`) would encode `0.1638`,
//! i.e. a 6 m wavelength — a whole-world salt-and-pepper field. The kilometre
//! conversion is what makes `ONE / wavelength_km` meaningful, and the
//! generator tests pin the resulting landmass structure.
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
use crate::splines::{PEAK, RELIEF_WEIGHT, SEA_LEVEL, surface_height};
use crate::tile::{MacroTile, TileError, TileHeader};

/// Version of the field generator, stored in every tile.
pub const GENERATOR_VERSION: u16 = 1;

/// Version of the macro field set and its meaning.
pub const MACRO_VERSION: u16 = 1;

/// Version of the content definitions (biomes, surfaces, flora).
pub const CONTENT_VERSION: u16 = 1;

/// Salt for the continent stream.
const CONTINENT_SALT: u64 = 0x434F_4E54_0001;
/// Salt for the coastline-detail stream.
const COASTLINE_SALT: u64 = 0x4445_5441_0001;
/// Salt for the erosion stream.
const EROSION_SALT: u64 = 0x4552_4F53_0001;
/// Salt for the 24 km valley stream.
const VALLEY_SALT: u64 = 0x5348_5031_0001;
/// Salt for the 8 km valley stream.
const VALLEY_FINE_SALT: u64 = 0x5348_5032_0001;

/// Continent base wavelength: 900 km, so the 1,024 km world holds 1–7 landmasses.
const CONTINENT_WAVELENGTH_KM: i64 = 900;
/// Continent octaves: 900 km and 450 km.
const CONTINENT_OCTAVES: u32 = 2;

/// Coastline detail wavelength: 400 km.
const COASTLINE_WAVELENGTH_KM: i64 = 400;
/// Coastline octaves: 400, 200 and 100 km.
const COASTLINE_OCTAVES: u32 = 3;

/// Erosion base wavelength, 180 km.
const EROSION_WAVELENGTH_KM: i64 = 180;

/// Coarse valley wavelength, 24 km.
const VALLEY_WAVELENGTH_KM: i64 = 24;
/// Coarse valley octaves: 24, 12 and 6 km.
const VALLEY_OCTAVES: u32 = 3;
/// Coarse valley amplitude in metres.
const VALLEY_AMPLITUDE_M: i32 = 300;

/// Fine valley wavelength, 8 km.
const VALLEY_FINE_WAVELENGTH_KM: i64 = 8;
/// Fine valley octaves: 8 and 4 km.
const VALLEY_FINE_OCTAVES: u32 = 2;
/// Fine valley amplitude in metres.
const VALLEY_FINE_AMPLITUDE_M: i32 = 120;

/// Gain applied to the continent fbm.
const CONTINENT_GAIN: i32 = ONE * 24 / 10;

/// Gain applied to the coastline fbm.
const COASTLINE_GAIN: i32 = ONE * 9 / 10;

/// Share of the world that should be ocean, in percent (design target 35%).
pub const OCEAN_PERCENT: usize = 38;

/// Calibration samples per axis: 64 × 64 probes at 16 km.
const CALIBRATION_STEPS: usize = 64;

/// Width of the forced-ocean rim, in metres.
///
/// The design gives the world a deep ocean edge (20–50 km) so a boat cannot
/// leave the map. The rim caps continentalness at `-1` on the border, crosses
/// zero at half the rim and reaches `+1` (no cap) at the rim width. It also
/// guarantees that every border outlet is sea, which is what lets river
/// extraction assert that every river reaches the sea.
const EDGE_RIM_M: i64 = 40_000;

/// Q16.16 scale that turns a Q16.16 kilometre coordinate into noise space.
const fn scale_km(wavelength_km: i64) -> i32 {
    (ONE as i64 / wavelength_km) as i32
}

/// Q16.16 metres to Q16.16 kilometres.
const fn to_km(metres: Coord) -> Coord {
    metres / 1_000
}

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
    continent: Noise,
    coastline: Noise,
    erosion: Noise,
    valley: Noise,
    valley_fine: Noise,
    climate: Climate,
    /// Bias that puts [`OCEAN_PERCENT`] of the raw field at or below zero.
    ///
    /// Derived from the seed, not stored: a tile regenerates with the same
    /// bias on every machine.
    ocean_bias: i32,
}

impl MacroGenerator {
    /// Create a generator from a world seed.
    ///
    /// Calibrates the continent bias once by sampling the raw field on a
    /// 64 × 64 grid at 16 km. Without calibration the ocean share swings with
    /// the seed because the world spans barely one continent wavelength.
    pub fn new(seed: u64) -> Self {
        let mut generator = Self {
            seed,
            continent: Noise::new(seed ^ CONTINENT_SALT),
            coastline: Noise::new(seed ^ COASTLINE_SALT),
            erosion: Noise::new(seed ^ EROSION_SALT),
            valley: Noise::new(seed ^ VALLEY_SALT),
            valley_fine: Noise::new(seed ^ VALLEY_FINE_SALT),
            climate: Climate::new(seed),
            ocean_bias: 0,
        };
        generator.ocean_bias = generator.calibrated_bias();
        generator
    }

    /// The seed-derived continentalness bias.
    pub const fn ocean_bias(&self) -> i32 {
        self.ocean_bias
    }

    /// The continentalness cap imposed by the ocean rim.
    ///
    /// `-ONE` on the border, `0` at half the rim (coastline) and `ONE` at the
    /// rim width (no cap).
    fn edge_limit(x: Coord, z: Coord) -> i32 {
        let world = WORLD_EXTENT_M as i64;
        let x_metres = x >> 16;
        let z_metres = z >> 16;
        let distance = x_metres
            .min(world - 1 - x_metres)
            .min(z_metres)
            .min(world - 1 - z_metres)
            .clamp(0, EDGE_RIM_M);
        ((distance * 2 * ONE as i64) / EDGE_RIM_M - ONE as i64) as i32
    }

    /// Raw continentalness before the ocean-share bias.
    fn raw_continentalness(&self, x: Coord, z: Coord) -> i32 {
        let (x_km, z_km) = (to_km(x), to_km(z));
        let continent = self.continent.fbm_2d(
            fixed::coord_scale(x_km, scale_km(CONTINENT_WAVELENGTH_KM)),
            fixed::coord_scale(z_km, scale_km(CONTINENT_WAVELENGTH_KM)),
            CONTINENT_OCTAVES,
            2 * ONE,
            ONE / 2,
        );
        let coastline = self.coastline.fbm_2d(
            fixed::coord_scale(x_km, scale_km(COASTLINE_WAVELENGTH_KM)),
            fixed::coord_scale(z_km, scale_km(COASTLINE_WAVELENGTH_KM)),
            COASTLINE_OCTAVES,
            2 * ONE,
            ONE / 2,
        );
        fixed::mul(continent, CONTINENT_GAIN) + fixed::mul(coastline, COASTLINE_GAIN)
    }

    /// The bias that makes [`OCEAN_PERCENT`] of the sampled field negative.
    ///
    /// The ocean rim caps some samples at `-1` regardless of bias, so this is
    /// a binary search over the bias rather than a single percentile.
    /// Deterministic: fixed 32 iterations, integer comparisons only.
    fn calibrated_bias(&self) -> i32 {
        let step = WORLD_EXTENT_M as i64 / (CALIBRATION_STEPS as i64 - 1);
        let mut samples = Vec::with_capacity(CALIBRATION_STEPS * CALIBRATION_STEPS);
        for z in 0..CALIBRATION_STEPS as i64 {
            for x in 0..CALIBRATION_STEPS as i64 {
                let world_x = (x * step) << 16;
                let world_z = (z * step) << 16;
                let raw = self.raw_continentalness(world_x, world_z);
                let limit = Self::edge_limit(world_x, world_z);
                samples.push((raw, limit));
            }
        }
        let target = samples.len() as i64 * OCEAN_PERCENT as i64;
        let mut low = -2 * ONE;
        let mut high = 2 * ONE;
        for _ in 0..32 {
            let middle = low + (high - low) / 2;
            let ocean = samples
                .iter()
                .filter(|&&(raw, limit)| (raw + middle).min(limit) <= 0)
                .count() as i64;
            if ocean * 100 > target {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    /// The seed this generator was created with.
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Continentalness at a Q16.16 world position, in `[-ONE, ONE]`.
    ///
    /// The continent component decides where land is; the coastline component
    /// roughens the shore without moving the continents. Both are sampled in
    /// kilometres — see the module docs on length units.
    pub fn continentalness(&self, x: Coord, z: Coord) -> i32 {
        fixed::clamp_unit(self.raw_continentalness(x, z) + self.ocean_bias)
            .min(Self::edge_limit(x, z))
    }

    /// Erosion at a Q16.16 world position, in `[0, ONE]`.
    pub fn erosion(&self, x: Coord, z: Coord) -> i32 {
        let raw = self.erosion.fbm_2d(
            fixed::coord_scale(to_km(x), scale_km(EROSION_WAVELENGTH_KM)),
            fixed::coord_scale(to_km(z), scale_km(EROSION_WAVELENGTH_KM)),
            3,
            2 * ONE,
            ONE / 2,
        );
        (ONE / 2 + raw).clamp(0, ONE)
    }

    /// Surface elevation at a Q16.16 world position, in Q16.16 metres.
    pub fn surface(&self, x: Coord, z: Coord) -> i32 {
        let continentalness = self.continentalness(x, z);
        let erosion = self.erosion(x, z);
        self.surface_for(x, z, continentalness, erosion)
    }

    /// Surface elevation when the shape inputs are already sampled.
    ///
    /// Adds the valley detail to the shape spline result. The detail is
    /// suppressed by the relief weight, so ocean floor and beaches stay flat
    /// and only the land interior gets valleys. Without this term the surface
    /// is a dome and water drains radially to the coast with no catchment.
    pub fn surface_for(&self, x: Coord, z: Coord, continentalness: i32, erosion: i32) -> i32 {
        let base = surface_height(continentalness, erosion) as i64;
        let weight = RELIEF_WEIGHT.eval(continentalness);
        let detail = self.valley_detail(x, z, weight);
        (base + detail as i64).clamp(0, PEAK as i64) as i32
    }

    /// Signed valley detail at a position, already weighted for land.
    fn valley_detail(&self, x: Coord, z: Coord, weight: i32) -> i32 {
        let (x_km, z_km) = (to_km(x), to_km(z));
        let coarse = self.valley.fbm_2d(
            fixed::coord_scale(x_km, scale_km(VALLEY_WAVELENGTH_KM)),
            fixed::coord_scale(z_km, scale_km(VALLEY_WAVELENGTH_KM)),
            VALLEY_OCTAVES,
            2 * ONE,
            ONE / 2,
        );
        let fine = self.valley_fine.fbm_2d(
            fixed::coord_scale(x_km, scale_km(VALLEY_FINE_WAVELENGTH_KM)),
            fixed::coord_scale(z_km, scale_km(VALLEY_FINE_WAVELENGTH_KM)),
            VALLEY_FINE_OCTAVES,
            2 * ONE,
            ONE / 2,
        );
        let metres = fixed::mul(coarse, VALLEY_AMPLITUDE_M << 16)
            + fixed::mul(fine, VALLEY_FINE_AMPLITUDE_M << 16);
        fixed::mul(metres, weight)
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
                let surface = self.surface_for(world_x, world_z, continentalness, erosion);
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
