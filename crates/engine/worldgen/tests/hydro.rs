//! M2.5-06: priority-flood hydrology.
//!
//! The task asks for three properties: lakes sit at their spill level, the flow
//! graph is acyclic, and a hand-checked fixture agrees. The fixtures below are
//! small enough to verify by eye; the random grids then check the invariants
//! the fixtures cannot cover, and a brute-force minimax path search
//! independently reproduces the filled surface.

use voxel_worldgen::hydro::{FlowDir, Hydrology};

/// Build a row-major heightfield from rows of equal length.
fn grid(rows: &[&[u16]]) -> (Vec<u16>, usize, usize) {
    let height = rows.len();
    let width = rows[0].len();
    let mut heights = Vec::with_capacity(width * height);
    for row in rows {
        assert_eq!(row.len(), width, "rows must be equal length");
        heights.extend_from_slice(row);
    }
    (heights, width, height)
}

/// A 5 × 5 bowl with a single low spill on the east border.
///
/// ```text
/// 5 5 5 5 5
/// 5 1 1 1 5
/// 5 1 1 1 3   <- spill: east border, height 3
/// 5 1 1 1 5
/// 5 5 5 5 5
/// ```
fn bowl() -> Hydrology {
    let (heights, width, height) = grid(&[
        &[5, 5, 5, 5, 5],
        &[5, 1, 1, 1, 5],
        &[5, 1, 1, 1, 3],
        &[5, 1, 1, 1, 5],
        &[5, 5, 5, 5, 5],
    ]);
    Hydrology::compute(heights, width, height, 0)
}

#[test]
fn hydro_bowl_fills_to_the_spill_level() {
    let hydro = bowl();

    // The nine interior cells are all below the spill and all reach 3.
    for z in 1..4 {
        for x in 1..4 {
            let cell = hydro.cell(x, z);
            assert_eq!(cell.height, 1, "fixture interior height at ({x}, {z})");
            assert_eq!(
                cell.filled, 3,
                "interior ({x}, {z}) is not at the spill level"
            );
            assert!(cell.is_lake());
            assert_eq!(cell.lake_depth(), 2);
        }
    }

    // The spill cell itself is an outlet and is not raised.
    let spill = hydro.cell(4, 2);
    assert_eq!(spill.height, 3);
    assert_eq!(spill.filled, 3);
    assert_eq!(spill.flow, FlowDir::Outlet);

    // Border cells keep their own elevation.
    for z in 0..5 {
        for x in 0..5 {
            if x == 0 || z == 0 || x == 4 || z == 4 {
                let cell = hydro.cell(x, z);
                assert_eq!(cell.filled, cell.height, "border ({x}, {z}) was filled");
                assert_eq!(cell.flow, FlowDir::Outlet);
            }
        }
    }

    assert_eq!(hydro.lake_cells(), 9);
    assert_eq!(hydro.max_fill_depth(), 2);
}

#[test]
fn hydro_bowl_accumulates_at_the_spill() {
    let hydro = bowl();
    // Nine interior cells plus the spill cell itself drain through (4, 2).
    assert_eq!(hydro.cell(4, 2).accumulation, 10);

    // Every cell drains to exactly one outlet: outlet accumulation sums to the
    // cell count, because each cell is counted once on its own path.
    let total: u64 = (0..5)
        .flat_map(|z| (0..5).map(move |x| (x, z)))
        .filter(|&(x, z)| hydro.cell(x, z).flow.is_outlet())
        .map(|(x, z)| hydro.cell(x, z).accumulation as u64)
        .sum();
    assert_eq!(total, 25);
}

#[test]
fn hydro_slope_is_never_filled_and_flows_hand_checked() {
    // ```text
    // 9 9 9 1
    // 9 5 4 2
    // 9 9 9 3
    // ```
    // The low east border drains the two interior cells; nothing needs filling.
    let (heights, width, height) = grid(&[&[9, 9, 9, 1], &[9, 5, 4, 2], &[9, 9, 9, 3]]);
    let hydro = Hydrology::compute(heights, width, height, 0);

    assert_eq!(hydro.lake_cells(), 0);
    for index in 0..hydro.len() {
        assert_eq!(hydro.filled()[index], hydro.heights()[index]);
    }

    // The tree drains (1,1) -> (2,1) -> (3,0).
    assert_eq!(hydro.cell(1, 1).flow, FlowDir::E);
    assert_eq!(hydro.cell(2, 1).flow, FlowDir::NE);
    assert_eq!(hydro.cell(3, 0).flow, FlowDir::Outlet);

    assert_eq!(hydro.cell(2, 1).accumulation, 2);
    assert_eq!(hydro.cell(3, 0).accumulation, 3);
}

#[test]
fn hydro_flow_graph_is_acyclic_and_downhill() {
    let hydro = random_hydro(24, 24, 0x5EED, 4);
    let cells = hydro.len();

    for index in 0..cells {
        let cell_height = hydro.filled()[index];
        let mut current = index;
        let mut steps = 0;
        loop {
            let direction = hydro.flow()[current];
            if direction.is_outlet() {
                break;
            }
            let (dx, dz) = direction.delta();
            let x = (current % hydro.width()) as i32 + dx;
            let z = (current / hydro.width()) as i32 + dz;
            assert!(x >= 0 && z >= 0, "flow left the grid");
            assert!(
                (x as usize) < hydro.width() && (z as usize) < hydro.height(),
                "flow left the grid"
            );
            let parent = z as usize * hydro.width() + x as usize;
            assert!(
                hydro.filled()[parent] <= cell_height,
                "flow climbs from {cell_height} to {}",
                hydro.filled()[parent]
            );
            assert!(
                hydro.accumulation()[parent] > hydro.accumulation()[current],
                "accumulation must grow downstream"
            );
            current = parent;
            steps += 1;
            assert!(steps <= cells, "flow graph contains a cycle");
        }
    }
}

#[test]
fn hydro_filled_surface_matches_a_brute_force_minimax() {
    // Independent implementation: the filled elevation is the minimum, over
    // all paths to an outlet, of the maximum elevation on the path.
    for (width, height, seed) in [(6usize, 6usize, 1u64), (7, 5, 2), (5, 9, 3)] {
        let hydro = random_hydro(width, height, seed, 3);
        let expected = brute_force_minimax(hydro.heights(), width, height, hydro.sea_level());
        assert_eq!(
            hydro.filled(),
            &expected[..],
            "minimax disagreement on {width}×{height}, seed {seed}"
        );
    }
}

#[test]
fn hydro_sea_level_cells_are_outlets_and_never_filled() {
    let (heights, width, height) = grid(&[&[3, 3, 3], &[3, 1, 3], &[3, 3, 0]]);
    let hydro = Hydrology::compute(heights, width, height, 1);

    for (x, z) in [(1, 1), (2, 2)] {
        let cell = hydro.cell(x, z);
        assert_eq!(cell.flow, FlowDir::Outlet, "({x}, {z}) is at sea level");
        assert_eq!(cell.filled, cell.height);
        assert!(!cell.is_lake());
    }
}

#[test]
fn hydro_matches_between_runs_and_sizes() {
    let first = random_hydro(20, 16, 0xC0FFEE, 2);
    let second = random_hydro(20, 16, 0xC0FFEE, 2);
    assert_eq!(first.filled(), second.filled());
    assert_eq!(first.flow(), second.flow());
    assert_eq!(first.accumulation(), second.accumulation());
    assert_eq!(first.lake_cells(), second.lake_cells());
    assert_eq!(first.max_accumulation(), second.max_accumulation());
}

#[test]
fn hydro_outlet_accumulation_always_sums_to_the_cell_count() {
    for seed in 0..8 {
        let hydro = random_hydro(12, 9, seed, 2);
        let total: u64 = (0..hydro.len())
            .filter(|&index| hydro.flow()[index].is_outlet())
            .map(|index| hydro.accumulation()[index] as u64)
            .sum();
        assert_eq!(
            total,
            hydro.len() as u64,
            "seed {seed}: outlet accumulation is not the cell count"
        );
    }
}

#[test]
fn hydro_accumulation_counts_every_upstream_cell() {
    // Local invariant: a cell's accumulation is one (itself) plus the
    // accumulation of every neighbour that points at it.
    let hydro = random_hydro(15, 11, 77, 3);
    let width = hydro.width();
    let height = hydro.height();
    for z in 0..height {
        for x in 0..width {
            let index = z * width + x;
            let mut sum = 1u64;
            for direction in FlowDir::ALL {
                let (dx, dz) = direction.delta();
                let (nx, nz) = (x as i32 - dx, z as i32 - dz);
                if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                    continue;
                }
                let neighbour = nz as usize * width + nx as usize;
                if hydro.flow()[neighbour] == direction {
                    sum += hydro.accumulation()[neighbour] as u64;
                }
            }
            assert_eq!(
                hydro.accumulation()[index] as u64,
                sum,
                "accumulation at ({x}, {z})"
            );
        }
    }
}

/// A deterministic pseudo-random heightfield: an LCG over `u16` heights.
fn random_hydro(width: usize, height: usize, seed: u64, sea_level: u16) -> Hydrology {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut heights = Vec::with_capacity(width * height);
    for _ in 0..width * height {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        heights.push(((state >> 33) % 40) as u16);
    }
    Hydrology::compute(heights, width, height, sea_level)
}

/// Minimax path cost to an outlet: the textbook definition of the filled
/// surface, computed without a heap.
fn brute_force_minimax(heights: &[u16], width: usize, height: usize, sea_level: u16) -> Vec<u16> {
    let cells = width * height;
    let mut cost = vec![u16::MAX; cells];
    for z in 0..height {
        for x in 0..width {
            let index = z * width + x;
            let border = x == 0 || z == 0 || x + 1 == width || z + 1 == height;
            if border || heights[index] <= sea_level {
                cost[index] = heights[index];
            }
        }
    }
    loop {
        let mut changed = false;
        for z in 0..height {
            for x in 0..width {
                let index = z * width + x;
                let mut best = cost[index];
                for direction in FlowDir::ALL {
                    let (dx, dz) = direction.delta();
                    let (nx, nz) = (x as i32 + dx, z as i32 + dz);
                    if nx < 0 || nz < 0 || nx >= width as i32 || nz >= height as i32 {
                        continue;
                    }
                    let neighbour = nz as usize * width + nx as usize;
                    if cost[neighbour] == u16::MAX {
                        continue;
                    }
                    let candidate = heights[index].max(cost[neighbour]);
                    if candidate < best {
                        best = candidate;
                    }
                }
                if best < cost[index] {
                    cost[index] = best;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    cost
}
