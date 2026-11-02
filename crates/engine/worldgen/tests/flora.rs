//! M2.5-21: flora species, density, ecotones and giants.

use std::collections::{BTreeMap, BTreeSet};

use voxel_block::{BlockRegistry, BlockRegistryBuilder};
use voxel_worldgen::biomes::{BiomeClass, BiomeDef, catalog};
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::flora::{
    FLORA_MAX_SLOPE, FloraContext, FloraField, FloraKind, FloraPlacement, GIANT_CELL_M,
    GIANT_MAX_HEIGHT, GIANT_MIN_HEIGHT, SEAM_WEIGHT, is_tree_key, leaves_for,
};
use voxel_worldgen::splines::{SEA_LEVEL, SEA_LEVEL_M};
use voxel_worldgen::weights::{Axes, BiomeWeights};

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

fn context_for(biome: &BiomeDef, surface_m: i32) -> FloraContext {
    let centre = &biome.centre;
    FloraContext {
        surface: (surface_m << 16),
        continentalness: ((centre.continentalness * 2.0 - 1.0) * ONE as f64) as i32,
        erosion: (centre.erosion * ONE as f64) as i32,
        temperature: (centre.temperature * 255.0) as u8,
        humidity: (centre.humidity * 255.0) as u8,
        slope: 0,
    }
}

fn biome(key: &str) -> &'static BiomeDef {
    catalog()
        .get(key)
        .unwrap_or_else(|| panic!("no biome {key}"))
}

fn scan(field: &FloraField, context: &FloraContext, size: i32) -> Vec<FloraPlacement> {
    let mut placements = Vec::new();
    for z in 0..size {
        for x in 0..size {
            if let Some(placement) = field.sample(metres(x), metres(z), context) {
                placements.push(placement);
            }
        }
    }
    placements
}

#[test]
fn flora_species_keys_resolve_in_the_manifest() {
    let registry = registry();
    let mut keys = BTreeSet::new();
    for biome in &catalog().biomes {
        for entry in &biome.flora {
            keys.insert(entry.species.clone());
        }
        keys.extend(biome.ecotone.iter().cloned());
    }
    for key in &keys {
        let id = registry
            .id_of(key)
            .unwrap_or_else(|| panic!("species {key:?} is not in the manifest"));
        assert!(
            registry.face_materials(id).is_some(),
            "species {key:?} has no atlas materials"
        );
        if is_tree_key(key) {
            let leaves = leaves_for(key).unwrap_or_else(|| panic!("{key} has no leaves"));
            let id = registry
                .id_of(leaves)
                .unwrap_or_else(|| panic!("leaves {leaves:?} are not in the manifest"));
            assert!(registry.face_materials(id).is_some());
        }
    }

    // The M2.5-31 flora keys must all be exercised by the catalogue.
    for key in [
        "voxel:birch_log",
        "voxel:birch_leaves",
        "voxel:spruce_log",
        "voxel:spruce_leaves",
        "voxel:fern",
        "voxel:cactus",
        "voxel:dead_bush",
    ] {
        if key.ends_with("_leaves") {
            assert!(
                keys.iter().any(|species| leaves_for(species) == Some(key)),
                "{key:?} is not the leaves of any species"
            );
        } else {
            assert!(keys.contains(key), "{key:?} is not a species or ecotone");
        }
    }
}

#[test]
fn flora_density_is_moderate_and_biome_dependent() {
    let field = FloraField::new(1);
    let size = 128;
    let area = (size * size) as usize;
    for biome in &catalog().biomes {
        if biome.class != BiomeClass::Land {
            continue;
        }
        let context = context_for(biome, SEA_LEVEL_M as i32 + 500);
        let placements = scan(&field, &context, size);
        let total = placements.len();
        let trees = placements
            .iter()
            .filter(|placement| placement.kind == FloraKind::Tree)
            .count();
        let plants = total - trees;
        let trees_in_catalog = biome.flora.iter().any(|entry| is_tree_key(&entry.species));

        if biome.flora.is_empty() {
            assert_eq!(total, 0, "{} should be barren", biome.key);
            continue;
        }
        assert!(total > 0, "{} has no flora", biome.key);
        assert!(
            total * 100 / area <= 75,
            "{} flora covers {}%",
            biome.key,
            total * 100 / area
        );
        if trees_in_catalog {
            assert!(trees > 0, "{} has no trees", biome.key);
        } else {
            assert_eq!(trees, 0, "{} has unexpected trees", biome.key);
        }
        assert!(plants > 0, "{} has no plants", biome.key);

        let mut species: BTreeMap<&str, usize> = BTreeMap::new();
        for placement in &placements {
            *species.entry(placement.species).or_insert(0) += 1;
        }
        for entry in &biome.flora {
            assert!(
                species.get(entry.species.as_str()).copied().unwrap_or(0) > 0,
                "{}: {:?} density {} produced nothing in {area} columns",
                biome.key,
                entry.species,
                entry.density
            );
        }
    }

    // A few biome-specific species, not just “some plant”.
    assert!(
        scan(&field, &context_for(biome("voxel:forest"), 7_500), 64)
            .iter()
            .any(|placement| placement.species == "voxel:oak_log")
    );
    assert!(
        scan(&field, &context_for(biome("voxel:taiga"), 7_500), 64)
            .iter()
            .any(|placement| placement.species == "voxel:spruce_log")
    );
    assert!(
        scan(&field, &context_for(biome("voxel:desert"), 7_500), 64)
            .iter()
            .any(|placement| placement.species == "voxel:cactus")
    );
    assert!(
        scan(&field, &context_for(biome("voxel:plains"), 7_500), 64)
            .iter()
            .any(|placement| placement.species == "voxel:tall_grass")
    );
}

#[test]
fn flora_ecotones_only_occur_at_seams() {
    let field = FloraField::new(1);
    let forest = biome("voxel:forest");
    let taiga = biome("voxel:taiga");

    let mut birch = 0;
    let mut ecotone = 0;
    for step in 0..10_000 {
        let t = step as f64 / 9_999.0;
        let context = FloraContext {
            surface: 7_500 << 16,
            continentalness: (((forest.centre.continentalness * (1.0 - t)
                + taiga.centre.continentalness * t)
                * 2.0
                - 1.0)
                * ONE as f64) as i32,
            erosion: ((forest.centre.erosion * (1.0 - t) + taiga.centre.erosion * t) * ONE as f64)
                as i32,
            temperature: ((forest.centre.temperature * (1.0 - t) + taiga.centre.temperature * t)
                * 255.0) as u8,
            humidity: ((forest.centre.humidity * (1.0 - t) + taiga.centre.humidity * t) * 255.0)
                as u8,
            slope: 0,
        };
        let axes = Axes::surface(
            context.continentalness,
            context.temperature,
            context.humidity,
            context.erosion,
        );
        let weights = BiomeWeights::at(&axes);
        let second = {
            let mut first = 0;
            let mut second = 0;
            for weight in weights.weights() {
                if *weight >= first {
                    second = first;
                    first = *weight;
                } else if *weight > second {
                    second = *weight;
                }
            }
            second
        };
        if let Some(placement) = field.sample(metres(step), 0, &context) {
            if placement.ecotone {
                ecotone += 1;
                assert!(
                    second >= SEAM_WEIGHT,
                    "ecotone {:?} outside the seam (second {second})",
                    placement.species
                );
                if placement.species == "voxel:birch_log" {
                    birch += 1;
                    assert_eq!(placement.kind, FloraKind::Tree);
                }
            }
            if placement.species == "voxel:birch_log" {
                assert!(placement.ecotone, "birch must be an ecotone");
            }
        }
    }
    assert!(ecotone > 0, "the forest/taiga transect has no ecotone");
    assert!(birch > 0, "the forest/taiga transect has no birch");

    // A biome interior does not draw ecotone species, even though the normal
    // desert dead bush can still grow there.
    let interior = scan(&field, &context_for(forest, 7_500), 64);
    assert!(interior.iter().all(|placement| !placement.ecotone));
    let desert = scan(&field, &context_for(biome("voxel:desert"), 7_500), 64);
    assert!(desert.iter().all(|placement| !placement.ecotone));
    assert!(
        desert
            .iter()
            .any(|placement| placement.species == "voxel:dead_bush" && !placement.ecotone)
    );
}

#[test]
fn flora_giants_are_rare_and_tall() {
    let field = FloraField::new(1);
    assert!((20_000..=50_000).contains(&GIANT_CELL_M));
    let forest = context_for(biome("voxel:forest"), 7_500);
    let mut giants = 0;
    for cell_z in 0..8 {
        for cell_x in 0..8 {
            let (x, z) = field.giant_anchor(cell_x, cell_z);
            let placement = field
                .sample(x, z, &forest)
                .unwrap_or_else(|| panic!("no giant at cell ({cell_x}, {cell_z})"));
            assert!(placement.giant, "cell ({cell_x}, {cell_z}) is not a giant");
            assert!(placement.kind == FloraKind::Tree);
            assert!((GIANT_MIN_HEIGHT..=GIANT_MAX_HEIGHT).contains(&placement.height));
            assert!(leaves_for(placement.species).is_some());
            giants += 1;
        }
    }
    assert_eq!(giants, 64);

    // Plains has no tree entries, so a giant anchor stays a normal plant or
    // empty rather than inventing a tree.
    let plains = context_for(biome("voxel:plains"), 7_500);
    for cell_z in 0..8 {
        for cell_x in 0..8 {
            let (x, z) = field.giant_anchor(cell_x, cell_z);
            if let Some(placement) = field.sample(x, z, &plains) {
                assert!(!placement.giant);
            }
        }
    }
}

#[test]
fn flora_stays_off_water_and_cliffs() {
    let field = FloraField::new(1);
    let forest = biome("voxel:forest");
    let mut context = context_for(forest, 7_500);
    context.surface = SEA_LEVEL;
    assert!(field.sample(metres(10), metres(10), &context).is_none());

    let mut context = context_for(forest, 7_500);
    context.slope = FLORA_MAX_SLOPE + 1;
    assert!(field.sample(metres(10), metres(10), &context).is_none());

    // Determinism.
    let context = context_for(forest, 7_500);
    let a = field.sample(metres(11), metres(13), &context);
    let b = field.sample(metres(11), metres(13), &context);
    assert_eq!(a, b);
    assert_eq!(field.seed(), 1);
}
