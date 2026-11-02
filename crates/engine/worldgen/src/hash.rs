//! Integer hashing for the generation path.
//!
//! There is no ordered RNG anywhere in worldgen. Every random-looking decision
//! is a pure function of `(seed, position)`: feature placement hashes its
//! anchor cell, lattice noise hashes its four or eight corners, and biome
//! dither hashes the block position. The same query therefore returns the same
//! answer from any thread, in any order, in any chunk.
//!
//! The finaliser is SplitMix64. It needs one `u64` multiply-xor round and
//! passes BigCrush, and its low bits are as well mixed as its high bits, which
//! lets gradient selection and uniform samples both read cheaply.

/// SplitMix64's state increment, the 64-bit golden-ratio constant.
pub const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// Finalise a `u64` with the SplitMix64 output function.
///
/// The argument is treated as a state, so the constant is added first; this
/// matches the reference generator when called on successive states.
#[inline]
pub const fn splitmix64(mut state: u64) -> u64 {
    state = state.wrapping_add(GAMMA);
    state = (state ^ (state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    state = (state ^ (state >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    state ^ (state >> 31)
}

// Per-axis mixing constants, the odd 64-bit primes used elsewhere in the
// engine (`texgraph`, the M3 stress scene). Signed lattice coordinates are
// cast with `as u64`, i.e. two's complement, so `-1` and `0` hash differently.
const MIX_X: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_Y: u64 = 0xC2B2_AE3D_27D4_EB4F;
const MIX_Z: u64 = 0x1656_67B1_9E37_79F9;

/// Hash a seed and a 2D lattice coordinate.
///
/// Each axis is folded in and finalised separately. A single XOR of two axis
/// products is cheaper but not injective: `x = 1, y = 0` and some other pair
/// can cancel, which shows up as two lattice corners sharing a gradient.
#[inline]
pub fn hash2(seed: u64, x: i64, y: i64) -> u64 {
    let x_mixed = splitmix64(seed ^ (x as u64).wrapping_mul(MIX_X));
    splitmix64(x_mixed ^ (y as u64).wrapping_mul(MIX_Y))
}

/// Hash a seed and a 3D lattice coordinate.
#[inline]
pub fn hash3(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let x_mixed = splitmix64(seed ^ (x as u64).wrapping_mul(MIX_X));
    let y_mixed = splitmix64(x_mixed ^ (y as u64).wrapping_mul(MIX_Y));
    splitmix64(y_mixed ^ (z as u64).wrapping_mul(MIX_Z))
}

/// The 8 two-dimensional gradient directions, as edge and diagonal vectors.
///
/// All components are `-1`, `0` or `1`, so the dot product with a Q16.16
/// fraction is exact integer addition and subtraction.
const GRADIENTS_2D: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// The 12 three-dimensional gradient directions: the edges of the cube.
///
/// These are the gradients of "improved" Perlin noise. They avoid the axis
/// bias of the original 3D gradient set (which reused the 2D directions).
const GRADIENTS_3D: [(i32, i32, i32); 12] = [
    (1, 1, 0),
    (-1, 1, 0),
    (1, -1, 0),
    (-1, -1, 0),
    (1, 0, 1),
    (-1, 0, 1),
    (1, 0, -1),
    (-1, 0, -1),
    (0, 1, 1),
    (0, -1, 1),
    (0, 1, -1),
    (0, -1, -1),
];

/// Pick one of the 8 2D gradients from a hash.
#[inline]
pub fn gradient2(hash: u64) -> (i32, i32) {
    GRADIENTS_2D[(hash & 7) as usize]
}

/// Pick one of the 12 3D gradients from a hash.
#[inline]
pub fn gradient3(hash: u64) -> (i32, i32, i32) {
    GRADIENTS_3D[(hash % 12) as usize]
}

/// A uniform Q16.16 value in `[0, ONE)` from a hash.
///
/// Used by placement code (flora density, ore veins, biome dither) wherever a
/// probability is needed without a sequential RNG.
#[inline]
pub fn unit(hash: u64) -> i32 {
    ((hash >> 32) as u32 >> 16) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_reference_sequence() {
        // Reference: splitmix64 output for states 0, 1, 2.
        let mut state = 0u64;
        let mut first_three = Vec::new();
        for _ in 0..3 {
            first_three.push(splitmix64(state));
            state = state.wrapping_add(GAMMA);
        }
        assert_eq!(
            first_three,
            vec![
                0xE220_A839_7B1D_CDAF,
                0x6E78_9E6A_A1B9_65F4,
                0x06C4_5D18_8009_454F,
            ]
        );
    }

    #[test]
    fn lattice_hashes_are_signed_and_distinct() {
        let seed = 7;
        let points = [(-1, 0), (0, -1), (0, 0), (0, 1), (1, 0), (-1, -1)];
        let mut seen = Vec::new();
        for (x, y) in points {
            let hash = hash2(seed, x, y);
            assert!(!seen.contains(&hash), "collision at ({x}, {y})");
            seen.push(hash);
        }
        assert_ne!(hash2(seed, 0, 0), hash3(seed, 0, 0, 0));
        assert_ne!(hash3(seed, 0, 0, 0), hash3(seed, 0, 0, 1));
    }

    #[test]
    fn gradients_cover_the_declared_sets() {
        let mut twos = Vec::new();
        let mut threes = Vec::new();
        for hash in 0..64u64 {
            let g2 = gradient2(hash);
            assert!((-1..=1).contains(&g2.0) && (-1..=1).contains(&g2.1));
            assert_ne!(g2, (0, 0));
            if !twos.contains(&g2) {
                twos.push(g2);
            }
            let g3 = gradient3(hash);
            assert!((-1..=1).contains(&g3.0));
            assert!((-1..=1).contains(&g3.1));
            assert!((-1..=1).contains(&g3.2));
            assert_ne!(g3, (0, 0, 0));
            if !threes.contains(&g3) {
                threes.push(g3);
            }
        }
        assert_eq!(twos.len(), 8);
        assert_eq!(threes.len(), 12);
    }

    #[test]
    fn unit_is_in_range_and_spread() {
        let samples: Vec<i32> = (0..10_000).map(|i| unit(hash2(3, i, 0))).collect();
        assert!(samples.iter().all(|&v| (0..crate::fixed::ONE).contains(&v)));
        // The mean of 10,000 uniform samples is within 2% of 0.5 (about 3
        // standard errors of the mean).
        let mean = samples.iter().map(|&v| v as i64).sum::<i64>() / samples.len() as i64;
        let half = crate::fixed::ONE as i64 / 2;
        assert!(
            (mean - half).abs() < half / 50,
            "unit() mean {mean} is not near {half}"
        );
    }
}
