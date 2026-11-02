//! Climate fields: compressed latitude, ocean distance and altitude lapse.
//!
//! The macro tile stores temperature and humidity as `u8` (0 coldest/driest,
//! 255 hottest/wettest). This module turns three inputs into that pair:
//!
//! 1. **Latitude** — the world is 1,024,000 m from pole to pole and stands in
//!    for a planet's 10,000 km hemisphere, so climate zones are ~10×
//!    compressed. The equator is at `z = 512,000 m`; the north and south edges
//!    are the poles. [`latitude_warmth`] is a non-increasing spline over the
//!    normalized polar distance.
//! 2. **Altitude** — a lapse rate cools the surface above sea level, so peaks
//!    are cold regardless of latitude (the design's universal rock-to-snow
//!    above the treeline). Below sea level the lapse is zero: a water body is
//!    the temperature of the air above it, not of its floor.
//! 3. **Ocean distance** — humidity is a non-increasing spline over the
//!    distance to the nearest ocean cell; coasts are wet, interiors dry.
//!    M2.5-05 supplies the distance field, M2.5-06 refines it with hydrology.
//!
//! Temperature also carries a small 8 km-wavelength noise term (`±0.06` of the
//! channel range) so latitude bands and coast bands do not read as straight
//! contours. The noise is seeded per climate instance and sampled as a pure
//! function of world position, like every other field.
//!
//! All table arithmetic is Q16.16 integer; only the final conversion to `u8`
//! rounds, half up.

use crate::fixed::{self, Coord, ONE};
use crate::macro_grid::WORLD_EXTENT_M;
use crate::noise::Noise;
use crate::splines::{PEAK, SEA_LEVEL, Spline};

/// Amplitude of the climate noise, Q16.16 (0.06).
pub const VARIATION_AMPLITUDE: i32 = ONE * 6 / 100;

/// Noise coordinate scale: one lattice cell per 8,192 m.
const VARIATION_SCALE: i32 = ONE / 8_000;

/// Salt for the temperature noise stream.
const TEMPERATURE_SALT: u64 = 0x434C_494D_0001;
/// Salt for the humidity noise stream.
const HUMIDITY_SALT: u64 = 0x434C_494D_0002;

/// Equator position, Q16.16 metres: half the world.
pub const EQUATOR: Coord = (WORLD_EXTENT_M as i64 / 2) << 16;

/// Normalized polar distance (`0` equator, `ONE` pole) to warmth.
///
/// A cosine-shaped falloff, sampled as a monotone table so it stays integer
/// and can be built in a `const`. The curve is symmetric about the equator
/// because the input is the absolute polar distance.
pub const LATITUDE_WARMTH: Spline<8> = Spline::new([
    (0, ONE),
    (ONE * 15 / 100, ONE * 98 / 100),
    (ONE * 30 / 100, ONE * 92 / 100),
    (ONE * 45 / 100, ONE * 80 / 100),
    (ONE * 60 / 100, ONE * 62 / 100),
    (ONE * 75 / 100, ONE * 42 / 100),
    (ONE * 90 / 100, ONE * 18 / 100),
    (ONE, 0),
]);

/// Surface height above sea level to temperature loss.
///
/// Zero at sea level, `0.38` at the midpoint, `0.80` at the 12,000 m peak. The
/// table is non-decreasing, and the evaluator clamps below the first knot, so
/// the ocean floor loses nothing. Even an equatorial peak lands near `0.20`,
/// which is cold enough for the surface rules to treat it as snow.
pub const ALTITUDE_LAPSE: Spline<3> = Spline::new([
    (SEA_LEVEL, 0),
    (SEA_LEVEL + (PEAK - SEA_LEVEL) / 2, ONE * 38 / 100),
    (PEAK, ONE * 80 / 100),
]);

/// Ocean distance (Q16.16 **kilometres**) to humidity.
///
/// Wet at the coast, dry inland. Beyond 512 km the last knot is used; the
/// world cannot be farther from an ocean than that.
pub const OCEAN_HUMIDITY: Spline<6> = Spline::new([
    (0, ONE * 95 / 100),
    (10 << 16, ONE * 85 / 100),
    (30 << 16, ONE * 70 / 100),
    (80 << 16, ONE * 50 / 100),
    (200 << 16, ONE * 32 / 100),
    (512 << 16, ONE * 20 / 100),
]);

/// Temperature and humidity for one macro cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClimateSample {
    /// 0 coldest, 255 hottest.
    pub temperature: u8,
    /// 0 driest, 255 wettest.
    pub humidity: u8,
}

/// The seeded climate field.
///
/// One instance owns the two noise streams; the seed is the only input.
#[derive(Clone, Copy, Debug)]
pub struct Climate {
    temperature_noise: Noise,
    humidity_noise: Noise,
}

impl Climate {
    /// Create a climate field from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self {
            temperature_noise: Noise::new(seed ^ TEMPERATURE_SALT),
            humidity_noise: Noise::new(seed ^ HUMIDITY_SALT),
        }
    }

    /// Temperature at a world position and Q16.16 surface elevation.
    ///
    /// `x` and `z` are Q16.16 metres; `surface` is the shape spline's
    /// elevation (`crate::splines::surface_height`).
    pub fn temperature(&self, x: Coord, z: Coord, surface: i32) -> u8 {
        let variation = variation(self.temperature_noise, x, z);
        unit_to_u8(temperature_base(z, surface) + variation)
    }

    /// Humidity at a world position and a Q16.16 distance to the nearest
    /// ocean, in metres.
    pub fn humidity(&self, x: Coord, z: Coord, ocean_distance: Coord) -> u8 {
        let variation = variation(self.humidity_noise, x, z);
        unit_to_u8(humidity_base(ocean_distance) + variation)
    }

    /// Both channels for one cell.
    pub fn sample(&self, x: Coord, z: Coord, surface: i32, ocean_distance: Coord) -> ClimateSample {
        ClimateSample {
            temperature: self.temperature(x, z, surface),
            humidity: self.humidity(x, z, ocean_distance),
        }
    }
}

/// Warmth at a world `z` coordinate: `ONE` at the equator, `0` at the poles.
pub fn latitude_warmth(z: Coord) -> i32 {
    let distance = (z - EQUATOR).abs();
    let polar = ((distance << 16) / EQUATOR) as i32;
    LATITUDE_WARMTH.eval(polar)
}

/// Temperature without the noise term, Q16.16.
///
/// Used by map renders and by the monotonicity tests; the stored channel adds
/// the 8 km variation and converts to `u8`.
pub fn temperature_base(z: Coord, surface: i32) -> i32 {
    latitude_warmth(z) - ALTITUDE_LAPSE.eval(surface)
}

/// Humidity without the noise term, Q16.16, from a Q16.16 metre distance.
pub fn humidity_base(ocean_distance: Coord) -> i32 {
    let metres = ocean_distance.clamp(0, (WORLD_EXTENT_M as i64) << 16);
    // Q16.16 metres / 1,000 = Q16.16 kilometres, which fits in an i32.
    let distance = (metres / 1_000) as i32;
    OCEAN_HUMIDITY.eval(distance)
}

/// Convert a Q16.16 value to a `u8` channel, clamping and rounding half up.
pub fn unit_to_u8(value: i32) -> u8 {
    let clamped = value.clamp(0, ONE) as i64;
    ((clamped * 255 + ONE as i64 / 2) / ONE as i64) as u8
}

/// A signed Q16.16 variation in `[-VARIATION_AMPLITUDE, VARIATION_AMPLITUDE]`.
fn variation(noise: Noise, x: Coord, z: Coord) -> i32 {
    let scaled_x = fixed::coord_scale(x, VARIATION_SCALE);
    let scaled_z = fixed::coord_scale(z, VARIATION_SCALE);
    fixed::mul(noise.value_2d(scaled_x, scaled_z), VARIATION_AMPLITUDE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climate_unit_conversion_rounds_half_up() {
        assert_eq!(unit_to_u8(0), 0);
        assert_eq!(unit_to_u8(ONE), 255);
        assert_eq!(unit_to_u8(ONE / 2), 128);
        assert_eq!(unit_to_u8(-ONE), 0);
        assert_eq!(unit_to_u8(2 * ONE), 255);
        assert_eq!(unit_to_u8(ONE / 255), 1);
        assert_eq!(unit_to_u8(ONE / 1_000), 0);
    }

    #[test]
    fn climate_latitude_is_exact_at_the_equator_and_poles() {
        assert_eq!(latitude_warmth(EQUATOR), ONE);
        assert_eq!(latitude_warmth(0), 0);
        assert_eq!(latitude_warmth((WORLD_EXTENT_M as i64) << 16), 0);
    }

    #[test]
    fn climate_noise_streams_are_salted() {
        let climate = Climate::new(7);
        assert_ne!(climate.temperature_noise, climate.humidity_noise);
        // Deterministic construction for the same seed.
        let again = Climate::new(7);
        assert_eq!(
            climate.temperature_noise.seed(),
            again.temperature_noise.seed()
        );
    }
}
