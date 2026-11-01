//! Biome weights: a softmax over the biome centres.
//!
//! Every biome (defined in `data/biomes.ron`) has a centre in a normalised
//! five-axis space:
//!
//! | Axis | Source | Range |
//! |---|---|---|
//! | continentalness | the macro field, remapped `(c + 1) / 2` | 0–1 |
//! | temperature | the climate channel | 0–1 |
//! | humidity | the climate channel | 0–1 |
//! | erosion | the macro field | 0–1 |
//! | depth | 0 at the surface, 1 deep underground | 0–1 |
//!
//! The weight of a biome is `exp(-d² / T)` where `d²` is the squared distance in
//! that space, normalised so the weights sum to exactly `ONE`. The softmax
//! temperature `T = 0.03` (in squared normalised units) is what makes edges
//! blend: a pair of centres one axis gap apart (0.25) has a blend band of about
//! 0.26 axis units, which is 300–500 m of terrain when the axes move at the
//! design's edge gradient (one gap over 400 m). The tests pin that conversion,
//! the exact sum, and the absence of hard argmax switches.
//!
//! Underground biomes (aquifer, fungal deep, cavern) and the rare geothermal
//! biome sit at a depth centre between 0.6 and 0.9. At the surface their
//! distance includes the depth gap, so they carry a trace weight rather than
//! competing; M2.5-15 samples with `depth > 0` to hand the underground over to
//! them. The centres themselves are content: `crate::biomes` parses them from
//! `data/biomes.ron`.
//!
//! # Blend shape parameters, not finished heights
//!
//! The design's rule is that shape parameters are blended **before** the shape
//! spline is evaluated. [`BiomeWeights::blend`] exists for that: pass a
//! parameter vector and get the weighted parameter, then evaluate once. Do not
//! evaluate per biome and blend the results.
//!
//! All arithmetic is Q16.16 integer. `exp` is a compile-time table, so the
//! whole function is deterministic and platform-independent.

use crate::biomes;
use crate::fixed::ONE;

/// Number of biomes.
pub const BIOME_COUNT: usize = 18;

/// Softmax temperature in squared normalised distance, Q16.16 (0.03).
pub const SOFTMAX_TEMPERATURE: i32 = ONE * 3 / 100;

/// Normalised distance steps in the exponential table: 1/16 per entry.
const EXP_STEP_SHIFT: i32 = 12;

/// Entries in the exponential table (`0..=8` in 1/16 steps).
const EXP_TABLE_LEN: usize = 129;

/// The table of `exp(-x)` in Q16.16 for `x = index / 16`.
static EXP_TABLE: [i32; EXP_TABLE_LEN] = build_exp_table();

/// Axes in the biome space.
pub const BIOME_COUNT_AXES: usize = 5;

/// Every biome centre in Q16.16, in catalogue order.
pub type Centres = [[i32; BIOME_COUNT_AXES]; BIOME_COUNT];

/// Sample axes, normalised to `[0, ONE]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axes {
    /// Continentalness remapped from `[-ONE, ONE]` to `[0, ONE]`.
    pub continentalness: i32,
    /// Temperature channel.
    pub temperature: i32,
    /// Humidity channel.
    pub humidity: i32,
    /// Erosion channel.
    pub erosion: i32,
    /// 0 at the surface, `ONE` deep underground.
    pub depth: i32,
}

impl Axes {
    /// Surface axes from the macro fields, as M2.5-05 produces them.
    pub fn surface(continentalness: i32, temperature: u8, humidity: u8, erosion: i32) -> Self {
        Self {
            continentalness: ((continentalness as i64 + ONE as i64) / 2) as i32,
            temperature: (temperature as i32) * ONE / 255,
            humidity: (humidity as i32) * ONE / 255,
            erosion,
            depth: 0,
        }
    }

    /// Squared distance to a centre, Q16.16, five axes.
    fn distance_squared(&self, centre: &[i32; BIOME_COUNT_AXES]) -> i64 {
        let axes = [
            self.continentalness,
            self.temperature,
            self.humidity,
            self.erosion,
            self.depth,
        ];
        let mut total = 0i64;
        for (axis, centre) in axes.iter().zip(centre) {
            let difference = (*axis - *centre) as i64;
            total += difference * difference;
        }
        // Squared Q16.16 differences are Q32.32; scale back to Q16.16.
        total / ONE as i64
    }
}

/// The biome weights for one sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeWeights {
    weights: [i32; BIOME_COUNT],
}

impl BiomeWeights {
    /// Weights for the given axes, using the embedded biome catalogue.
    pub fn at(axes: &Axes) -> Self {
        Self::at_with(axes, biomes::embedded_centres())
    }

    /// Weights for the given axes and centres, summing to exactly [`ONE`].
    pub fn at_with(axes: &Axes, centres: &Centres) -> Self {
        let mut scores = [0i32; BIOME_COUNT];
        let mut total = 0i64;
        for (index, centre) in centres.iter().enumerate() {
            let distance = axes.distance_squared(centre);
            // Score in Q16.16; the table has 1/16 steps and clamps at x = 8.
            let scaled = (distance * ONE as i64) / SOFTMAX_TEMPERATURE as i64;
            let step = (scaled >> EXP_STEP_SHIFT).clamp(0, (EXP_TABLE_LEN - 1) as i64) as usize;
            scores[index] = EXP_TABLE[step];
            total += scores[index] as i64;
        }

        let mut weights = [0i32; BIOME_COUNT];
        let mut assigned = 0i32;
        let mut largest = 0usize;
        for index in 0..BIOME_COUNT {
            let weight = ((scores[index] as i64 * ONE as i64 + total / 2) / total) as i32;
            weights[index] = weight;
            assigned += weight;
            if weight > weights[largest] {
                largest = index;
            }
        }
        // Absorb the rounding residual in the largest weight so the sum is
        // exact rather than merely close.
        weights[largest] += ONE - assigned;
        Self { weights }
    }

    /// The weight vector, in [`BIOMES`] order, summing to [`ONE`].
    pub const fn weights(&self) -> &[i32; BIOME_COUNT] {
        &self.weights
    }

    /// One biome's weight.
    pub fn weight(&self, index: usize) -> i32 {
        self.weights[index]
    }

    /// Index of the strongest biome.
    pub fn dominant(&self) -> usize {
        let mut best = 0;
        for index in 1..BIOME_COUNT {
            if self.weights[index] > self.weights[best] {
                best = index;
            }
        }
        best
    }

    /// Weighted blend of a per-biome parameter vector, Q16.16 in and out.
    ///
    /// Use this on shape parameters **before** evaluating the shape spline, as
    /// the design requires; blending finished heights would cut straight
    /// through ridges.
    pub fn blend(&self, values: &[i32; BIOME_COUNT]) -> i32 {
        let mut total = 0i64;
        for (weight, value) in self.weights.iter().zip(values) {
            total += *weight as i64 * *value as i64;
        }
        (total / ONE as i64) as i32
    }
}

const fn build_exp_table() -> [i32; EXP_TABLE_LEN] {
    let mut table = [0i32; EXP_TABLE_LEN];
    let mut index = 0;
    while index < EXP_TABLE_LEN {
        // x = index / 16 in Q16.16.
        let x = (index as i128) * 4096;
        let value = exp_neg(x);
        // The series truncates at 1/65,536 and can wobble by a unit or two;
        // clamp to the previous value so the table is exactly monotone.
        table[index] = if index == 0 || value < table[index - 1] {
            value
        } else {
            table[index - 1]
        };
        index += 1;
    }
    table
}

/// `exp(-x)` in Q16.16 by its Taylor series, for compile-time use only.
const fn exp_neg(x: i128) -> i32 {
    let one = 65536i128;
    let mut term = one;
    let mut sum = one;
    let mut n = 1i128;
    while n <= 48 {
        term = -term * x / one / n;
        sum += term;
        n += 1;
    }
    if sum < 0 {
        0
    } else if sum > one {
        one as i32
    } else {
        sum as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_exp_table_matches_known_values() {
        assert_eq!(EXP_TABLE[0], ONE); // exp(0)
        assert_eq!(EXP_TABLE[16], 24_109); // exp(-1) = 0.3679
        assert!((EXP_TABLE[32] - 8_869).abs() <= 2); // exp(-2) = 0.1353
        assert!((EXP_TABLE[128] - 21).abs() <= 2); // exp(-8) = 0.000335
        // Monotone decreasing.
        for pair in EXP_TABLE.windows(2) {
            assert!(pair[1] <= pair[0]);
        }
    }
}
