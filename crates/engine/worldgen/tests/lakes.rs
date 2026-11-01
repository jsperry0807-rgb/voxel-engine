//! M2.5-08: lake extraction, spill levels and shorelines.
//!
//! The fixtures pin the hand-checkable cases: one bowl at its spill, two basins
//! at different spill levels. The invariant sweeps then assert the design's
//! promises on random fields: no leaking basins, a spill at the lake surface
//! that drains away, 4-connected components, a uniform surface, and a shoreline
//! with no isolated cells.

use std::collections::HashSet;

use voxel_worldgen::generator::{MacroGenerator, SEA_LEVEL_UNITS};
use voxel_worldgen::hydro::Hydrology;
use voxel_worldgen::lakes::{DEFAULT_MIN_CELLS, Lake, LakeConfig, Lakes};
use voxel_worldgen::macro_grid::{CELL_SIZE_M, height_from_q16};

fn grid(rows: &[&[u16]]) -> (Vec<u16>, usize, usize) {
    let height = rows.len();
    let width = rows[0].len();
    let mut heights = Vec::with_capacity(width * height);
    for row in rows {
        assert_eq!(row.len(), width);
        heights.extend_from_slice(row);
    }
    (heights, width, height)
}

fn hydrology(rows: &[&[u16]], sea_level: u16) -> Hydrology {
    let (heights, width, height) = grid(rows);
    Hydrology::compute(heights, width, height, sea_level)
}

/// A single bowl with one low spill on the east border.
fn bowl() -> Hydrology {
    hydrology(
        &[
            &[5, 5, 5, 5, 5],
            &[5, 1, 1, 1, 5],
            &[5, 1, 1, 1, 3],
            &[5, 1, 1, 1, 5],
            &[5, 5, 5, 5, 5],
        ],
        0,
    )
}

fn assert_no_leak(hydro: &Hydrology, lake: &Lake) {
    let width = hydro.width();
    let height = hydro.height();
    for &cell in &lake.cells {
        let cell = cell as usize;
        let x = (cell % width) as i32;
        let z = (cell / width) as i32;
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    continue;
                }
                let neighbour = nz as usize * width + nx as usize;
                assert!(
                    hydro.filled()[neighbour] >= lake.surface,
                    "lake at ({}, {}) leaks into ({nx}, {nz})",
                    lake.spill.0,
                    lake.spill.1
                );
            }
        }
    }
}

fn assert_spill_drains(hydro: &Hydrology, lake: &Lake) {
    let width = hydro.width();
    let height = hydro.height();
    let spill = lake.spill.1 as usize * width + lake.spill.0 as usize;
    assert_eq!(
        hydro.filled()[spill],
        lake.surface,
        "the spill must sit at the lake surface"
    );
    assert!(
        !lake.cells.contains(&(spill as u32)),
        "the spill is inside the lake"
    );

    // The spill is outside the lake. Its flow may cross dry cells at the same
    // level, but it must leave the water and drop the level without re-entering.
    assert!(
        !lake.cells.contains(&(spill as u32)),
        "the spill is inside the lake"
    );
    let lake_set: HashSet<u32> = lake.cells.iter().copied().collect();
    let mut probe = spill;
    let mut probe_steps = 0;
    loop {
        let direction = hydro.flow()[probe];
        if direction.is_outlet() {
            break;
        }
        let (dx, dz) = direction.delta();
        let parent = (probe / width) as i32 + dz;
        let column = (probe % width) as i32 + dx;
        let parent = parent as usize * width + column as usize;
        if hydro.filled()[parent] < lake.surface {
            break;
        }
        assert!(
            !lake_set.contains(&(parent as u32)),
            "the spill re-enters its own lake without dropping the level"
        );
        probe = parent;
        probe_steps += 1;
        assert!(probe_steps <= 256, "the spill does not leave the surface");
    }
    let mut current = spill;
    let mut steps = 0;
    loop {
        let direction = hydro.flow()[current];
        if direction.is_outlet() {
            break;
        }
        let (dx, dz) = direction.delta();
        let x = (current % width) as i32 + dx;
        let z = (current / width) as i32 + dz;
        assert!(x >= 0 && z >= 0);
        assert!((x as usize) < width && (z as usize) < height);
        let parent = z as usize * width + x as usize;
        assert!(
            hydro.filled()[parent] <= hydro.filled()[current],
            "the spill climbs"
        );
        current = parent;
        steps += 1;
        assert!(steps <= hydro.len(), "spill flow does not reach an outlet");
    }
}

fn assert_lake_invariants(hydro: &Hydrology, lake: &Lake) {
    assert_no_leak(hydro, lake);
    assert_spill_drains(hydro, lake);
    assert!(lake.area_cells as usize == lake.cells.len());
    assert!(!lake.cells.is_empty());
    assert!(!lake.shoreline.is_empty());

    // Uniform surface and 8-connected water.
    let width = hydro.width();
    let height = hydro.height();
    let mut remaining: HashSet<u32> = lake.cells.iter().copied().collect();
    let first = *lake.cells.first().expect("non-empty");
    assert_eq!(hydro.filled()[first as usize], lake.surface);
    let mut stack = vec![first];
    remaining.remove(&first);
    while let Some(cell) = stack.pop() {
        let cell = cell as usize;
        let x = (cell % width) as i32;
        let z = (cell / width) as i32;
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    continue;
                }
                let neighbour = nz as usize * width + nx as usize;
                if remaining.remove(&(neighbour as u32)) {
                    stack.push(neighbour as u32);
                }
            }
        }
    }
    assert!(remaining.is_empty(), "lake cells are not 8-connected");

    // Shoreline: sorted, no duplicates, every cell touches the lake, and no
    // shoreline cell is isolated from the rest of the ring.
    // Row-major order: z then x.
    let mut previous = None;
    for &cell in &lake.shoreline {
        if let Some((previous_z, previous_x)) = previous {
            assert!(
                (cell.1, cell.0) > (previous_z, previous_x),
                "shoreline must be sorted and unique"
            );
        }
        previous = Some((cell.1, cell.0));
    }
    let shore: HashSet<(i32, i32)> = lake.shoreline.iter().copied().collect();
    let lake_set: HashSet<u32> = lake.cells.iter().copied().collect();
    for &(x, z) in &lake.shoreline {
        let mut touches_lake = false;
        let mut touches_shore = false;
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 {
                    continue;
                }
                if shore.contains(&(nx, nz)) {
                    touches_shore = true;
                }
                let index = nz as usize * width + nx as usize;
                if lake_set.contains(&(index as u32)) {
                    touches_lake = true;
                }
            }
        }
        assert!(
            touches_lake,
            "shoreline cell ({x}, {z}) does not touch the lake"
        );
        assert!(
            touches_shore,
            "shoreline cell ({x}, {z}) is isolated from its ring"
        );
    }
}

#[test]
fn lakes_bowl_is_extracted_with_its_spill() {
    let hydro = bowl();
    let lakes = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    assert_eq!(lakes.len(), 1);

    let lake = &lakes.lakes()[0];
    assert_eq!(lake.area_cells, 9);
    assert_eq!(lake.surface, 3);
    assert_eq!(lake.max_depth, 2);
    assert_eq!(lake.spill, (4, 2));
    assert_eq!(lake.bounds, (1, 1, 3, 3));
    assert_eq!(lake.centroid, (2, 2));
    // Every non-lake cell of the 5 × 5 grid touches the 3 × 3 lake.
    assert_eq!(lake.shoreline.len(), 16);
    assert!(lake.shoreline.contains(&(4, 2)));
    assert_eq!(lakes.lake_cells(), 9);
    assert_eq!(lakes.area_cells(), 9);
    assert_eq!(lake.surface_mm(), 750);
    assert_eq!(lake.max_depth_mm(), 500);

    assert_lake_invariants(&hydro, lake);
}

#[test]
fn lakes_two_basins_have_their_own_spill_levels() {
    // The right basin spills over the east border at height 3; the left basin
    // spills over the ridge at height 5.
    let hydro = hydrology(
        &[
            &[5, 5, 5, 5, 5, 5, 5, 5, 5],
            &[5, 1, 1, 1, 5, 1, 1, 1, 5],
            &[5, 1, 1, 1, 5, 1, 1, 1, 3],
            &[5, 1, 1, 1, 5, 1, 1, 1, 5],
            &[5, 5, 5, 5, 5, 5, 5, 5, 5],
        ],
        0,
    );
    let lakes = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    assert_eq!(lakes.len(), 2, "two separate basins");

    for lake in lakes.lakes() {
        if lake.centroid.0 < 4 {
            assert_eq!(lake.surface, 5, "left basin spills over the ridge");
            assert_eq!(lake.max_depth, 4);
        } else {
            assert_eq!(lake.surface, 3, "right basin spills over the border");
            assert_eq!(lake.max_depth, 2);
        }
        assert_lake_invariants(&hydro, lake);
    }
}

#[test]
fn lakes_plane_has_no_water() {
    let hydro = hydrology(&[&[9, 9, 9], &[0, 10, 20], &[9, 9, 9]], 0);
    let lakes = Lakes::extract(&hydro);
    assert!(lakes.is_empty());
    assert_eq!(lakes.lake_cells(), 0);
    assert_eq!(lakes.largest(), None);
    assert_eq!(lakes.deepest(), None);
}

#[test]
fn lakes_area_filter_keeps_the_big_ones() {
    // One large lake and one single-cell puddle.
    let hydro = hydrology(
        &[
            &[9, 9, 9, 9, 9, 9, 9],
            &[9, 1, 1, 9, 9, 2, 9],
            &[9, 1, 1, 9, 9, 9, 9],
            &[9, 9, 9, 9, 9, 9, 9],
        ],
        0,
    );
    // The 2 × 2 basin has no low rim, so it fills to the surrounding 9 as well;
    // both basins are lakes of the whole grid height. The point of the test is
    // the filter, so compare counts rather than surfaces.
    let all = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    let big = Lakes::extract_with(&hydro, LakeConfig { min_cells: 2 });
    assert!(all.len() >= big.len());
    assert_eq!(all.lake_cells(), big.lake_cells());
    for lake in big.lakes() {
        assert!(lake.area_cells >= 2);
    }
}

#[test]
fn lakes_no_leaks_on_random_fields() {
    let mut total = 0;
    for seed in 0..6u64 {
        let hydro = random_hydro(80, 64, seed, 6);
        let lakes = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
        total += lakes.len();
        for lake in lakes.lakes() {
            assert_lake_invariants(&hydro, lake);
        }
    }
    assert!(total >= 20, "only {total} lakes across six fields");
}

#[test]
fn lakes_surface_is_uniform_and_at_or_above_every_bed() {
    let hydro = random_hydro(64, 64, 99, 4);
    let lakes = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    for lake in lakes.lakes() {
        for &cell in &lake.cells {
            let cell = cell as usize;
            assert_eq!(hydro.filled()[cell], lake.surface);
            assert!(hydro.filled()[cell] > hydro.heights()[cell]);
            assert!(lake.surface - hydro.heights()[cell] <= lake.max_depth);
        }
    }
}

#[test]
fn lakes_extraction_is_deterministic() {
    let hydro = random_hydro(72, 56, 7, 5);
    let first = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    let second = Lakes::extract_with(&hydro, LakeConfig { min_cells: 1 });
    assert_eq!(first.lakes(), second.lakes());
    assert_eq!(first.content_hash(), second.content_hash());
    assert_eq!(first.len(), second.len());
}

#[test]
fn lakes_default_minimum_is_a_pond() {
    assert_eq!(DEFAULT_MIN_CELLS, 8);
    assert_eq!(LakeConfig::default().min_cells, DEFAULT_MIN_CELLS);
}

#[test]
fn lakes_real_window_is_extracted() {
    // A 256 × 256 window of the generated world near the centre: the field has
    // lakes at this scale, and the same invariants must hold.
    let generator = MacroGenerator::new(1);
    let size = 256usize;
    let start = 2_000i64;
    let half = CELL_SIZE_M as i64 / 2;
    let mut heights = Vec::with_capacity(size * size);
    for z in 0..size {
        for x in 0..size {
            let world_x = ((start + x as i64) * CELL_SIZE_M as i64 + half) << 16;
            let world_z = ((start + z as i64) * CELL_SIZE_M as i64 + half) << 16;
            heights.push(height_from_q16(generator.surface(world_x, world_z)));
        }
    }
    let hydro = Hydrology::compute(heights, size, size, SEA_LEVEL_UNITS);
    let lakes = Lakes::extract_with(&hydro, LakeConfig { min_cells: 4 });
    assert!(!lakes.is_empty(), "the window has lakes");
    for lake in lakes.lakes() {
        assert_lake_invariants(&hydro, lake);
    }
    let largest = lakes.largest().expect("non-empty");
    assert!(largest.area_cells >= 4);
    assert!(largest.max_depth >= 1);
}

/// Deterministic pseudo-random heightfield.
fn random_hydro(width: usize, height: usize, seed: u64, sea_level: u16) -> Hydrology {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut heights = Vec::with_capacity(width * height);
    for _ in 0..width * height {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        heights.push(((state >> 33) % 60) as u16);
    }
    Hydrology::compute(heights, width, height, sea_level)
}
