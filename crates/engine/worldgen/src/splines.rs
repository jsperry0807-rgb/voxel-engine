//! Shape splines: continentalness and erosion to a surface elevation.
//!
//! The macro height is **not** a blend of finished heights. Continentalness
//! and erosion are blended first (M2.5-10), then the height is evaluated once
//! from the blended pair, which is the rule in the design (§5.2: "blend
//! continentalness/erosion before evaluating the height spline"). This module
//! is that evaluator.
//!
//! # Tables
//!
//! | Spline | Input | Output | Monotone |
//! |---|---|---|---|
//! | [`CONTINENTALNESS_HEIGHT`] | continentalness `[-1, 1]` | base elevation | non-decreasing |
//! | [`EROSION_RELIEF`] | erosion `[0, 1]` | relief amplitude | non-increasing |
//! | [`RELIEF_WEIGHT`] | continentalness `[-1, 1]` | relief on land | non-decreasing |
//!
//! The surface is
//!
//! ```text
//! height(c, e) = base(c) + relief(e) * weight(c)
//! ```
//!
//! with the result clamped to `[0 m, 12,000 m]`. Because `base` and `weight`
//! are non-decreasing and `relief` is non-negative, the height is
//! non-decreasing in continentalness. Because `relief` is non-increasing and
//! `weight` is non-negative, it is non-increasing in erosion. Both directions
//! are asserted by tests over the whole domain.
//!
//! # Why the table starts at 0 m
//!
//! The ocean is 6,600 m of depth: continentalness `-1` is the trench floor at
//! 0 m, `-0.25` is the shelf edge at 6,450 m, and `0` is the coastline at
//! exactly 6,600 m. Land relief adds up to 5,400 m above sea level, reaching
//! 12,000 m at continentalness `1` with no erosion. Sea level and peak are
//! therefore exact table values, not approximations.
//!
//! # Interpolation
//!
//! Each segment is a cubic Hermite with **PCHIP** slopes: the interior slope
//! is the harmonic mean of the adjacent secants when they agree in sign and
//! zero otherwise. That is the Fritsch–Carlson construction, so a monotone
//! table stays monotone inside a segment and never overshoots between control
//! points. Everything is Q16.16 integer arithmetic, including the harmonic
//! mean and the Hermite basis, so the table is bit-exact across platforms and
//! can be built in a `const` context.
//!
//! Outside a table's domain the endpoint value is returned unchanged; the
//! caller's noise is already clamped to the channel range.

use crate::fixed::{self, ONE};

/// Floor of the world, Q16.16 metres.
pub const FLOOR: i32 = 0;

/// Sea level, Q16.16 metres: exactly 6,600 m.
pub const SEA_LEVEL: i32 = 6_600 << 16;

/// Highest surface, Q16.16 metres: exactly 12,000 m.
pub const PEAK: i32 = 12_000 << 16;

/// Sea level in whole metres.
pub const SEA_LEVEL_M: u32 = 6_600;

/// Peak in whole metres.
pub const PEAK_M: u32 = 12_000;

/// Whole metres to Q16.16, for table literals.
const fn metres(value: i32) -> i32 {
    value << 16
}

/// One knot of a spline: its position, value and PCHIP slope.
#[derive(Clone, Copy, Debug)]
struct Knot {
    x: i32,
    y: i32,
    /// `dy/dx` in Q16.16, computed once by [`Spline::new`].
    slope: i32,
}

/// A monotone piecewise-cubic spline over Q16.16 control points.
///
/// Construct it in a `const` or `static`; the slopes are derived at
/// construction and stored, so sampling is a few integer multiplies.
///
/// ```
/// use voxel_worldgen::fixed::ONE;
/// use voxel_worldgen::splines::Spline;
///
/// const RAMP: Spline<3> = Spline::new([(0, 0), (ONE / 2, ONE / 2), (ONE, ONE)]);
/// assert_eq!(RAMP.eval(0), 0);
/// assert_eq!(RAMP.eval(ONE), ONE);
/// assert_eq!(RAMP.eval(ONE * 3), ONE); // clamped to the domain
/// ```
#[derive(Clone, Debug)]
pub struct Spline<const N: usize> {
    knots: [Knot; N],
}

impl<const N: usize> Spline<N> {
    /// Build a spline from `(x, y)` control points with strictly increasing
    /// `x`.
    ///
    /// The interior slopes are the harmonic mean of the adjacent secants when
    /// they have the same sign, and zero otherwise. That choice (PCHIP) is
    /// what makes a monotone table interpolate monotonically; it is also
    /// continuous in the first derivative at every knot.
    pub const fn new(points: [(i32, i32); N]) -> Self {
        assert!(N >= 2, "a spline needs at least two control points");
        let mut knots = [Knot {
            x: 0,
            y: 0,
            slope: 0,
        }; N];
        let mut index = 0;
        while index < N {
            knots[index].x = points[index].0;
            knots[index].y = points[index].1;
            index += 1;
        }
        let mut index = 1;
        while index < N {
            assert!(
                knots[index].x > knots[index - 1].x,
                "spline control points must increase in x"
            );
            index += 1;
        }

        // Secant slopes, Q16.16. `dy << 16` is exact for every table in this
        // module: the largest rise is 5,400 m over 1/4 of the domain.
        let mut secants = [0i32; N];
        let mut index = 0;
        while index + 1 < N {
            let dx = (knots[index + 1].x - knots[index].x) as i64;
            let dy = (knots[index + 1].y - knots[index].y) as i64;
            secants[index] = ((dy << 16) / dx) as i32;
            index += 1;
        }

        knots[0].slope = secants[0];
        knots[N - 1].slope = secants[N - 2];
        let mut index = 1;
        while index + 1 < N {
            let left = secants[index - 1] as i64;
            let right = secants[index] as i64;
            knots[index].slope = if (left > 0 && right > 0) || (left < 0 && right < 0) {
                ((2 * left * right) / (left + right)) as i32
            } else {
                0
            };
            index += 1;
        }
        Self { knots }
    }

    /// Number of control points.
    pub const fn len(&self) -> usize {
        N
    }

    /// True when the spline has no control points (never, but clippy asks).
    pub const fn is_empty(&self) -> bool {
        N == 0
    }

    /// The `index`-th control point.
    ///
    /// # Panics
    ///
    /// If `index >= len()`.
    pub const fn point(&self, index: usize) -> (i32, i32) {
        (self.knots[index].x, self.knots[index].y)
    }

    /// The PCHIP slope at a control point, Q16.16 `dy/dx`.
    pub const fn slope(&self, index: usize) -> i32 {
        self.knots[index].slope
    }

    /// Evaluate at `x`, clamping to the table's domain.
    pub fn eval(&self, x: i32) -> i32 {
        let first = self.knots[0];
        let last = self.knots[N - 1];
        if x <= first.x {
            return first.y;
        }
        if x >= last.x {
            return last.y;
        }
        let mut index = 0;
        while index + 2 < N && x >= self.knots[index + 1].x {
            index += 1;
        }
        self.segment(index, x)
    }

    /// Evaluate inside the segment starting at knot `index`.
    fn segment(&self, index: usize, x: i32) -> i32 {
        let left = self.knots[index];
        let right = self.knots[index + 1];
        let width = (right.x - left.x) as i64;
        // Position inside the segment in Q16.16, `0..=ONE`.
        let t = ((((x - left.x) as i64) << 16) / width) as i32;
        let t2 = fixed::mul(t, t);
        let t3 = fixed::mul(t2, t);
        let h00 = 2 * t3 - 3 * t2 + ONE;
        let h10 = t3 - 2 * t2 + t;
        let h01 = -2 * t3 + 3 * t2;
        let h11 = t3 - t2;
        // A slope is dy/dx; multiply by the segment width to get a dy.
        let m0 = fixed::mul(left.slope, width as i32);
        let m1 = fixed::mul(right.slope, width as i32);
        let y = h00 as i64 * left.y as i64
            + h10 as i64 * m0 as i64
            + h01 as i64 * right.y as i64
            + h11 as i64 * m1 as i64;
        (y >> fixed::FRAC_BITS) as i32
    }
}

/// Continentalness to base elevation: the ocean floor, the shelf, the coast
/// and the mountain base.
///
/// | Continentalness | Elevation | Reads as |
/// |---:|---:|---|
/// | -1.000 | 0 m | trench floor |
/// | -0.750 | 1,800 m | trench |
/// | -0.500 | 4,200 m | deep ocean |
/// | -0.250 | 6,450 m | shelf edge |
/// | 0.000 | 6,600 m | coastline |
/// | 0.125 | 6,750 m | beach and lowland |
/// | 0.250 | 7,000 m | plains |
/// | 0.500 | 8,000 m | hills |
/// | 0.750 | 9,400 m | mountains |
/// | 1.000 | 10,800 m | high mountains (before relief) |
pub const CONTINENTALNESS_HEIGHT: Spline<10> = Spline::new([
    (-ONE, metres(0)),
    (-3 * ONE / 4, metres(1_800)),
    (-ONE / 2, metres(4_200)),
    (-ONE / 4, metres(6_450)),
    (0, SEA_LEVEL),
    (ONE / 8, metres(6_750)),
    (ONE / 4, metres(7_000)),
    (ONE / 2, metres(8_000)),
    (3 * ONE / 4, metres(9_400)),
    (ONE, metres(10_800)),
]);

/// Erosion to relief amplitude, added on land.
///
/// Erosion `0` is the least eroded terrain and carries the full 1,200 m of
/// relief; erosion `1` is the most eroded (old, rounded) terrain and carries
/// none. The table is non-increasing, and the surface height inherits that.
pub const EROSION_RELIEF: Spline<5> = Spline::new([
    (0, metres(1_200)),
    (ONE / 4, metres(900)),
    (ONE / 2, metres(500)),
    (3 * ONE / 4, metres(200)),
    (ONE, metres(0)),
]);

/// Continentalness to the share of relief that applies.
///
/// The ocean floor and the coastline are flat: weight is zero at and below
/// continentalness `0`, so a coast is exactly sea level and a shelf is exactly
/// its base elevation. Relief fades in over the first quarter of the land,
/// which keeps beaches flat without a separate beach rule.
pub const RELIEF_WEIGHT: Spline<6> = Spline::new([
    (-ONE, 0),
    (-ONE / 4, 0),
    (0, 0),
    (ONE / 8, ONE / 2),
    (ONE / 4, ONE),
    (ONE, ONE),
]);

/// The surface elevation in Q16.16 metres for a blended shape pair.
///
/// `continentalness` is in `[-ONE, ONE]` and `erosion` in `[0, ONE]`; both are
/// clamped to the table domains. The result is clamped to
/// `[FLOOR, PEAK]`.
///
/// The shape parameters must be blended **before** this call: blending two
/// finished heights from different biomes produces a straight line through the
/// middle of a ridge, which is why the design forbids it.
pub fn surface_height(continentalness: i32, erosion: i32) -> i32 {
    let base = CONTINENTALNESS_HEIGHT.eval(continentalness) as i64;
    let weight = RELIEF_WEIGHT.eval(continentalness) as i64;
    let relief = EROSION_RELIEF.eval(erosion) as i64;
    let height = base + ((weight * relief) >> fixed::FRAC_BITS);
    height.clamp(FLOOR as i64, PEAK as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spline_control_points_are_exact() {
        assert_eq!(CONTINENTALNESS_HEIGHT.eval(0), SEA_LEVEL);
        assert_eq!(CONTINENTALNESS_HEIGHT.eval(-ONE), 0);
        assert_eq!(CONTINENTALNESS_HEIGHT.eval(ONE), metres(10_800));
        assert_eq!(EROSION_RELIEF.eval(0), metres(1_200));
        assert_eq!(EROSION_RELIEF.eval(ONE), 0);
        assert_eq!(RELIEF_WEIGHT.eval(0), 0);
        assert_eq!(RELIEF_WEIGHT.eval(ONE), ONE);
    }

    #[test]
    fn spline_derives_pchip_slopes() {
        let ramp = Spline::new([(0, 0), (ONE, ONE)]);
        assert_eq!(ramp.slope(0), ONE);
        assert_eq!(ramp.slope(1), ONE);

        // A flat segment forces the shared slope to zero, so the join cannot
        // overshoot into a bump.
        let plateau = Spline::new([(0, 0), (ONE, ONE), (2 * ONE, ONE)]);
        assert_eq!(plateau.slope(1), 0);
        assert_eq!(plateau.eval(ONE), ONE);
        assert_eq!(plateau.eval(2 * ONE), ONE);
        for step in 0..=64 {
            let x = ONE + step * ONE / 64;
            assert_eq!(plateau.eval(x), ONE, "plateau must stay flat at {x}");
        }
    }
}
