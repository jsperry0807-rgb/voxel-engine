//! M2.5-14: the five cave types, balanced density and the water seal.
//!
//! The scan box is a 128 × 128 m patch with a 48 m surface. That is large
//! enough to contain several cheese chambers and shafts, so "all five types"
//! and the density bounds are checked per seed rather than relying on one lucky
//! sample. The unit tests in `src/caves.rs` pin the surface, floor, depth and
//! determinism rules; this suite checks the content the design promises.

use voxel_worldgen::caves::{
    CaveContext, CaveField, CaveFill, CaveShape, GALLERY_HEIGHT_M, WATER_SEAL_M,
};

fn metres(value: i32) -> i64 {
    (value as i64) << 16
}

struct Stats {
    shapes: [usize; 4],
    fills: [usize; 3],
    carved: usize,
    underground: usize,
}

impl Stats {
    fn shape_share(&self, shape: CaveShape) -> usize {
        self.shapes[shape as usize] * 100 / self.carved
    }

    fn fill_share(&self, fill: CaveFill) -> usize {
        self.fills[fill as usize] * 100 / self.carved
    }

    fn carve_percent(&self) -> usize {
        self.carved * 100 / self.underground
    }
}

fn scan(seed: u64, surface: i32, underwater: bool) -> Stats {
    let field = CaveField::new(seed);
    let context = CaveContext::new(
        metres(surface),
        underwater,
        metres(surface / 2),
        metres(surface / 6),
    );
    let mut stats = Stats {
        shapes: [0usize; 4],
        fills: [0usize; 3],
        carved: 0,
        underground: (surface as usize + 1) * 128 * 128,
    };
    for x in 0..128 {
        for z in 0..128 {
            for y in 0..=surface {
                if let Some(voxel) = field.sample(metres(x), metres(y), metres(z), &context) {
                    stats.carved += 1;
                    stats.shapes[voxel.shape as usize] += 1;
                    stats.fills[voxel.fill as usize] += 1;
                }
            }
        }
    }
    stats
}

#[test]
fn caves_have_all_five_types() {
    for seed in 1..=4 {
        let stats = scan(seed, 48, false);
        for shape in CaveShape::ALL {
            assert!(
                stats.shapes[shape as usize] > 0,
                "seed {seed}: {shape:?} is missing"
            );
        }
        assert!(
            stats.fills[CaveFill::Water as usize] > 0,
            "seed {seed}: no water cave"
        );
        assert!(
            stats.fills[CaveFill::Lava as usize] > 0,
            "seed {seed}: no lava cave"
        );
    }
}

#[test]
fn caves_density_is_balanced() {
    for seed in 1..=4 {
        let stats = scan(seed, 48, false);
        assert!(
            (5..=15).contains(&stats.carve_percent()),
            "seed {seed}: {}% of the underground is carved",
            stats.carve_percent()
        );
        for shape in CaveShape::ALL {
            let share = stats.shape_share(shape);
            assert!(
                (10..=45).contains(&share),
                "seed {seed}: {shape:?} is {share}% of the carved volume"
            );
        }
        assert!(
            stats.fill_share(CaveFill::Water) >= 5,
            "seed {seed}: water is only {}%",
            stats.fill_share(CaveFill::Water)
        );
        assert!(
            stats.fill_share(CaveFill::Lava) >= 5,
            "seed {seed}: lava is only {}%",
            stats.fill_share(CaveFill::Lava)
        );
        assert!(
            stats.fills[CaveFill::Air as usize] > 0,
            "seed {seed}: no air-filled cave"
        );
    }
}

#[test]
fn caves_keep_the_two_metre_water_seal() {
    let field = CaveField::new(9);
    let surface = 48;
    let land = CaveContext::land(metres(surface), metres(surface / 2), metres(surface / 6));
    let wet = CaveContext::submerged(metres(surface), metres(surface / 2), metres(surface / 6));
    let mut sealed_band = 0usize;
    let mut land_in_band = 0usize;
    let mut wet_below = 0usize;
    let mut land_below = 0usize;
    for x in 0..64 {
        for z in 0..64 {
            for y in 0..=surface {
                let land_carved = field
                    .sample(metres(x), metres(y), metres(z), &land)
                    .is_some();
                let wet_carved = field
                    .sample(metres(x), metres(y), metres(z), &wet)
                    .is_some();
                if y > surface - WATER_SEAL_M {
                    sealed_band += 1;
                    assert!(!wet_carved, "carved into the seal at ({x}, {y}, {z})");
                    if land_carved {
                        land_in_band += 1;
                    }
                } else {
                    if wet_carved {
                        wet_below += 1;
                    }
                    if land_carved {
                        land_below += 1;
                    }
                }
            }
        }
    }
    assert!(sealed_band > 0);
    assert!(
        land_in_band > 0,
        "the seal test needs a cave in the sealed band on land"
    );
    assert!(wet_below > 0, "the seal must not stop all carving");
    assert_eq!(
        wet_below, land_below,
        "the seal must remove exactly the top {WATER_SEAL_M} m, not more"
    );
}

#[test]
fn caves_galleries_are_three_blocks_tall() {
    let field = CaveField::new(1);
    let surface = 48;
    let context = CaveContext::land(metres(surface), metres(surface / 2), metres(surface / 6));
    let mut gallery_voxels = 0usize;
    let mut runs = 0usize;
    for x in 0..64 {
        for z in 0..64 {
            let mut run = 0usize;
            for y in 0..=surface {
                let is_gallery = matches!(
                    field.sample(metres(x), metres(y), metres(z), &context),
                    Some(voxel) if voxel.shape == CaveShape::Gallery
                );
                if is_gallery {
                    gallery_voxels += 1;
                    run += 1;
                    assert!(
                        run <= GALLERY_HEIGHT_M as usize,
                        "gallery at ({x}, {y}, {z}) is taller than {GALLERY_HEIGHT_M} m"
                    );
                } else if run > 0 {
                    runs += 1;
                    run = 0;
                }
            }
            if run > 0 {
                runs += 1;
            }
        }
    }
    assert!(gallery_voxels > 0, "no gallery to measure");
    assert!(runs > 0, "no gallery runs to measure");
}
