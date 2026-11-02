//! 1 m material dither and canonical two-material + debris transition blocks.
//!
//! # Dither
//!
//! Biome weights vary over 300–500 m (M2.5-10), so choosing the palette by
//! `argmax` would draw a hard line through that band. Instead the surface
//! palette is chosen **per 1 m column** by walking the cumulative surface
//! weights with a uniform per-block hash:
//!
//! ```text
//! roll = hash2(seed, block_x, block_z)          // uniform in [0, ONE)
//! index = first i where roll < sum(weights[..=i])
//! ```
//!
//! The expected share of each palette equals its weight, and adjacent 1 m
//! columns interleave the two palettes instead of switching once. Underground
//! and rare biomes are excluded and the land/ocean weights are renormalised:
//! the dither is a surface operation, not a depth one.
//!
//! # Templates
//!
//! A transition block is a **16³ sub-block template** inside one 1 m block.
//! It is made only of the two parent materials plus one debris material, so it
//! introduces no block key, no face and **no atlas slot**. Four canonical
//! variants exist per unordered pair; the placement axis only rotates the same
//! four patterns.
//!
//! The template is symbolic: [`TransitionTemplate::role`] answers A/B/debris
//! for a voxel, and [`TransitionTemplate::material`] maps that to the parent
//! key. `A` is the material on the **high** side of the axis: above the
//! boundary for `Y`, east for `X`, south for `Z`. A chunk builder can expand
//! it directly or hand the pattern to the SVDAG placement path. The template
//! never allocates and is a pure function of its three keys, variant and axis.

use crate::biomes::{BiomeClass, SurfacePalette, catalog};
use crate::fixed::ONE;
use crate::hash;
use crate::surface::SurfaceColumn;
use crate::weights::BiomeWeights;

/// Canonical variants per unordered material pair.
pub const VARIANTS_PER_PAIR: u8 = 4;

/// Edge of a transition template: the 16³ sub-block inside one 1 m block.
pub const TEMPLATE_EDGE: usize = 16;

/// Voxels in one transition template.
pub const TEMPLATE_VOLUME: usize = TEMPLATE_EDGE * TEMPLATE_EDGE * TEMPLATE_EDGE;

/// Debris keys tried in order; the first that is not a parent is used.
///
/// All four are committed manifest keys. Debris is a third material, not a new
/// block: the template reuses committed materials and adds zero atlas slots.
pub const DEBRIS_KEYS: [&str; 4] = [
    "voxel:gravel",
    "voxel:cobblestone",
    "voxel:clay",
    "voxel:sand",
];

/// The axis the transition boundary runs along.
///
/// A top/filler boundary is horizontal (`Y`); a biome seam is vertical (`X` or
/// `Z`). Rotating the template does not create a fifth variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionAxis {
    /// Boundary between low and high `x`.
    X,
    /// Boundary between low and high `y`; used for the top/filler layer.
    Y,
    /// Boundary between low and high `z`.
    Z,
}

/// The role of one template voxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionRole {
    /// The first parent material, on the high side of the axis.
    A,
    /// The second parent material, on the low side of the axis.
    B,
    /// The debris material.
    Debris,
}

/// A canonical two-material + debris transition pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionTemplate<'a> {
    /// First parent material key.
    pub a: &'a str,
    /// Second parent material key.
    pub b: &'a str,
    /// Debris material key; always distinct from `a` and `b`.
    pub debris: &'a str,
    /// Canonical variant, `0..VARIANTS_PER_PAIR`.
    pub variant: u8,
    /// Boundary orientation.
    pub axis: TransitionAxis,
}

impl<'a> TransitionTemplate<'a> {
    /// A template for a pair, with the debris key chosen automatically.
    ///
    /// `variant` wraps modulo [`VARIANTS_PER_PAIR`].
    pub fn new(a: &'a str, b: &'a str, variant: u8) -> Self {
        Self {
            a,
            b,
            debris: debris_for(a, b),
            variant: variant % VARIANTS_PER_PAIR,
            axis: TransitionAxis::X,
        }
    }

    /// A template for a pair, choosing the variant from a per-block hash.
    pub fn from_hash(a: &'a str, b: &'a str, hash: u64) -> Self {
        Self::new(a, b, variant_for(hash))
    }

    /// Rotate the same canonical variant to another boundary axis.
    pub const fn with_axis(mut self, axis: TransitionAxis) -> Self {
        self.axis = axis;
        self
    }

    /// The three material keys this template can emit, in role order.
    pub const fn keys(&self) -> [&'a str; 3] {
        [self.a, self.b, self.debris]
    }

    /// The role of a voxel. Coordinates must be inside [`TEMPLATE_EDGE`].
    pub fn role(&self, x: usize, y: usize, z: usize) -> TransitionRole {
        debug_assert!(x < TEMPLATE_EDGE && y < TEMPLATE_EDGE && z < TEMPLATE_EDGE);
        let (along, u, v) = match self.axis {
            TransitionAxis::X => (x as i32, z as i32, y as i32),
            TransitionAxis::Y => (y as i32, x as i32, z as i32),
            TransitionAxis::Z => (z as i32, x as i32, y as i32),
        };
        let boundary = self.boundary(u, v);
        let near = (along - boundary).abs() <= 1;
        if near && debris_voxel(x, y, z, self.variant) {
            TransitionRole::Debris
        } else if along < boundary {
            TransitionRole::B
        } else {
            TransitionRole::A
        }
    }

    /// The material key of a voxel.
    pub fn material(&self, x: usize, y: usize, z: usize) -> &'a str {
        match self.role(x, y, z) {
            TransitionRole::A => self.a,
            TransitionRole::B => self.b,
            TransitionRole::Debris => self.debris,
        }
    }

    /// Role counts over the whole template, in `A, B, Debris` order.
    pub fn counts(&self) -> [u32; 3] {
        let mut counts = [0u32; 3];
        for x in 0..TEMPLATE_EDGE {
            for y in 0..TEMPLATE_EDGE {
                for z in 0..TEMPLATE_EDGE {
                    counts[self.role(x, y, z) as usize] += 1;
                }
            }
        }
        counts
    }

    /// The boundary position along the template axis for a row.
    fn boundary(&self, u: i32, v: i32) -> i32 {
        let middle = TEMPLATE_EDGE as i32 / 2;
        match self.variant {
            // Straight.
            0 => middle,
            // Gentle diagonal.
            1 => middle + (u - middle) / 3,
            // V-shaped wedge pushing B into A.
            2 => middle + ((u - middle).abs() - middle) / 2,
            // Two-row jitter, so the seam is not a plane.
            _ => middle + (u * 5 + v * 3) % 5 - 2,
        }
    }
}

/// The debris key for a pair: the first committed key that is not a parent.
pub fn debris_for(a: &str, b: &str) -> &'static str {
    for key in DEBRIS_KEYS {
        if key != a && key != b {
            return key;
        }
    }
    // Four candidates and at most two parents, so the loop always returns.
    DEBRIS_KEYS[0]
}

/// The canonical variant for a per-block hash.
pub const fn variant_for(hash: u64) -> u8 {
    (hash % VARIANTS_PER_PAIR as u64) as u8
}

/// Weighted choice over all 18 biomes for a Q16.16 uniform in `[0, ONE)`.
///
/// Used by tests and by depth-aware callers; the surface dither uses
/// [`choose_surface`].
pub fn choose(weights: &BiomeWeights, uniform: i32) -> usize {
    let uniform = uniform.clamp(0, ONE - 1);
    let mut cumulative = 0i32;
    for (index, weight) in weights.weights().iter().enumerate() {
        cumulative += *weight;
        if uniform < cumulative {
            return index;
        }
    }
    // Weights sum to ONE, so this is only reachable if that invariant breaks.
    weights.dominant()
}

/// Weighted choice over land and ocean biomes, renormalised.
///
/// Underground and rare biomes are excluded: their surface weight is a trace
/// that must never place a cavern or geothermal block in a field.
pub fn choose_surface(weights: &BiomeWeights, uniform: i32) -> usize {
    let biomes = &catalog().biomes;
    let total: i32 = biomes
        .iter()
        .zip(weights.weights())
        .filter(|(biome, _)| is_surface(biome.class))
        .map(|(_, weight)| *weight)
        .sum();
    if total <= 0 {
        return weights.dominant();
    }

    let uniform = uniform.clamp(0, ONE - 1);
    let scaled = ((uniform as i64 * total as i64) / ONE as i64) as i32;
    let scaled = scaled.min(total - 1);

    let mut cumulative = 0i32;
    let mut last = None;
    for (index, weight) in weights.weights().iter().enumerate() {
        if !is_surface(biomes[index].class) {
            continue;
        }
        cumulative += *weight;
        if *weight > 0 {
            last = Some(index);
        }
        if scaled < cumulative {
            return index;
        }
    }
    last.unwrap_or_else(|| weights.dominant())
}

/// The surface palette chosen by a per-block hash.
///
/// The caller hashes 1 m block coordinates, so the grain is exactly 1 m:
/// `palette_at(&weights, hash2(seed, block_x, block_z))`.
pub fn palette_at(weights: &BiomeWeights, hash: u64) -> &'static SurfacePalette {
    &catalog().biomes[choose_surface(weights, hash::unit(hash))].surface
}

/// The top material chosen by a per-block hash.
pub fn top_at(weights: &BiomeWeights, hash: u64) -> &'static str {
    palette_at(weights, hash).top.as_str()
}

/// The surface palette for a 1 m block at `(block_x, block_z)`.
///
/// This is the grain the design asks for: the weights can vary over hundreds
/// of metres, while the material changes from column to column.
pub fn palette_for_block(
    weights: &BiomeWeights,
    seed: u64,
    block_x: i64,
    block_z: i64,
) -> &'static SurfacePalette {
    palette_at(weights, hash::hash2(seed, block_x, block_z))
}

/// The top material for a 1 m block at `(block_x, block_z)`.
pub fn top_for_block(
    weights: &BiomeWeights,
    seed: u64,
    block_x: i64,
    block_z: i64,
) -> &'static str {
    palette_for_block(weights, seed, block_x, block_z)
        .top
        .as_str()
}

/// The transition template for a column's top/filler boundary, if it has one.
///
/// A column whose top and filler are the same material (beach, cliff, a
/// single-block snow cap) has no boundary to mix, so it returns `None`. The
/// template is rotated to `Y`, because the top/filler boundary is horizontal;
/// `a` is the top material (above the boundary) and `b` the filler below.
pub fn transition_for<'a>(column: &SurfaceColumn<'a>, hash: u64) -> Option<TransitionTemplate<'a>> {
    if column.filler_depth() == 0 || column.top == column.filler {
        return None;
    }
    Some(
        TransitionTemplate::new(column.top, column.filler, variant_for(hash))
            .with_axis(TransitionAxis::Y),
    )
}

fn is_surface(class: BiomeClass) -> bool {
    matches!(class, BiomeClass::Land | BiomeClass::Ocean)
}

/// A deterministic debris decision near the boundary.
///
/// The constants mix the three voxel axes and the variant; `% 5` keeps about
/// one fifth of the boundary voxels as debris, enough to break the pattern
/// without swallowing either parent.
fn debris_voxel(x: usize, y: usize, z: usize, variant: u8) -> bool {
    let mixed = (x as u32).wrapping_mul(73_856_093)
        ^ (y as u32).wrapping_mul(19_349_663)
        ^ (z as u32).wrapping_mul(83_492_791)
        ^ (variant as u32).wrapping_mul(2_654_435_761);
    mixed.is_multiple_of(5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biomes::catalog;

    fn biome(key: &str) -> &'static crate::biomes::BiomeDef {
        catalog()
            .biomes
            .iter()
            .find(|biome| biome.key == key)
            .unwrap_or_else(|| panic!("no biome {key}"))
    }

    #[test]
    fn transitions_variants_wrap_at_four() {
        assert_eq!(VARIANTS_PER_PAIR, 4);
        let pair = ("voxel:grass_block", "voxel:dirt");
        for variant in 0..8 {
            let template = TransitionTemplate::new(pair.0, pair.1, variant);
            assert_eq!(template.variant, variant % VARIANTS_PER_PAIR);
            assert_eq!(
                template.variant,
                TransitionTemplate::new(pair.0, pair.1, variant % VARIANTS_PER_PAIR).variant
            );
        }
    }

    #[test]
    fn transitions_each_variant_has_both_parents_and_debris() {
        let pair = ("voxel:grass_block", "voxel:dirt");
        for variant in 0..VARIANTS_PER_PAIR {
            for axis in [TransitionAxis::X, TransitionAxis::Y, TransitionAxis::Z] {
                let template = TransitionTemplate::new(pair.0, pair.1, variant).with_axis(axis);
                let counts = template.counts();
                assert!(
                    counts[TransitionRole::A as usize] > 0,
                    "{variant:?} {axis:?}"
                );
                assert!(
                    counts[TransitionRole::B as usize] > 0,
                    "{variant:?} {axis:?}"
                );
                assert!(
                    counts[TransitionRole::Debris as usize] > 0,
                    "{variant:?} {axis:?}"
                );
                assert_eq!(
                    counts.iter().sum::<u32>(),
                    TEMPLATE_VOLUME as u32,
                    "{variant:?} {axis:?}"
                );
            }
        }
    }

    #[test]
    fn transitions_a_is_above_b_on_the_vertical_axis() {
        let template = TransitionTemplate::new("voxel:grass_block", "voxel:dirt", 0)
            .with_axis(TransitionAxis::Y);
        // Far from the jittered boundary, so debris cannot hide the roles.
        assert_eq!(template.role(0, 0, 0), TransitionRole::B);
        assert_eq!(template.material(0, 0, 0), "voxel:dirt");
        assert_eq!(template.role(0, TEMPLATE_EDGE - 1, 0), TransitionRole::A);
        assert_eq!(
            template.material(0, TEMPLATE_EDGE - 1, 0),
            "voxel:grass_block"
        );
    }

    #[test]
    fn transitions_variants_are_distinct_patterns() {
        let pair = ("voxel:grass_block", "voxel:dirt");
        let mut patterns = Vec::new();
        for variant in 0..VARIANTS_PER_PAIR {
            let template = TransitionTemplate::new(pair.0, pair.1, variant);
            let pattern: Vec<_> = (0..TEMPLATE_EDGE)
                .flat_map(|x| {
                    (0..TEMPLATE_EDGE)
                        .flat_map(move |y| (0..TEMPLATE_EDGE).map(move |z| template.role(x, y, z)))
                })
                .collect();
            assert!(
                !patterns.contains(&pattern),
                "variant {variant} repeats another pattern"
            );
            patterns.push(pattern);
        }
    }

    #[test]
    fn transitions_debris_is_never_a_parent() {
        let keys: Vec<&str> = catalog()
            .biomes
            .iter()
            .flat_map(|biome| biome.surface.keys())
            .collect();
        for a in &keys {
            for b in &keys {
                let debris = debris_for(a, b);
                assert_ne!(debris, *a);
                assert_ne!(debris, *b);
                assert!(DEBRIS_KEYS.contains(&debris));
            }
        }
    }

    #[test]
    fn transitions_choose_surface_skips_underground() {
        let axes = crate::weights::Axes {
            continentalness: ONE / 2,
            temperature: ONE / 2,
            humidity: ONE / 2,
            erosion: ONE / 2,
            depth: 0,
        };
        let weights = BiomeWeights::at(&axes);
        for roll in (0..ONE).step_by(97) {
            let index = choose_surface(&weights, roll);
            assert!(
                is_surface(catalog().biomes[index].class),
                "chose {}",
                catalog().biomes[index].key
            );
        }
    }

    #[test]
    fn transitions_palette_at_is_deterministic() {
        let axes = crate::weights::Axes {
            continentalness: ONE / 2,
            temperature: ONE / 2,
            humidity: ONE / 2,
            erosion: ONE / 2,
            depth: 0,
        };
        let weights = BiomeWeights::at(&axes);
        let first = palette_at(&weights, 0xDEAD_BEEF);
        let second = palette_at(&weights, 0xDEAD_BEEF);
        assert_eq!(first, second);
    }

    #[test]
    fn transitions_soil_pairs_get_a_horizontal_template() {
        let plains = biome("voxel:plains");
        let sample = crate::surface::SurfaceSample::new(
            crate::splines::SEA_LEVEL + (500 << 16),
            0,
            false,
            7,
        );
        let column = crate::surface::column(plains, sample);
        let template = transition_for(&column, 11).expect("soil has a boundary");
        assert_eq!(template.a, column.top);
        assert_eq!(template.b, column.filler);
        assert_eq!(template.axis, TransitionAxis::Y);
        assert!(template.variant < VARIANTS_PER_PAIR);
        assert_eq!(
            template.material(0, TEMPLATE_EDGE - 1, 0),
            column.top,
            "the top material must be above the boundary"
        );
        assert_eq!(
            template.material(0, 0, 0),
            column.filler,
            "the filler must be below the boundary"
        );

        // Same material above and below: nothing to mix.
        let beach = biome("voxel:beach");
        let beach_column = crate::surface::column(
            beach,
            crate::surface::SurfaceSample::new(crate::splines::SEA_LEVEL + (8 << 16), 0, true, 0),
        );
        assert_eq!(beach_column.top, beach_column.filler);
        assert!(transition_for(&beach_column, 0).is_none());
    }
}
