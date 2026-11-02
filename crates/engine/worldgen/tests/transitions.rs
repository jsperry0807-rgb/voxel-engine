//! M2.5-13: the 1 m surface dither and canonical transition templates.
//!
//! The task's acceptance points are connected to real content here:
//!
//! - the dither follows the M2.5-10 weights rather than an argmax, so a seam
//!   is a mixed band and not a one-block line;
//! - every material pair has at most four canonical variants;
//! - every template uses only its two parents plus one debris key, all of
//!   which already exist in the committed block/atlas manifest. A template
//!   introduces no material key, so it costs zero atlas slots.

use std::collections::BTreeSet;

use voxel_block::{BlockRegistry, BlockRegistryBuilder};
use voxel_worldgen::biomes::catalog;
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::hash;
use voxel_worldgen::splines::SEA_LEVEL;
use voxel_worldgen::surface::{SNOW_LINE, SurfaceSample, column};
use voxel_worldgen::transitions::{
    DEBRIS_KEYS, TEMPLATE_EDGE, TEMPLATE_VOLUME, TransitionAxis, TransitionRole,
    TransitionTemplate, VARIANTS_PER_PAIR, choose, choose_surface, palette_at, top_at,
    top_for_block, transition_for, variant_for,
};
use voxel_worldgen::weights::{Axes, BiomeWeights};

fn p(percent: i32) -> i32 {
    percent * ONE / 100
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

fn biome_index(key: &str) -> usize {
    catalog()
        .biomes
        .iter()
        .position(|biome| biome.key == key)
        .unwrap_or_else(|| panic!("no biome {key}"))
}

/// The M2.5-10 desert/savanna transect: only humidity moves, from 5% to 50%
/// over 600 m, one centre gap.
fn transect_axes(step: i32) -> Axes {
    Axes {
        continentalness: p(60),
        temperature: p(82),
        humidity: p(5) + p(45) * step / 600,
        erosion: p(48),
        depth: 0,
    }
}

#[test]
fn transitions_dither_follows_the_weights() {
    let weights = BiomeWeights::at(&transect_axes(300));
    let samples = 20_000usize;
    let mut counts = [0usize; 18];
    for index in 0..samples {
        let roll = hash::unit(hash::hash2(7, index as i64, 0));
        counts[choose(&weights, roll)] += 1;
    }
    assert_eq!(counts.iter().sum::<usize>(), samples);

    for (index, count) in counts.iter().enumerate() {
        let probability = weights.weight(index) as f64 / ONE as f64;
        let expected = probability * samples as f64;
        let sigma = (expected * (1.0 - probability)).sqrt();
        assert!(
            (*count as f64 - expected).abs() <= 5.0 * sigma + 20.0,
            "{}: {count} picks, expected {expected:.1}",
            catalog().biomes[index].key
        );
    }

    // The two main materials carry most of the mass; a hard argmax would put
    // all of it on one.
    assert!(weights.weight(biome_index("voxel:desert")) > ONE / 10);
    assert!(weights.weight(biome_index("voxel:savanna")) > ONE / 10);
}

#[test]
fn transitions_surface_dither_never_returns_underground() {
    let rolls = (0..ONE).step_by(1_009);
    for c in [-ONE, 0, ONE] {
        for temperature in [0u8, 128, 255] {
            for humidity in [0u8, 128, 255] {
                for erosion in [0, ONE / 2, ONE] {
                    let axes = Axes::surface(c, temperature, humidity, erosion);
                    let weights = BiomeWeights::at(&axes);
                    for roll in rolls.clone() {
                        let index = choose_surface(&weights, roll);
                        assert!(
                            matches!(
                                catalog().biomes[index].class,
                                voxel_worldgen::biomes::BiomeClass::Land
                                    | voxel_worldgen::biomes::BiomeClass::Ocean
                            ),
                            "chose {} at {axes:?}",
                            catalog().biomes[index].key
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn transitions_seam_is_mixed_not_a_line() {
    let desert = biome_index("voxel:desert");
    let savanna = biome_index("voxel:savanna");
    let desert_top = catalog().biomes[desert].surface.top.as_str();
    let savanna_top = catalog().biomes[savanna].surface.top.as_str();

    let crossover = (0..=600)
        .find(|&step| {
            let weights = BiomeWeights::at(&transect_axes(step));
            weights.weight(savanna) > weights.weight(desert)
        })
        .expect("the transect crosses from desert to savanna");

    let start = crossover.saturating_sub(100);
    let end = (crossover + 100).min(600);
    let tops: Vec<&str> = (start..=end)
        .map(|step| {
            let weights = BiomeWeights::at(&transect_axes(step));
            top_for_block(&weights, 11, step as i64, 0)
        })
        .collect();

    assert!(
        tops.contains(&desert_top),
        "the mixed seam has no {desert_top:?}"
    );
    assert!(
        tops.contains(&savanna_top),
        "the mixed seam has no {savanna_top:?}"
    );
    assert!(
        tops[..(crossover - start) as usize].contains(&savanna_top),
        "savanna material never appears before the crossover"
    );
    assert!(
        tops[(crossover - start) as usize..].contains(&desert_top),
        "desert material never appears after the crossover"
    );

    let switches = tops
        .windows(2)
        .filter(|pair| {
            (pair[0] == desert_top && pair[1] == savanna_top)
                || (pair[0] == savanna_top && pair[1] == desert_top)
        })
        .count();
    assert!(
        switches >= 4,
        "the seam switches only {switches} times; it is a line, not a mix"
    );
}

#[test]
fn transitions_have_at_most_four_variants_per_pair() {
    assert_eq!(VARIANTS_PER_PAIR, 4);
    let a = "voxel:grass_block";
    let b = "voxel:dirt";
    let mut patterns = BTreeSet::new();
    for variant in 0..VARIANTS_PER_PAIR {
        let template = TransitionTemplate::new(a, b, variant);
        assert_eq!(template.variant, variant);
        let signature: Vec<u8> = (0..TEMPLATE_EDGE)
            .flat_map(|x| {
                (0..TEMPLATE_EDGE).flat_map(move |y| {
                    (0..TEMPLATE_EDGE).map(move |z| template.role(x, y, z) as u8)
                })
            })
            .collect();
        patterns.insert(signature);
    }
    assert_eq!(patterns.len(), VARIANTS_PER_PAIR as usize);

    // A larger variant number wraps instead of creating a fifth pattern.
    assert_eq!(TransitionTemplate::new(a, b, 4).variant, 0);
    assert_eq!(variant_for(u64::MAX), 3);
}

#[test]
fn transitions_use_only_parent_materials_and_debris() {
    let registry = registry();
    let mut palette_keys: BTreeSet<&str> = BTreeSet::new();
    for biome in &catalog().biomes {
        palette_keys.extend(biome.surface.keys());
    }

    for key in &palette_keys {
        let id = registry
            .id_of(key)
            .unwrap_or_else(|| panic!("palette key {key:?} is not in the manifest"));
        assert!(
            registry.face_materials(id).is_some(),
            "palette key {key:?} has no atlas materials"
        );
    }
    for key in DEBRIS_KEYS {
        let id = registry
            .id_of(key)
            .unwrap_or_else(|| panic!("debris key {key:?} is not in the manifest"));
        assert!(
            registry.face_materials(id).is_some(),
            "debris key {key:?} has no atlas materials"
        );
    }

    // Every unordered pair, including a pair that repeats a key.
    let keys: Vec<&str> = palette_keys.iter().copied().collect();
    for (index, a) in keys.iter().enumerate() {
        for b in &keys[index..] {
            let template = TransitionTemplate::new(a, b, 0);
            assert_ne!(template.debris, *a);
            assert_ne!(template.debris, *b);
            for key in template.keys() {
                assert!(
                    registry.id_of(key).is_some(),
                    "{a:?}/{b:?} uses unknown key {key:?}"
                );
            }
        }
    }
}

#[test]
fn transitions_every_variant_emits_all_three_roles() {
    let registry = registry();
    // A soil pair and a contrasting pair cover the normal case and a pair that
    // already has sand as one parent.
    for (a, b) in [
        ("voxel:grass_block", "voxel:dirt"),
        ("voxel:red_sand", "voxel:savanna_grass_block"),
        ("voxel:gravel", "voxel:dirt"),
    ] {
        for variant in 0..VARIANTS_PER_PAIR {
            for axis in [TransitionAxis::X, TransitionAxis::Y, TransitionAxis::Z] {
                let template = TransitionTemplate::new(a, b, variant).with_axis(axis);
                let counts = template.counts();
                assert_eq!(counts.iter().sum::<u32>(), TEMPLATE_VOLUME as u32);
                for role in [TransitionRole::A, TransitionRole::B, TransitionRole::Debris] {
                    assert!(
                        counts[role as usize] > 0,
                        "{a}/{b} variant {variant} axis {axis:?} has no {role:?}"
                    );
                }

                let mut materials = BTreeSet::new();
                for x in 0..TEMPLATE_EDGE {
                    for y in 0..TEMPLATE_EDGE {
                        for z in 0..TEMPLATE_EDGE {
                            materials.insert(template.material(x, y, z));
                        }
                    }
                }
                assert_eq!(
                    materials,
                    BTreeSet::from([a, b, template.debris]),
                    "{a}/{b} variant {variant} axis {axis:?}"
                );
                for key in materials {
                    assert!(
                        registry.id_of(key).is_some(),
                        "template emits unknown key {key:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn transitions_surface_columns_with_a_boundary_get_a_template() {
    let registry = registry();
    let samples = [
        // Soil.
        SurfaceSample::new(SEA_LEVEL + (500 << 16), 0, false, 3),
        // Snow cap: top differs from filler, but the cap is one block.
        SurfaceSample::new(SEA_LEVEL + SNOW_LINE, 0, false, 5),
    ];
    for biome in &catalog().biomes {
        for sample in &samples {
            let column = column(biome, *sample);
            match transition_for(&column, 0x1234) {
                Some(template) => {
                    assert_eq!(template.a, column.top, "{}", biome.key);
                    assert_eq!(template.b, column.filler, "{}", biome.key);
                    assert_eq!(template.axis, TransitionAxis::Y, "{}", biome.key);
                    assert!(template.variant < VARIANTS_PER_PAIR, "{}", biome.key);
                    for key in template.keys() {
                        assert!(
                            registry.id_of(key).is_some(),
                            "{} uses unknown key {key:?}",
                            biome.key
                        );
                    }
                }
                None => {
                    assert!(
                        column.filler_depth() == 0 || column.top == column.filler,
                        "{} lost a real boundary",
                        biome.key
                    );
                }
            }
        }
    }
}

#[test]
fn transitions_palette_choice_is_deterministic() {
    let weights = BiomeWeights::at(&transect_axes(300));
    let first = palette_at(&weights, 0xDEAD_BEEF);
    let second = palette_at(&weights, 0xDEAD_BEEF);
    assert_eq!(first, second);
    assert_eq!(top_at(&weights, 0xDEAD_BEEF), first.top.as_str());
}
