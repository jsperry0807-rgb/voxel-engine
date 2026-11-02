//! M2.5-15: local-surface cave depth and underground biome assignment.
//!
//! The unit tests in `src/cave_depth.rs` pin the depth arithmetic and the four
//! representative assignments. This suite connects the assignment to the real
//! content: every underground palette key resolves in the committed block
//! manifest, and every carved voxel in a sample field receives an
//! underground/rare biome whose materials resolve.

use std::collections::BTreeSet;

use voxel_block::{BlockRegistry, BlockRegistryBuilder};
use voxel_worldgen::biomes::{BiomeClass, catalog};
use voxel_worldgen::cave_depth::{CaveDepth, UndergroundClimate, assign_biome, assign_key, axes};
use voxel_worldgen::caves::{CaveContext, CaveField, CaveFill, MAX_CAVE_DEPTH_M};
use voxel_worldgen::fixed::ONE;

fn metres(value: i32) -> i64 {
    (value as i64) << 16
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

fn climate(temperature: u8, humidity: u8, erosion: i32, water: bool) -> UndergroundClimate {
    UndergroundClimate::new(temperature, humidity, erosion, water)
}

fn assert_resolves(registry: &BlockRegistry, biome_key: &str, keys: [&str; 4]) {
    for key in keys {
        let id = registry
            .id_of(key)
            .unwrap_or_else(|| panic!("{biome_key}: {key:?} is not in the manifest"));
        assert!(
            registry.face_materials(id).is_some(),
            "{biome_key}: {key:?} has no atlas materials"
        );
    }
}

#[test]
fn cave_depth_underground_material_keys_resolve() {
    let registry = registry();
    let mut seen = BTreeSet::new();
    for biome in &catalog().biomes {
        if !matches!(biome.class, BiomeClass::Underground | BiomeClass::Rare) {
            continue;
        }
        assert_resolves(&registry, &biome.key, biome.surface.keys());
        seen.extend(biome.surface.keys());
    }

    // The M2.5-31 underground keys must be exercised, not merely present in
    // some unrelated palette.
    for key in [
        "voxel:clay",
        "voxel:deep_stone",
        "voxel:fungal_cap",
        "voxel:fungal_stem",
        "voxel:geothermal_rock",
        "voxel:obsidian",
        "voxel:stone",
        "voxel:gravel",
        "voxel:sand",
    ] {
        assert!(
            seen.contains(key),
            "{key:?} is missing from the underground palettes"
        );
    }
}

#[test]
fn cave_depth_assignment_grid_resolves() {
    let registry = registry();
    for depth_m in [0, 100, 600, 1_200, 1_500, 1_800, 2_000] {
        let depth = CaveDepth::new(metres(5_000), metres(5_000 - depth_m));
        assert_eq!(depth.metres(), depth_m);
        for water in [false, true] {
            for temperature in [0u8, 128, 255] {
                for humidity in [0u8, 128, 255] {
                    let biome = assign_biome(depth, climate(temperature, humidity, ONE / 2, water))
                        .expect("an underground biome");
                    assert!(
                        matches!(biome.class, BiomeClass::Underground | BiomeClass::Rare),
                        "assigned {} at {depth_m} m",
                        biome.key
                    );
                    assert_resolves(&registry, &biome.key, biome.surface.keys());
                }
            }
        }
    }
}

#[test]
fn cave_depth_assignment_follows_depth_and_water() {
    // At the cavern/aquifer depth, dry erosion picks the cavern and water
    // flips it to the aquifer.
    let depth = CaveDepth::new(metres(2_000), metres(2_000 - 1_300));
    let dry = assign_key(depth, climate(128, 102, ONE * 6 / 10, false));
    let wet = assign_key(depth, climate(128, 102, ONE * 6 / 10, true));
    assert_eq!(dry, Some("voxel:cavern"));
    assert_eq!(wet, Some("voxel:aquifer"));
    assert_ne!(dry, wet);

    // The three depth bands.
    assert_eq!(
        assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_200)),
            climate(128, 102, ONE * 4 / 10, true)
        ),
        Some("voxel:aquifer")
    );
    assert_eq!(
        assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_500)),
            climate(128, 128, ONE * 3 / 10, false)
        ),
        Some("voxel:fungal_deep")
    );
    assert_eq!(
        assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_300)),
            climate(128, 102, ONE * 6 / 10, false)
        ),
        Some("voxel:cavern")
    );
}

#[test]
fn cave_depth_follows_the_local_surface() {
    // The same 500 m below two different surfaces is the same normalised
    // depth, even though the absolute heights differ by 7,000 m.
    let mountain = CaveDepth::new(metres(9_000), metres(8_500));
    let valley = CaveDepth::new(metres(2_000), metres(1_500));
    assert_eq!(mountain.metres(), 500);
    assert_eq!(valley.metres(), 500);
    assert_eq!(mountain.normalized(), valley.normalized());
    assert_eq!(
        mountain.normalized(),
        axes(mountain, climate(128, 128, ONE / 2, false)).depth
    );

    // The same absolute height is a different depth under a different surface.
    let shallow = CaveDepth::new(metres(1_000), metres(500));
    let deep = CaveDepth::new(metres(2_000), metres(500));
    assert_ne!(shallow.normalized(), deep.normalized());
    assert!(!CaveDepth::new(metres(1_000), metres(1_000 - MAX_CAVE_DEPTH_M - 1)).within_budget());
    assert!(CaveDepth::new(metres(1_000), metres(1_000 - MAX_CAVE_DEPTH_M)).within_budget());
}

#[test]
fn cave_depth_carved_voxels_assign_resolvable_content() {
    let registry = registry();
    let field = CaveField::new(1);
    let surface = 48;
    let context = CaveContext::land(metres(surface), metres(surface / 2), metres(surface / 6));
    let mut seen = BTreeSet::new();
    let mut carved = 0usize;
    for x in 0..32 {
        for z in 0..32 {
            for y in 0..=surface {
                let Some(voxel) = field.sample(metres(x), metres(y), metres(z), &context) else {
                    continue;
                };
                let depth = CaveDepth::new(metres(surface), metres(y));
                assert!(
                    depth.within_budget(),
                    "carved outside the budget at {depth:?}"
                );
                let biome = assign_biome(
                    depth,
                    climate(128, 128, ONE / 2, voxel.fill == CaveFill::Water),
                )
                .expect("an underground biome");
                assert!(
                    matches!(biome.class, BiomeClass::Underground | BiomeClass::Rare),
                    "assigned {} at ({x}, {y}, {z})",
                    biome.key
                );
                assert_resolves(&registry, &biome.key, biome.surface.keys());
                seen.insert(biome.key.as_str());
                carved += 1;
            }
        }
    }
    assert!(carved > 0, "the sample field carved nothing");
    assert!(!seen.is_empty());
}
