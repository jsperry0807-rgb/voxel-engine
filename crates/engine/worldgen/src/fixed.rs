//! Q16.16 fixed-point arithmetic for the generation path.
//!
//! Two widths, for two different jobs:
//!
//! - **Coordinates** are [`Coord`] (`i64`, 48.16). The world is 1,024 km per
//!   side, which in Q16.16 metres is `6.7e10` — larger than an `i32` can hold.
//!   `i64` also leaves room for the 1 m detail octaves that scale a coordinate
//!   by a lacunarity of 8 or more.
//! - **Values** are `i32` (16.16). Noise output is normalised to
//!   `[-ONE, ONE]`, so an `i32` value has 5 fractional decimal digits of
//!   precision, far more than the terrain needs.
//!
//! Every operation **truncates** (arithmetic shift toward negative infinity),
//! never rounds. Rounding a negative product and rounding a positive product
//! symmetrically would need a branch, and the branch would be the only place
//! where two builds could disagree. Truncation is exact and branch-free.
//!
//! Floating point is confined to [`coord_from_f32`], [`coord_to_f32`],
//! [`value_from_f32`] and [`value_to_f32`]. Those exist for callers, tests and
//! the CLI; nothing inside the generation path calls them.

/// Fractional bits in both coordinate and value representations.
pub const FRAC_BITS: u32 = 16;

/// Mask of the fractional bits of a coordinator or value.
pub const FRAC_MASK: i64 = (1 << FRAC_BITS) - 1;

/// `1.0` as an `i32` Q16.16 value.
pub const ONE: i32 = 1 << FRAC_BITS;

/// `1.0` as an `i64` Q16.16 coordinate.
pub const ONE_COORD: Coord = 1 << FRAC_BITS;

/// A fixed-point coordinate with 16 fractional bits (48.16).
pub type Coord = i64;

/// Clamp a Q16.16 value to `[-ONE, ONE]`.
///
/// The noise normalisation constants are chosen so that this never changes a
/// sample; it is a guard against a future edit, and the noise tests assert the
/// raw (unclamped) range stays inside.
#[inline]
pub const fn clamp_unit(value: i32) -> i32 {
    if value > ONE {
        ONE
    } else if value < -ONE {
        -ONE
    } else {
        value
    }
}

/// Integer part of a coordinate, floored. Arithmetic shift gives the floor for
/// negative coordinates as well, which is what lattice interpolation needs.
#[inline]
pub const fn coord_floor(value: Coord) -> i64 {
    value >> FRAC_BITS
}

/// Fractional part of a coordinate, in `[0, ONE)`.
#[inline]
pub const fn coord_frac(value: Coord) -> i32 {
    (value & FRAC_MASK) as i32
}

/// Multiply a coordinate by a Q16.16 factor (for example a lacunarity of `2.0`).
///
/// Truncating division by `2^16`; the `i128` product keeps the full range of a
/// 48.16 coordinate times a 16.16 factor without wrap-around.
#[inline]
pub fn coord_scale(value: Coord, factor: i32) -> Coord {
    ((value as i128 * factor as i128) >> FRAC_BITS) as Coord
}

/// Multiply two Q16.16 values.
#[inline]
pub fn mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> FRAC_BITS) as i32
}

/// Linear interpolation from `a` to `b` by `t` in `[0, ONE]`.
///
/// Exactly `a` at `t = 0` and exactly `b` at `t = ONE`.
#[inline]
pub fn lerp(a: i32, b: i32, t: i32) -> i32 {
    (a as i64 + (((b as i64 - a as i64) * t as i64) >> FRAC_BITS)) as i32
}

/// Quintic fade curve `6t^5 - 15t^4 + 10t^3`, in Q16.16.
///
/// Maps `[0, ONE]` onto `[0, ONE]`, is monotone, and has zero first and second
/// derivatives at both ends, so neighbouring lattice cells join without a
/// visible crease in the terrain.
#[inline]
pub fn fade(t: i32) -> i32 {
    let t = t as i64;
    let t2 = (t * t) >> FRAC_BITS;
    let t3 = (t2 * t) >> FRAC_BITS;
    let t4 = (t3 * t) >> FRAC_BITS;
    let t5 = (t4 * t) >> FRAC_BITS;
    (6 * t5 - 15 * t4 + 10 * t3) as i32
}

/// Scale a Q16.16 value by a Q16.16 factor, keeping Q16.16.
#[inline]
pub fn scale(value: i32, factor: i32) -> i32 {
    ((value as i64 * factor as i64) >> FRAC_BITS) as i32
}

/// Convert metres to a fixed-point coordinate.
///
/// Truncates toward zero; the precision is `1/65,536` m (15.3 µm), far below
/// the 6.25 cm voxel.
#[inline]
pub fn coord_from_f32(metres: f32) -> Coord {
    (metres as f64 * ONE_COORD as f64) as Coord
}

/// Convert a fixed-point coordinate back to metres.
#[inline]
pub fn coord_to_f32(value: Coord) -> f32 {
    (value as f64 / ONE_COORD as f64) as f32
}

/// Convert a real number in `[-1, 1]` to a Q16.16 value.
#[inline]
pub fn value_from_f32(value: f32) -> i32 {
    (value as f64 * ONE as f64) as i32
}

/// Convert a Q16.16 value back to a real number.
#[inline]
pub fn value_to_f32(value: i32) -> f32 {
    (value as f64 / ONE as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_round_trips_known_values() {
        assert_eq!(coord_from_f32(1.0), ONE_COORD);
        assert_eq!(coord_from_f32(-1.0), -ONE_COORD);
        assert_eq!(coord_from_f32(0.5), ONE_COORD / 2);
        assert_eq!(coord_to_f32(ONE_COORD), 1.0);
        assert_eq!(value_from_f32(1.0), ONE);
        assert_eq!(value_from_f32(-1.0), -ONE);
        assert_eq!(value_to_f32(ONE), 1.0);
    }

    #[test]
    fn coord_floor_and_frac_agree_for_negative_values() {
        for (metres, floor, frac) in [
            (0.0, 0, 0),
            (0.75, 0, 3 * ONE / 4),
            (-0.25, -1, 3 * ONE / 4),
            (-1.0, -1, 0),
            (-1.5, -2, ONE / 2),
        ] {
            let value = coord_from_f32(metres);
            assert_eq!(coord_floor(value), floor, "floor of {metres}");
            assert_eq!(coord_frac(value), frac, "frac of {metres}");
        }
    }

    #[test]
    fn fade_maps_endpoints_and_is_monotone() {
        assert_eq!(fade(0), 0);
        assert_eq!(fade(ONE), ONE);
        let mut previous = 0;
        for step in 0..=64 {
            let t = step * ONE / 64;
            let value = fade(t);
            assert!(value >= previous, "fade must not decrease at {t}");
            previous = value;
        }
    }

    #[test]
    fn lerp_hits_both_endpoints_exactly() {
        assert_eq!(lerp(-ONE, ONE, 0), -ONE);
        assert_eq!(lerp(-ONE, ONE, ONE), ONE);
        assert_eq!(lerp(12345, 54321, 0), 12345);
        assert_eq!(lerp(12345, 54321, ONE), 54321);
    }

    #[test]
    fn scale_and_mul_truncate_identically() {
        assert_eq!(mul(ONE, ONE), ONE);
        assert_eq!(scale(ONE, ONE / 2), ONE / 2);
        assert_eq!(mul(-ONE, ONE / 2), -ONE / 2);
    }

    #[test]
    fn coord_scale_handles_world_extent() {
        // 1,024 km in fixed point, doubled: still inside an i64.
        let edge = coord_from_f32(1_024_000.0);
        assert_eq!(coord_scale(edge, 2 * ONE), 2 * edge);
    }
}
