//! M2.5-12: surface rules against the committed content and manifest.
//!
//! The unit tests in `src/surface.rs` cover the rule precedence; this suite
//! connects the output to the two data sources the task names: the 18 biome
//! palettes in `data/biomes.ron` and the block manifest in
//! `crates/engine/block/data/blocks.ron`. Every top, filler, stone and beach
//! key the rules can emit must resolve, and every palette role must actually
//! reach the output somewhere in the sample matrix.

use voxel_block::BlockRegistry;
use voxel_block::BlockRegistryBuilder;
use voxel_worldgen::biomes::{BiomeClass, catalog};
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::splines::SEA_LEVEL;
use voxel_worldgen::surface::{
    BEACH_BAND, CLIFF_SLOPE, HARD_STONE, ROCK_LINE, SNOW, SNOW_LINE, SurfaceKind, SurfaceSample,
    column, column_for,
};

fn metres(value: i32) -> i32 {
    value << 16
}

fn registry() -> BlockRegistry {
    let blocks = include_str!("../../block/data/blocks.ron");
    let materials = include_str!("../../block/data/materials.gen.ron");
    let mut builder = BlockRegistryBuilder::new();
    builder
        .load_ron_with_materials(blocks, materials)
        .expect("committed blocks and materials load");
    builder.freeze().expect("blocks freeze")
}

/// One sample per rule, plus a spread of soil hashes.
fn samples() -> Vec<SurfaceSample> {
    let mut samples = vec![
        // Soil at a mid altitude, away from water.
        SurfaceSample::new(SEA_LEVEL + metres(500), 0, false, 0),
        // Beach: low, gentle, on a shore.
        SurfaceSample::new(SEA_LEVEL + metres(8), 0, true, 0),
        // Seabed, including a land biome under a lake or river.
        SurfaceSample::new(SEA_LEVEL - metres(20), 0, true, 0),
        // Cliff above and below the waterline.
        SurfaceSample::new(SEA_LEVEL + metres(500), CLIFF_SLOPE, false, 0),
        SurfaceSample::new(SEA_LEVEL - metres(200), CLIFF_SLOPE, true, 0),
        // Universal rock and snow.
        SurfaceSample::new(SEA_LEVEL + ROCK_LINE, 0, false, 0),
        SurfaceSample::new(SEA_LEVEL + SNOW_LINE, 0, false, 0),
        // Boundaries: one unit either side of the beach band and the slope.
        SurfaceSample::new(SEA_LEVEL + BEACH_BAND, 0, true, 0),
        SurfaceSample::new(SEA_LEVEL + BEACH_BAND + 1, 0, true, 0),
        SurfaceSample::new(SEA_LEVEL + metres(8), ONE / 4, true, 0),
    ];
    for hash in [1, 7, 41, 0xDEAD_BEEF, u64::MAX] {
        samples.push(SurfaceSample::new(SEA_LEVEL + metres(500), 0, false, hash));
    }
    samples
}

#[test]
fn surface_every_output_key_resolves_in_the_manifest() {
    let registry = registry();
    let samples = samples();
    for biome in &catalog().biomes {
        for sample in &samples {
            let column = column(biome, *sample);
            for (role, key) in [
                ("top", column.top),
                ("filler", column.filler),
                ("stone", column.stone),
            ] {
                assert!(
                    registry.id_of(key).is_some(),
                    "{} {:?} {role} key {key:?} is not in the block manifest",
                    biome.key,
                    column.kind
                );
            }
        }
    }
}

#[test]
fn surface_every_palette_role_reaches_the_output() {
    let samples = samples();
    for biome in &catalog().biomes {
        let mut top = false;
        let mut filler = false;
        let mut stone = false;
        let mut beach = false;
        for sample in &samples {
            let column = column(biome, *sample);
            top |= column.top == biome.surface.top;
            filler |= column.filler == biome.surface.filler;
            stone |= column.stone == biome.surface.stone;
            beach |= column.top == biome.surface.beach;
        }
        assert!(top, "{} never emits its top material", biome.key);
        assert!(filler, "{} never emits its filler material", biome.key);
        assert!(stone, "{} never emits its stone material", biome.key);
        assert!(beach, "{} never emits its beach material", biome.key);
    }
}

#[test]
fn surface_canonical_keys_resolve() {
    let registry = registry();
    for key in [HARD_STONE, SNOW] {
        assert!(
            registry.id_of(key).is_some(),
            "canonical surface key {key:?} is not in the block manifest"
        );
    }
}

#[test]
fn surface_catalogue_index_matches_the_definition_api() {
    for (index, biome) in catalog().biomes.iter().enumerate() {
        let sample = SurfaceSample::new(SEA_LEVEL + metres(500), 0, false, 11);
        assert_eq!(column_for(index, sample), column(biome, sample));
    }
}

#[test]
fn surface_ocean_biomes_are_reachable_underwater() {
    // The shelf/deep/trench palettes must be what a submerged ocean column
    // uses; the rule is not allowed to fall back to a land beach there.
    for biome in &catalog().biomes {
        if biome.class != BiomeClass::Ocean {
            continue;
        }
        let column = column(
            biome,
            SurfaceSample::new(SEA_LEVEL - metres(300), 0, true, 0),
        );
        assert_eq!(column.kind, SurfaceKind::Seabed, "{}", biome.key);
        assert_eq!(column.top, biome.surface.top, "{}", biome.key);
        assert_eq!(column.filler, biome.surface.filler, "{}", biome.key);
    }
}
