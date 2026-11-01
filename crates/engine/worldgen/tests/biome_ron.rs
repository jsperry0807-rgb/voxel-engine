//! M2.5-11: the RON biome catalogue.
//!
//! The committed file must parse, hold the 18 designed biomes with the right
//! class counts, resolve every material and flora key against the block
//! manifest, and cover the climate space so every sample has a dominant biome.
//! The rejection tests mutate a parsed copy, so the structural rules are
//! exercised without brittle RON strings.

use voxel_block::BlockRegistryBuilder;
use voxel_worldgen::biomes::{BiomeCatalog, BiomeClass, EMBEDDED_BIOMES, catalog};
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::weights::{Axes, BIOME_COUNT, BiomeWeights};

fn registry() -> voxel_block::BlockRegistry {
    let blocks = include_str!("../../block/data/blocks.ron");
    let materials = include_str!("../../block/data/materials.gen.ron");
    let mut builder = BlockRegistryBuilder::new();
    builder
        .load_ron_with_materials(blocks, materials)
        .expect("committed blocks and materials load");
    builder.freeze().expect("blocks freeze")
}

#[test]
fn biome_ron_catalog_has_the_designed_shape() {
    let catalog = catalog();
    assert_eq!(catalog.len(), BIOME_COUNT);
    for class in [
        BiomeClass::Land,
        BiomeClass::Ocean,
        BiomeClass::Underground,
        BiomeClass::Rare,
    ] {
        let found = catalog
            .biomes
            .iter()
            .filter(|biome| biome.class == class)
            .count();
        assert_eq!(found, class.required_count(), "{class:?}");
    }

    let mut keys: Vec<&str> = catalog
        .biomes
        .iter()
        .map(|biome| biome.key.as_str())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), BIOME_COUNT, "biome keys must be unique");

    for biome in &catalog.biomes {
        assert_eq!(
            biome.surface.keys().len(),
            4,
            "{} needs four surface keys",
            biome.key
        );
        let (low, high) = biome.soil_depth;
        assert!(1 <= low && low <= high && high <= 8, "{}", biome.key);
        for flora in &biome.flora {
            assert!((0.0..=1.0).contains(&flora.density), "{}", biome.key);
        }
    }
}

#[test]
fn biome_ron_every_key_resolves_in_the_block_manifest() {
    let registry = registry();
    catalog()
        .validate(|key| registry.id_of(key).is_some())
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn biome_ron_reports_the_first_unknown_key() {
    let registry = registry();
    let mut edited = catalog().clone();
    edited.biomes[0].surface.top = "voxel:not_a_block".to_owned();
    let error = edited
        .validate(|key| registry.id_of(key).is_some())
        .expect_err("unknown material must be rejected");
    let message = error.to_string();
    assert!(message.contains("voxel:not_a_block"), "{message}");
    assert!(message.contains(&edited.biomes[0].key), "{message}");

    assert!(
        catalog()
            .validate(|_| false)
            .expect_err("no key can resolve")
            .to_string()
            .contains("not in the block manifest")
    );
}

#[test]
fn biome_ron_every_climate_cell_resolves() {
    let mut dominated = [false; BIOME_COUNT];
    for c in -8..=8 {
        for temperature in [0u8, 64, 128, 191, 255] {
            for humidity in [0u8, 64, 128, 191, 255] {
                for erosion in [0i32, ONE / 2, ONE] {
                    for depth in [0i32, ONE / 2, 3 * ONE / 5, 2 * ONE / 3, 3 * ONE / 4, ONE] {
                        let axes = Axes {
                            continentalness: (c * ONE / 8 + ONE) / 2,
                            temperature: temperature as i32 * ONE / 255,
                            humidity: humidity as i32 * ONE / 255,
                            erosion,
                            depth,
                        };
                        let weights = BiomeWeights::at(&axes);
                        assert_eq!(
                            weights.weights().iter().sum::<i32>(),
                            ONE,
                            "weights must sum to ONE at {axes:?}"
                        );
                        let dominant = weights.dominant();
                        assert!(
                            weights.weight(dominant) >= ONE / BIOME_COUNT as i32,
                            "no dominant biome at {axes:?}"
                        );
                        dominated[dominant] = true;
                    }
                }
            }
        }
    }
    let missing: Vec<&str> = catalog()
        .biomes
        .iter()
        .zip(dominated)
        .filter(|(_, seen)| !seen)
        .map(|(biome, _)| biome.key.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "never dominant in the sweep: {missing:?}"
    );
}

#[test]
fn biome_ron_centres_match_the_weight_input() {
    let embedded = voxel_worldgen::biomes::embedded_centres();
    assert_eq!(embedded, &catalog().centres_q16());
    // Beach's first centre axis is 0.52 in the file.
    assert_eq!(embedded[0][0], (0.52 * ONE as f64).round() as i32);
}

#[test]
fn biome_ron_rejects_a_short_file() {
    let mut edited = catalog().clone();
    edited.biomes.pop();
    assert!(matches!(
        edited.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::WrongCount { found: 17 })
    ));
}

#[test]
fn biome_ron_rejects_duplicate_and_bad_keys() {
    let mut duplicate = catalog().clone();
    duplicate.biomes[1].key = duplicate.biomes[0].key.clone();
    assert!(matches!(
        duplicate.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::DuplicateKey { .. })
    ));

    let mut bad = catalog().clone();
    bad.biomes[0].key = "beach".to_owned();
    assert!(matches!(
        bad.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadKey { .. })
    ));
}

#[test]
fn biome_ron_rejects_bad_ranges_and_units() {
    let mut inverted = catalog().clone();
    inverted.biomes[0].climate.temperature = (0.8, 0.2);
    assert!(matches!(
        inverted.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadRange { .. })
    ));

    let mut unit = catalog().clone();
    unit.biomes[0].centre.temperature = 1.5;
    assert!(matches!(
        unit.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadUnit { .. })
    ));

    let mut fog = catalog().clone();
    fog.biomes[0].fog.1 = -0.1;
    assert!(matches!(
        fog.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadUnit { .. })
    ));
}

#[test]
fn biome_ron_rejects_bad_soil_and_density() {
    let mut soil = catalog().clone();
    soil.biomes[0].soil_depth = (0, 3);
    assert!(matches!(
        soil.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadSoilDepth { .. })
    ));

    let mut density = catalog().clone();
    let biome = density
        .biomes
        .iter_mut()
        .find(|biome| !biome.flora.is_empty())
        .expect("some biome has flora");
    biome.flora[0].density = 2.0;
    assert!(matches!(
        density.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::BadDensity { .. })
    ));
}

#[test]
fn biome_ron_rejects_a_wrong_class_count() {
    let mut edited = catalog().clone();
    edited.biomes[1].class = BiomeClass::Ocean;
    assert!(matches!(
        edited.check_structure(),
        Err(voxel_worldgen::biomes::BiomeError::WrongClassCount { .. })
    ));
}

#[test]
fn biome_ron_rejects_broken_ron() {
    assert!(matches!(
        BiomeCatalog::parse("not a catalog"),
        Err(voxel_worldgen::biomes::BiomeError::Parse(_))
    ));
}

#[test]
fn biome_ron_content_is_data() {
    // An edited file parses, and its edited centre is what the weights use.
    let edited = EMBEDDED_BIOMES.replace(
        "centre: (continentalness: 0.52, temperature: 0.50, humidity: 0.50, erosion: 0.40, depth: 0.0)",
        "centre: (continentalness: 0.12, temperature: 0.50, humidity: 0.50, erosion: 0.40, depth: 0.0)",
    );
    assert_ne!(edited, EMBEDDED_BIOMES, "the probe text must match");
    let parsed = BiomeCatalog::parse(&edited).expect("edited content parses");
    assert_eq!(parsed.biomes[0].centre.continentalness, 0.12);

    // The edited centre changes the weight of that biome somewhere.
    let axes = Axes {
        continentalness: (0.52 * ONE as f64) as i32,
        temperature: ONE / 2,
        humidity: ONE / 2,
        erosion: (0.40 * ONE as f64) as i32,
        depth: 0,
    };
    let original = BiomeWeights::at_with(&axes, &catalog().centres_q16());
    let changed = BiomeWeights::at_with(&axes, &parsed.centres_q16());
    assert_ne!(original.weights(), changed.weights());
}

#[test]
fn biome_ron_catalog_lookup_and_sizes() {
    let catalog = catalog();
    assert!(!catalog.is_empty());
    let beach = catalog.get("voxel:beach").expect("beach");
    assert_eq!(beach.class, BiomeClass::Land);
    assert_eq!(beach.surface.top, "voxel:sand");
    assert!(catalog.get("voxel:nope").is_none());
}
