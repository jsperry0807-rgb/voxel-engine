//! Deterministic 2D and 3D gradient noise.
//!
//! [`Noise`] is a seed plus a pure function of position. Sampling never touches
//! shared state, so 1, 2 and 8 threads produce identical bytes, and a chunk
//! generated on a worker is identical to the same chunk generated inline.
//!
//! # Representation
//!
//! Coordinates are [`Coord`] (48.16 i64 metres). Values are `i32` Q16.16
//! normalised to `[-ONE, ONE]`. The lattice index is the floor of the
//! coordinate and the fraction is the low 16 bits, so negative coordinates
//! sample the cell below zero rather than wrapping.
//!
//! # Interpolation
//!
//! The corner gradients are dotted with the **raw** fraction, and the results
//! are blended with the quintic [`fade`](crate::fixed::fade) of that fraction.
//! That is Perlin's construction; using the faded fraction in the dot product
//! instead would flatten the field near lattice points.
//!
//! # Normalisation
//!
//! The raw sum of eight unit-ish gradients is larger than one in magnitude.
//! [`NORM_2D`] and [`NORM_3D`] scale by `0.995 / B`, where `B` is the worst
//! case of the field's convex hull: `1.0` in 2D and `1.03633` in 3D. Because
//! `B` is a proven bound and not a sampled maximum, the
//! [`clamp_unit`](crate::fixed::clamp_unit) guard can never change a sample;
//! `tests/noise.rs` scans many seeds to confirm it.
//!
//! The proof is short. Interpolation is convex, so
//! `|v| <= sum(weight_i * |gradient_i . offset_i|)`. A corner gradient can be
//! chosen to align with the corner offset, so the achievable bound is
//! `sum(weight_i * d_i)`. In 2D `d_i` is the Manhattan distance to that corner
//! and its maximum is exactly `1.0` at the cell centre. In 3D the gradient set
//! only has cube edges, so `d_i` is the sum of the two largest components of
//! the corner offset; its maximum is `1.03633` at
//! `(0.3583, 0.4833, 0.5)`. The constants below are `0.995 / B`, leaving 0.5%
//! for the integer truncation in the three (2D) or seven (3D) interpolation
//! steps.

use crate::fixed::{self, Coord, ONE};
use crate::hash;

/// Normalisation for 2D samples: `0.995 * ONE / 1.0`.
const NORM_2D: i32 = 65_208;

/// Normalisation for 3D samples: `0.995 * ONE / 1.03633`.
const NORM_3D: i32 = 62_932;

/// A seeded integer-hash gradient noise field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Noise {
    seed: u64,
}

impl Noise {
    /// Create a field from a seed. The seed is the only input.
    #[inline]
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The seed this field was created with.
    #[inline]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// 2D gradient noise at a fixed-point coordinate, in `[-ONE, ONE]`.
    #[inline]
    pub fn value_2d(&self, x: Coord, y: Coord) -> i32 {
        fixed::clamp_unit(fixed::scale(self.raw_2d(x, y), NORM_2D))
    }

    /// 3D gradient noise at a fixed-point coordinate, in `[-ONE, ONE]`.
    #[inline]
    pub fn value_3d(&self, x: Coord, y: Coord, z: Coord) -> i32 {
        fixed::clamp_unit(fixed::scale(self.raw_3d(x, y, z), NORM_3D))
    }

    /// Unnormalised 2D sample. Exposed to the crate for the range test only.
    #[inline]
    pub(crate) fn raw_2d(&self, x: Coord, y: Coord) -> i32 {
        let (xi, yi) = (fixed::coord_floor(x), fixed::coord_floor(y));
        let (fx, fy) = (fixed::coord_frac(x), fixed::coord_frac(y));
        let (u, v) = (fixed::fade(fx), fixed::fade(fy));

        let n00 = dot2(self.gradient_2d(xi, yi), fx, fy);
        let n10 = dot2(self.gradient_2d(xi + 1, yi), fx - ONE, fy);
        let n01 = dot2(self.gradient_2d(xi, yi + 1), fx, fy - ONE);
        let n11 = dot2(self.gradient_2d(xi + 1, yi + 1), fx - ONE, fy - ONE);

        fixed::lerp(fixed::lerp(n00, n10, u), fixed::lerp(n01, n11, u), v)
    }

    /// Unnormalised 3D sample. Exposed to the crate for the range test only.
    #[inline]
    pub(crate) fn raw_3d(&self, x: Coord, y: Coord, z: Coord) -> i32 {
        let (xi, yi, zi) = (
            fixed::coord_floor(x),
            fixed::coord_floor(y),
            fixed::coord_floor(z),
        );
        let (fx, fy, fz) = (
            fixed::coord_frac(x),
            fixed::coord_frac(y),
            fixed::coord_frac(z),
        );
        let (u, v, w) = (fixed::fade(fx), fixed::fade(fy), fixed::fade(fz));

        // The corner at `zi` sees fraction `fz`; the corner at `zi + 1` sees
        // `fz - ONE`. The same holds for y and x.
        let mut planes = [0i32; 2];
        for (dz, plane) in planes.iter_mut().enumerate() {
            let dz = dz as i32;
            let cz = zi + dz as i64;
            let fz_corner = fz - dz * ONE;
            let mut row = [0i32; 2];
            for (dy, out) in row.iter_mut().enumerate() {
                let dy = dy as i32;
                let cy = yi + dy as i64;
                let fy_corner = fy - dy * ONE;
                let n0 = dot3(self.gradient_3d(xi, cy, cz), fx, fy_corner, fz_corner);
                let n1 = dot3(
                    self.gradient_3d(xi + 1, cy, cz),
                    fx - ONE,
                    fy_corner,
                    fz_corner,
                );
                *out = fixed::lerp(n0, n1, u);
            }
            *plane = fixed::lerp(row[0], row[1], v);
        }
        fixed::lerp(planes[0], planes[1], w)
    }

    /// Fractal sum of 2D samples, normalised back into `[-ONE, ONE]`.
    ///
    /// `lacunarity` and `gain` are Q16.16 factors (for example `2.0` and
    /// `0.5`). Each octave samples at the current scale, then scales the
    /// coordinate; the amplitude decays by `gain`. With `octaves == 0` the
    /// result is zero.
    pub fn fbm_2d(&self, x: Coord, y: Coord, octaves: u32, lacunarity: i32, gain: i32) -> i32 {
        assert!(lacunarity > 0, "lacunarity must be positive");
        assert!((0..=ONE).contains(&gain), "gain must be in [0, 1]");
        if octaves == 0 {
            return 0;
        }
        let mut total: i64 = 0;
        let mut total_amplitude: i64 = 0;
        let mut amplitude = ONE;
        let (mut cx, mut cy) = (x, y);
        for _ in 0..octaves {
            total += fixed::mul(self.value_2d(cx, cy), amplitude) as i64;
            total_amplitude += amplitude as i64;
            cx = fixed::coord_scale(cx, lacunarity);
            cy = fixed::coord_scale(cy, lacunarity);
            amplitude = fixed::mul(amplitude, gain);
        }
        ((total << fixed::FRAC_BITS) / total_amplitude) as i32
    }

    /// Fractal sum of 3D samples, normalised back into `[-ONE, ONE]`.
    pub fn fbm_3d(
        &self,
        x: Coord,
        y: Coord,
        z: Coord,
        octaves: u32,
        lacunarity: i32,
        gain: i32,
    ) -> i32 {
        assert!(lacunarity > 0, "lacunarity must be positive");
        assert!((0..=ONE).contains(&gain), "gain must be in [0, 1]");
        if octaves == 0 {
            return 0;
        }
        let mut total: i64 = 0;
        let mut total_amplitude: i64 = 0;
        let mut amplitude = ONE;
        let (mut cx, mut cy, mut cz) = (x, y, z);
        for _ in 0..octaves {
            total += fixed::mul(self.value_3d(cx, cy, cz), amplitude) as i64;
            total_amplitude += amplitude as i64;
            cx = fixed::coord_scale(cx, lacunarity);
            cy = fixed::coord_scale(cy, lacunarity);
            cz = fixed::coord_scale(cz, lacunarity);
            amplitude = fixed::mul(amplitude, gain);
        }
        ((total << fixed::FRAC_BITS) / total_amplitude) as i32
    }

    #[inline]
    fn gradient_2d(&self, x: i64, y: i64) -> (i32, i32) {
        hash::gradient2(hash::hash2(self.seed, x, y))
    }

    #[inline]
    fn gradient_3d(&self, x: i64, y: i64, z: i64) -> (i32, i32, i32) {
        hash::gradient3(hash::hash3(self.seed, x, y, z))
    }
}

#[inline]
fn dot2(gradient: (i32, i32), x: i32, y: i32) -> i32 {
    gradient.0 * x + gradient.1 * y
}

#[inline]
fn dot3(gradient: (i32, i32, i32), x: i32, y: i32, z: i32) -> i32 {
    gradient.0 * x + gradient.1 * y + gradient.2 * z
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::coord_from_f32;

    #[test]
    fn noise_is_zero_on_lattice_points() {
        let noise = Noise::new(0);
        for i in -8..=8 {
            let p = (i as i64) * fixed::ONE_COORD;
            assert_eq!(noise.value_2d(p, p), 0, "2D at {i}");
            assert_eq!(noise.value_3d(p, p, p), 0, "3D at {i}");
        }
    }

    #[test]
    fn noise_is_continuous_across_cells() {
        let noise = Noise::new(11);
        // A 1/4096 m step must not jump: gradient noise is C2.
        let step = fixed::ONE_COORD / 4096;
        let mut previous = noise.value_2d(0, 0);
        for i in 1..=4096 {
            let value = noise.value_2d(i * step, 0);
            assert!(
                (value - previous).abs() < ONE / 8,
                "2D jump at {i}: {previous} -> {value}"
            );
            previous = value;
        }
    }

    #[test]
    fn noise_normalisation_never_clamps() {
        // The normalisation constants are proven bounds, not sampled maxima;
        // this scan is the cross-check over several seeds and the negative
        // half of the plane. If it trips, NORM_* is too large and the output is
        // being clipped rather than normalised.
        let step = fixed::ONE_COORD / 16;
        for seed in [0, 1, 2, 3, 0xDEAD_BEEF, 0x1234_5678_9ABC_DEF0] {
            let noise = Noise::new(seed);
            for i in -16..48 {
                for j in -16..48 {
                    let raw = noise.raw_2d(i * step, j * step);
                    assert!(
                        fixed::scale(raw, NORM_2D).abs() <= ONE,
                        "2D clamp for seed {seed} at ({i}, {j})"
                    );
                }
            }
            for i in 0..32 {
                for j in 0..32 {
                    for k in 0..32 {
                        let raw = noise.raw_3d(i * step, j * step, k * step);
                        assert!(
                            fixed::scale(raw, NORM_3D).abs() <= ONE,
                            "3D clamp for seed {seed} at ({i}, {j}, {k})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn noise_fbm_endpoints_and_bounds() {
        let noise = Noise::new(3);
        let x = coord_from_f32(123.4);
        let y = coord_from_f32(-56.7);
        assert_eq!(noise.fbm_2d(x, y, 0, 2 * ONE, ONE / 2), 0);
        for octaves in 1..=6 {
            let value = noise.fbm_2d(x, y, octaves, 2 * ONE, ONE / 2);
            assert!((-ONE..=ONE).contains(&value), "fbm {} = {}", octaves, value);
        }
    }

    #[test]
    fn noise_is_seed_dependent() {
        let x = coord_from_f32(1.5);
        assert_ne!(Noise::new(1).value_2d(x, x), Noise::new(2).value_2d(x, x));
        assert_eq!(Noise::new(1).value_2d(x, x), Noise::new(1).value_2d(x, x));
    }
}
