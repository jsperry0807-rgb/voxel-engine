//! M2.5-10: biome weights.
//!
//! The task's three acceptance points: weights sum to one, the blend band is
//! 300–500 m at the design's edge gradient, and the weight map has no hard
//! argmax switches. The first is exact; the second is measured on a transect
//! where the axes move one centre gap (0.45 of humidity) over 600 m, which is
//! the "weights vary over 300–500 m" assumption in worldgen-design §5.2; the
//! third is checked wherever the dominant biome changes.

use voxel_worldgen::biomes::{BiomeClass, catalog};
use voxel_worldgen::fixed::ONE;
use voxel_worldgen::weights::{Axes, BIOME_COUNT, BiomeWeights};

fn p(percent: i32) -> i32 {
    percent * ONE / 100
}

fn desert() -> usize {
    biome("voxel:desert")
}

fn biome(key: &str) -> usize {
    catalog()
        .biomes
        .iter()
        .position(|biome| biome.key == key)
        .unwrap_or_else(|| panic!("no biome {key}"))
}

/// The desert/savanna transect: only humidity moves.
fn transect(step: i32) -> Axes {
    Axes {
        continentalness: p(60),
        temperature: p(82),
        humidity: p(5) + p(45) * step / 600,
        erosion: p(48),
        depth: 0,
    }
}

#[test]
fn weights_sum_to_one_everywhere() {
    for c in -2..=2 {
        for t in [0u8, 64, 128, 200, 255] {
            for h in [0u8, 40, 128, 200, 255] {
                for erosion in [0i32, ONE / 3, ONE / 2, ONE] {
                    let axes = Axes::surface(c * ONE / 2, t, h, erosion);
                    let weights = BiomeWeights::at(&axes);
                    let sum: i32 = weights.weights().iter().sum();
                    assert_eq!(sum, ONE, "weights must sum to ONE for {axes:?}");
                    for weight in weights.weights() {
                        assert!(*weight >= 0, "negative weight");
                    }
                    // At least one biome carries a meaningful share.
                    assert!(
                        weights
                            .weights()
                            .iter()
                            .any(|weight| *weight >= ONE / BIOME_COUNT as i32),
                        "no dominant biome for {axes:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn weights_prefer_their_own_centre() {
    for (index, biome) in catalog().biomes.iter().enumerate() {
        // Depth sentinels are only reachable with a depth term; for them use
        // their own depth centre.
        let centre = biome.centre_q16();
        let axes = Axes {
            continentalness: centre[0],
            temperature: centre[1],
            humidity: centre[2],
            erosion: centre[3],
            depth: centre[4],
        };
        let weights = BiomeWeights::at(&axes);
        assert_eq!(
            weights.dominant(),
            index,
            "{} is not dominant at its own centre",
            biome.key
        );
    }
}

#[test]
fn weights_transition_is_300_to_500_metres() {
    // Humidity moves 0.05 -> 0.50 over 600 m; the desert/savanna pair is the
    // nearest pair here, so the 0.9 -> 0.1 band is the biome edge.
    let mut start = None;
    let mut end = None;
    for step in 0..=600 {
        let weight = BiomeWeights::at(&transect(step)).weight(desert()) as f64 / ONE as f64;
        if start.is_none() && weight < 0.9 {
            start = Some(step);
        }
        if start.is_some() && weight < 0.1 {
            end = Some(step);
            break;
        }
    }
    let width =
        end.expect("desert must fall below 0.1") - start.expect("desert must start above 0.9");
    assert!(
        (300..=500).contains(&width),
        "transition width {width} m is outside the design's 300-500 m band"
    );
}

#[test]
fn weights_have_no_hard_argmax_switch() {
    // Wherever the dominant biome changes, both sides are genuinely mixed.
    let mut previous = BiomeWeights::at(&transect(0));
    for step in 1..=600 {
        let current = BiomeWeights::at(&transect(step));
        if current.dominant() != previous.dominant() {
            let before = previous.weights()[previous.dominant()];
            let after = current.weights()[current.dominant()];
            assert!(
                (ONE / 4..=3 * ONE / 4).contains(&before),
                "dominant weight jumps from {before} at step {step}"
            );
            assert!(
                (ONE / 4..=3 * ONE / 4).contains(&after),
                "dominant weight jumps to {after} at step {step}"
            );
        }
        previous = current;
    }
}

#[test]
fn weights_map_is_continuous() {
    // A 4 m step at the design's edge gradient moves an axis by about 0.0025;
    // in that step no weight may move by more than a few percent, so the map
    // has no single-sample seams for M2.5-13 to dither over.
    const STEPS: i32 = 240;
    for t_step in 0..=STEPS {
        // Each row is a fresh scan; the row ends are not adjacent samples.
        let mut previous: Option<[i32; BIOME_COUNT]> = None;
        for h_step in 0..=STEPS {
            let axes = Axes {
                continentalness: p(60),
                temperature: p(20) + p(60) * t_step / STEPS,
                humidity: p(10) + p(70) * h_step / STEPS,
                erosion: p(40),
                depth: 0,
            };
            let weights = *BiomeWeights::at(&axes).weights();
            if let Some(previous) = previous {
                for index in 0..BIOME_COUNT {
                    let delta = (weights[index] - previous[index]).abs();
                    assert!(
                        delta < ONE / 20,
                        "weight {index} moved {delta} in one 4 m step"
                    );
                }
            }
            previous = Some(weights);
        }
    }
}

#[test]
fn weights_blend_shape_parameters() {
    let axes = Axes {
        continentalness: p(62),
        temperature: p(52),
        humidity: p(57),
        erosion: p(40),
        depth: 0,
    };
    let weights = BiomeWeights::at(&axes);

    let constant = [3 * ONE; BIOME_COUNT];
    assert_eq!(weights.blend(&constant), 3 * ONE);

    let mut ramp = [0i32; BIOME_COUNT];
    for (index, value) in ramp.iter_mut().enumerate() {
        *value = index as i32 * ONE;
    }
    // `blend` returns the weighted parameter in Q16.16: the weights sum to ONE,
    // so the ramp's weighted mean is `sum(w_i * i)` in Q16.16.
    let expected: i64 = weights
        .weights()
        .iter()
        .enumerate()
        .map(|(index, weight)| *weight as i64 * index as i64)
        .sum();
    assert!((weights.blend(&ramp) - expected as i32).abs() <= 1);
}

#[test]
fn weights_underground_needs_depth() {
    let fungal = biome("voxel:fungal_deep");
    let centre = catalog().biomes[fungal].centre_q16();
    let surface = BiomeWeights::at(&Axes {
        continentalness: centre[0],
        temperature: centre[1],
        humidity: centre[2],
        erosion: centre[3],
        depth: 0,
    });
    assert!(
        !matches!(
            catalog().biomes[surface.dominant()].class,
            BiomeClass::Underground | BiomeClass::Rare
        ),
        "underground biome {} reached at the surface",
        catalog().biomes[surface.dominant()].key
    );

    let deep = BiomeWeights::at(&Axes {
        continentalness: centre[0],
        temperature: centre[1],
        humidity: centre[2],
        erosion: centre[3],
        depth: ONE,
    });
    assert_eq!(
        deep.dominant(),
        fungal,
        "expected fungal_deep at depth, got {}",
        catalog().biomes[deep.dominant()].key
    );
}

#[test]
fn weights_surface_axes_are_remapped() {
    let cold = Axes::surface(-ONE, 0, 0, 0);
    assert_eq!(cold.continentalness, 0);
    assert_eq!(cold.temperature, 0);
    assert_eq!(cold.humidity, 0);
    assert_eq!(cold.depth, 0);

    let warm = Axes::surface(ONE, 255, 255, ONE);
    assert_eq!(warm.continentalness, ONE);
    assert_eq!(warm.temperature, ONE);
    assert_eq!(warm.humidity, ONE);
    assert_eq!(warm.erosion, ONE);
}

#[test]
fn weights_are_deterministic() {
    let axes = Axes {
        continentalness: p(63),
        temperature: p(48),
        humidity: p(66),
        erosion: p(37),
        depth: p(10),
    };
    let first = BiomeWeights::at(&axes);
    let second = BiomeWeights::at(&axes);
    assert_eq!(first.weights(), second.weights());
    assert_eq!(first.dominant(), second.dominant());
}

#[test]
fn weights_ocean_biomes_own_the_sea() {
    // Continentalness 0.15 (well below the coast) is deepest ocean.
    let weights = BiomeWeights::at(&Axes {
        continentalness: p(12),
        temperature: p(50),
        humidity: p(50),
        erosion: p(40),
        depth: 0,
    });
    assert_eq!(catalog().biomes[weights.dominant()].key, "voxel:trench");

    let shelf = BiomeWeights::at(&Axes {
        continentalness: p(45),
        temperature: p(50),
        humidity: p(50),
        erosion: p(40),
        depth: 0,
    });
    assert_eq!(catalog().biomes[shelf.dominant()].key, "voxel:shelf");
}
