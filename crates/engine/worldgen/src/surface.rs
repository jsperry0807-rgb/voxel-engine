//! Surface rules: which materials make up the top of a column.
//!
//! The macro map decides the height; this module decides what the first few
//! blocks are. It is a pure function of a biome definition (from
//! `data/biomes.ron`) and a [`SurfaceSample`] carrying the local height, slope,
//! shore flag and per-column hash. Nothing here samples noise or touches the
//! registry, so the rules are cheap, deterministic and testable on their own:
//!
//! ```text
//! cliff  →  underwater  →  snow  →  rock  →  beach  →  soil
//! ```
//!
//! The order is the design's precedence. A steep face exposes bedrock even if
//! it is a snow peak; a seabed is water-lain material before altitude matters;
//! the universal rock-to-snow line overrides every biome; a beach needs both a
//! shore and a low, gentle column; everything else is the biome's own soil.
//!
//! # Soil depth
//!
//! The standard land biomes use a **3–6 block** soil default. The exact value
//! comes from the biome's `soil_depth` range in `data/biomes.ron`, so the
//! biome is the override: plains, forest, hills and taiga sit at 3–6; desert,
//! savanna, swamp and tundra are thinner; beach, mountains and alpine are
//! thinner still. The per-column hash picks inside the range, so soil depth
//! varies naturally without a sequential RNG.
//!
//! Beach and seabed are fixed thicknesses ([`BEACH_SOIL_DEPTH`] and
//! [`SEABED_SOIL_DEPTH`]) because they describe a depositional layer, not a
//! biome's soil. Cliffs and rock expose stone (`soil_depth == 0`); a snow cap
//! is one block over rock.
//!
//! # Slopes and shores
//!
//! `slope` is Q16.16 rise over run: `ONE` is 45°. [`CLIFF_SLOPE`] is that
//! 45° threshold. Beaches also require a gentle column ([`BEACH_MAX_SLOPE`]),
//! because a steep shore is a cliff or a headland, not sand.
//!
//! `shore` is supplied by the caller, not inferred from height alone: it is
//! true within the shore band of an ocean, lake or river. That distinction is
//! what makes **beaches ring water** instead of painting every low-lying
//! inland basin with sand.
//!
//! # Allocation
//!
//! [`column`] borrows the biome's `&str` keys, so a column plan is three
//! pointers and two bytes; it never allocates. Resolve the keys to
//! [`MaterialId`](https://docs.rs/voxel-block) in the chunk pipeline.

use crate::biomes::{BiomeClass, BiomeDef, catalog};
use crate::fixed::ONE;
use crate::splines::SEA_LEVEL;

/// Blocks of beach sand (or the biome's beach material), including the top.
pub const BEACH_SOIL_DEPTH: u8 = 3;

/// Blocks of water-lain seabed material, including the top.
pub const SEABED_SOIL_DEPTH: u8 = 2;

/// Lowest default land soil depth, in blocks including the top.
pub const DEFAULT_SOIL_MIN: u8 = 3;

/// Highest default land soil depth, in blocks including the top.
pub const DEFAULT_SOIL_MAX: u8 = 6;

/// Height above sea level within which a gentle shore is a beach, Q16.16 m.
pub const BEACH_BAND: i32 = 16 << 16;

/// Steepest column that can still be a beach, Q16.16 rise over run (14°).
pub const BEACH_MAX_SLOPE: i32 = ONE / 4;

/// Slope at or above which bedrock is exposed, Q16.16 rise over run (45°).
pub const CLIFF_SLOPE: i32 = ONE;

/// Height above sea level where the universal rock line starts, Q16.16 m.
///
/// Above this the biome palette no longer supplies the top block: mountains
/// show [`HARD_STONE`], and a desert mountain loses its red sand near the peak.
pub const ROCK_LINE: i32 = 2_400 << 16;

/// Height above sea level where the universal snow line starts, Q16.16 m.
///
/// The climate lapse has already made this altitude cold; the surface rule is
/// the geometric half of the same design decision.
pub const SNOW_LINE: i32 = 3_200 << 16;

/// The universal rock material used above [`ROCK_LINE`].
pub const HARD_STONE: &str = "voxel:stone";

/// The snow material used above [`SNOW_LINE`].
pub const SNOW: &str = "voxel:snow";

/// One column's local terrain inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceSample {
    /// Surface height, Q16.16 metres above the world floor.
    pub height: i32,
    /// Local slope, Q16.16 rise over run, non-negative. `ONE` is 45°.
    pub slope: i32,
    /// True within the shore band of an ocean, lake or river.
    pub shore: bool,
    /// Per-column hash for the soil-depth choice.
    ///
    /// Use `hash2(seed, block_x, block_z)` (or the 128 m cell hash) at the call
    /// site; the rules themselves do not know the seed.
    pub hash: u64,
}

impl SurfaceSample {
    /// A sample with every input given explicitly.
    pub const fn new(height: i32, slope: i32, shore: bool, hash: u64) -> Self {
        Self {
            height,
            slope,
            shore,
            hash,
        }
    }

    /// Height relative to sea level, Q16.16 metres; negative underwater.
    pub const fn above_sea(self) -> i32 {
        self.height - SEA_LEVEL
    }

    /// True at or below sea level, matching the macro tile's `WATER` flag.
    pub const fn underwater(self) -> bool {
        self.height <= SEA_LEVEL
    }
}

/// How a column's top was decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// The biome's soil palette and soil-depth range.
    Soil,
    /// A gentle shore within [`BEACH_BAND`] of sea level.
    Beach,
    /// A water-lain floor at or below sea level.
    Seabed,
    /// A slope at or above [`CLIFF_SLOPE`]; bedrock is exposed.
    Cliff,
    /// Above [`ROCK_LINE`] and below [`SNOW_LINE`]; universal rock.
    Rock,
    /// Above [`SNOW_LINE`]; a snow cap over universal rock.
    Snow,
}

/// The materials of one column, from the top down.
///
/// `top` is one block. `filler` is [`filler_depth`](Self::filler_depth) blocks
/// below it, then `stone` continues to the bottom of the surface layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceColumn<'a> {
    /// The material of the exposed top block.
    pub top: &'a str,
    /// The material between the top block and the stone.
    pub filler: &'a str,
    /// The substrate material below the soil.
    pub stone: &'a str,
    /// Total non-stone cover including the top block; `0` exposes stone.
    pub soil_depth: u8,
    /// How the top materials were chosen.
    pub kind: SurfaceKind,
}

impl SurfaceColumn<'_> {
    /// Blocks of `filler` below the top block.
    pub const fn filler_depth(&self) -> u8 {
        self.soil_depth.saturating_sub(1)
    }

    /// True when there is no soil and the top block is the substrate.
    pub const fn is_bare_stone(&self) -> bool {
        self.soil_depth == 0
    }
}

/// The surface column for a biome and a terrain sample.
pub fn column<'a>(biome: &'a BiomeDef, sample: SurfaceSample) -> SurfaceColumn<'a> {
    let above_sea = sample.above_sea();

    // A slope exposes bedrock regardless of biome or water: a cliff face and
    // an underwater canyon wall are both rock, not soil.
    if sample.slope >= CLIFF_SLOPE {
        return SurfaceColumn {
            top: &biome.surface.stone,
            filler: &biome.surface.stone,
            stone: &biome.surface.stone,
            soil_depth: 0,
            kind: SurfaceKind::Cliff,
        };
    }

    // Underwater beds are depositional. An ocean biome's palette already names
    // its shelf/deep/trench floor; a land column under a lake or river uses its
    // beach material (sand, gravel, clay or mud) instead of grass.
    if sample.underwater() {
        let (top, filler) = if biome.class == BiomeClass::Ocean {
            (biome.surface.top.as_str(), biome.surface.filler.as_str())
        } else {
            (biome.surface.beach.as_str(), biome.surface.beach.as_str())
        };
        return SurfaceColumn {
            top,
            filler,
            stone: &biome.surface.stone,
            soil_depth: SEABED_SOIL_DEPTH,
            kind: SurfaceKind::Seabed,
        };
    }

    // The universal altitude override. It deliberately uses HARD_STONE rather
    // than the palette's stone, so a desert peak loses its red rock too.
    if above_sea >= SNOW_LINE {
        return SurfaceColumn {
            top: SNOW,
            filler: HARD_STONE,
            stone: HARD_STONE,
            soil_depth: 1,
            kind: SurfaceKind::Snow,
        };
    }
    if above_sea >= ROCK_LINE {
        return SurfaceColumn {
            top: HARD_STONE,
            filler: HARD_STONE,
            stone: HARD_STONE,
            soil_depth: 0,
            kind: SurfaceKind::Rock,
        };
    }

    // A beach needs a water body and a low, gentle column. Altitude alone is
    // not enough: an inland basin at 10 m must stay soil.
    if sample.shore && above_sea <= BEACH_BAND && sample.slope < BEACH_MAX_SLOPE {
        return SurfaceColumn {
            top: &biome.surface.beach,
            filler: &biome.surface.beach,
            stone: &biome.surface.stone,
            soil_depth: BEACH_SOIL_DEPTH,
            kind: SurfaceKind::Beach,
        };
    }

    // Default soil: the biome's palette and its soil-depth range.
    SurfaceColumn {
        top: &biome.surface.top,
        filler: &biome.surface.filler,
        stone: &biome.surface.stone,
        soil_depth: soil_depth(biome, sample.hash),
        kind: SurfaceKind::Soil,
    }
}

/// The surface column for a catalogue index.
///
/// # Panics
///
/// If `index` is not a biome in the embedded catalogue.
pub fn column_for(index: usize, sample: SurfaceSample) -> SurfaceColumn<'static> {
    column(&catalog().biomes[index], sample)
}

/// Pick a soil depth inside the biome's declared range.
///
/// `soil_depth` is validated to `1..=8` when the catalogue is parsed, so the
/// cast and the span are safe.
pub fn soil_depth(biome: &BiomeDef, hash: u64) -> u8 {
    let (low, high) = biome.soil_depth;
    let span = u64::from(high - low) + 1;
    low + (hash % span) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metres(value: i32) -> i32 {
        value << 16
    }

    fn biome(key: &str) -> &'static BiomeDef {
        catalog()
            .biomes
            .iter()
            .find(|biome| biome.key == key)
            .unwrap_or_else(|| panic!("no biome {key}"))
    }

    /// A flat, dry, mid-altitude column.
    fn soil_sample(hash: u64) -> SurfaceSample {
        SurfaceSample::new(SEA_LEVEL + metres(500), 0, false, hash)
    }

    #[test]
    fn surface_soil_uses_the_biome_palette_and_range() {
        for biome in &catalog().biomes {
            for hash in [0, 1, 7, 41, u64::MAX] {
                let column = column(biome, soil_sample(hash));
                assert_eq!(column.kind, SurfaceKind::Soil, "{}", biome.key);
                assert_eq!(column.top, biome.surface.top, "{}", biome.key);
                assert_eq!(column.filler, biome.surface.filler, "{}", biome.key);
                assert_eq!(column.stone, biome.surface.stone, "{}", biome.key);
                let (low, high) = biome.soil_depth;
                assert!(
                    (low..=high).contains(&column.soil_depth),
                    "{}: depth {} outside {low}..={high}",
                    biome.key,
                    column.soil_depth
                );
                assert_eq!(column.filler_depth(), column.soil_depth - 1);
            }
        }
    }

    #[test]
    fn surface_soil_depth_covers_the_declared_range() {
        let plains = biome("voxel:plains"); // (3, 6)
        let mut seen = [false; 9];
        for hash in 0..64 {
            seen[usize::from(soil_depth(plains, hash))] = true;
        }
        for depth in plains.soil_depth.0..=plains.soil_depth.1 {
            assert!(seen[usize::from(depth)], "depth {depth} was never chosen");
        }
    }

    #[test]
    fn surface_default_land_soil_is_three_to_six_blocks() {
        // The standard land biomes use the design's default range.
        for key in ["voxel:plains", "voxel:forest", "voxel:hills", "voxel:taiga"] {
            let biome = biome(key);
            assert!(
                biome.soil_depth.0 >= DEFAULT_SOIL_MIN && biome.soil_depth.1 <= DEFAULT_SOIL_MAX,
                "{key}: {:?}",
                biome.soil_depth
            );
            for hash in 0..32 {
                let depth = soil_depth(biome, hash);
                assert!(
                    (DEFAULT_SOIL_MIN..=DEFAULT_SOIL_MAX).contains(&depth),
                    "{key}: {depth}"
                );
            }
        }

        // Dry, cold, coastal and mountain biomes override the default
        // shallower; that is the biome half of "biome/altitude override".
        for key in [
            "voxel:desert",
            "voxel:savanna",
            "voxel:tundra",
            "voxel:swamp",
            "voxel:beach",
            "voxel:mountains",
            "voxel:alpine",
        ] {
            assert!(
                biome(key).soil_depth.0 < DEFAULT_SOIL_MIN,
                "{key} should override the default soil depth"
            );
        }
    }

    #[test]
    fn surface_cliffs_expose_stone_in_every_biome() {
        for biome in &catalog().biomes {
            for height in [SEA_LEVEL - metres(100), SEA_LEVEL + metres(500)] {
                let column = column(biome, SurfaceSample::new(height, CLIFF_SLOPE, false, 0));
                assert_eq!(column.kind, SurfaceKind::Cliff, "{}", biome.key);
                assert_eq!(column.top, biome.surface.stone, "{}", biome.key);
                assert_eq!(column.filler, biome.surface.stone, "{}", biome.key);
                assert_eq!(column.stone, biome.surface.stone, "{}", biome.key);
                assert_eq!(column.soil_depth, 0, "{}", biome.key);
                assert!(column.is_bare_stone(), "{}", biome.key);
            }

            // One Q16.16 unit below the threshold is still soil/seabed.
            let gentler = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + metres(500), CLIFF_SLOPE - 1, false, 0),
            );
            assert_eq!(gentler.kind, SurfaceKind::Soil, "{}", biome.key);
        }
    }

    #[test]
    fn surface_beaches_ring_water_only() {
        for biome in &catalog().biomes {
            let shore = column(biome, SurfaceSample::new(SEA_LEVEL + metres(8), 0, true, 0));
            assert_eq!(shore.kind, SurfaceKind::Beach, "{}", biome.key);
            assert_eq!(shore.top, biome.surface.beach, "{}", biome.key);
            assert_eq!(shore.filler, biome.surface.beach, "{}", biome.key);
            assert_eq!(shore.stone, biome.surface.stone, "{}", biome.key);
            assert_eq!(shore.soil_depth, BEACH_SOIL_DEPTH, "{}", biome.key);

            // Same height and slope, but no water body: it is inland soil.
            let inland = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + metres(8), 0, false, 0),
            );
            assert_eq!(inland.kind, SurfaceKind::Soil, "{}", biome.key);

            // A steep shore is a cliff, not sand.
            let steep = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + metres(8), BEACH_MAX_SLOPE, true, 0),
            );
            assert_eq!(steep.kind, SurfaceKind::Soil, "{}", biome.key);

            // Above the beach band the shore is soil again.
            let high = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + BEACH_BAND + 1, 0, true, 0),
            );
            assert_eq!(high.kind, SurfaceKind::Soil, "{}", biome.key);
        }
    }

    #[test]
    fn surface_beach_band_is_inclusive_at_sixteen_metres() {
        let plains = biome("voxel:plains");
        assert_eq!(
            column(plains, SurfaceSample::new(SEA_LEVEL + 1, 0, true, 0)).kind,
            SurfaceKind::Beach
        );
        assert_eq!(
            column(
                plains,
                SurfaceSample::new(SEA_LEVEL + BEACH_BAND, 0, true, 0)
            )
            .kind,
            SurfaceKind::Beach
        );
        assert_eq!(
            column(
                plains,
                SurfaceSample::new(SEA_LEVEL + BEACH_BAND + 1, 0, true, 0)
            )
            .kind,
            SurfaceKind::Soil
        );
    }

    #[test]
    fn surface_underwater_uses_the_seabed_palette() {
        for biome in &catalog().biomes {
            let column = column(
                biome,
                SurfaceSample::new(SEA_LEVEL - metres(20), 0, true, 0),
            );
            assert_eq!(column.kind, SurfaceKind::Seabed, "{}", biome.key);
            if biome.class == BiomeClass::Ocean {
                assert_eq!(column.top, biome.surface.top, "{}", biome.key);
                assert_eq!(column.filler, biome.surface.filler, "{}", biome.key);
            } else {
                // A lake or river bed in a land biome is sand/gravel/clay, not
                // grass and dirt.
                assert_eq!(column.top, biome.surface.beach, "{}", biome.key);
                assert_eq!(column.filler, biome.surface.beach, "{}", biome.key);
            }
            assert_eq!(column.stone, biome.surface.stone, "{}", biome.key);
            assert_eq!(column.soil_depth, SEABED_SOIL_DEPTH, "{}", biome.key);
        }
    }

    #[test]
    fn surface_altitude_overrides_every_biome_palette() {
        let desert = biome("voxel:desert");
        for biome in &catalog().biomes {
            let rock = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + ROCK_LINE, 0, false, 0),
            );
            assert_eq!(rock.kind, SurfaceKind::Rock, "{}", biome.key);
            assert_eq!(rock.top, HARD_STONE, "{}", biome.key);
            assert_eq!(rock.filler, HARD_STONE, "{}", biome.key);
            assert_eq!(rock.stone, HARD_STONE, "{}", biome.key);
            assert!(rock.is_bare_stone(), "{}", biome.key);

            let snow = column(
                biome,
                SurfaceSample::new(SEA_LEVEL + SNOW_LINE, 0, false, 0),
            );
            assert_eq!(snow.kind, SurfaceKind::Snow, "{}", biome.key);
            assert_eq!(snow.top, SNOW, "{}", biome.key);
            assert_eq!(snow.filler, HARD_STONE, "{}", biome.key);
            assert_eq!(snow.stone, HARD_STONE, "{}", biome.key);
            assert_eq!(snow.soil_depth, 1, "{}", biome.key);
        }

        // The desert's red sand is gone at the rock line.
        let below = column(
            desert,
            SurfaceSample::new(SEA_LEVEL + ROCK_LINE - 1, 0, false, 0),
        );
        assert_eq!(below.top, "voxel:red_sand");
        let above = column(
            desert,
            SurfaceSample::new(SEA_LEVEL + ROCK_LINE, 0, false, 0),
        );
        assert_eq!(above.top, HARD_STONE);
    }

    #[test]
    fn surface_rules_are_deterministic() {
        let plains = biome("voxel:plains");
        let sample = SurfaceSample::new(SEA_LEVEL + metres(12), ONE / 8, true, 0xDEAD_BEEF);
        let first = column(plains, sample);
        let second = column(plains, sample);
        assert_eq!(first, second);
        assert_eq!(first.kind, SurfaceKind::Beach);
    }
}
