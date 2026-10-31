//! M2-15: incremental light equals a full recompute after every edit set.
//!
//! Two geometries are covered:
//!
//! - **production**: an 8³-cell grid (the 128³ chunk light grid; 64³ uses 4³).
//!   Edits land anywhere, so a removal escalates to a full relight. The test
//!   checks the result still equals a fresh full pass.
//! - **clipped window**: a 33³-cell grid with edits kept at least [`MAX_LEVEL`]
//!   cells from every edge. Here the dirty region is genuinely smaller than the
//!   chunk; the test asserts `full_relights() == 0` so the comparison cannot
//!   silently degrade into full-pass-versus-full-pass.

use voxel_mesh::light::{BlockLight, CellEdit, LightState, MAX_LEVEL, full_pass};

const SEED: u64 = 0x4D31_2F28_2026_1026u64;
const PRODUCTION_EDGE: usize = 8;
const WINDOW_EDGE: usize = 2 * MAX_LEVEL as usize + 3;
const SETS: usize = 1_000;
const WINDOW_SETS: usize = 100;

#[test]
fn light_incremental_matches_full_recompute() {
    let mut grid = Grid::new(PRODUCTION_EDGE);
    let mut state = LightState::new(
        PRODUCTION_EDGE,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );
    let mut random = SEED;

    for set in 0..SETS {
        let edits = next_edits(&mut random, &mut grid, 0, PRODUCTION_EDGE as i32);
        state.apply_edits(&edits);
        assert_matches_full(&state, &grid, PRODUCTION_EDGE, &format!("set {set}"));
    }
}

#[test]
fn clipped_window_matches_full_recompute() {
    let mut grid = Grid::new(WINDOW_EDGE);
    let mut state = LightState::new(
        WINDOW_EDGE,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );
    let mut random = SEED;
    let low = MAX_LEVEL as i32;
    let high = WINDOW_EDGE as i32 - MAX_LEVEL as i32;

    for set in 0..WINDOW_SETS {
        let edits = next_edits(&mut random, &mut grid, low, high);
        state.apply_edits(&edits);
        assert_eq!(
            state.full_relights(),
            0,
            "set {set}: an interior edit must stay on the dirty-region path"
        );
        // The padded volume is (EDGE + 2)³; a region recompute that ignores the
        // dirty window would exceed the chunk volume and fail here. Increase-only
        // batches report their seeds (a handful), so only the upper bound is
        // meaningful for every batch.
        assert!(
            state.last_touched_cells() <= WINDOW_EDGE.pow(3),
            "set {set}: touched {} cells, at most {} expected",
            state.last_touched_cells(),
            WINDOW_EDGE.pow(3)
        );
        assert_matches_full(&state, &grid, WINDOW_EDGE, &format!("window set {set}"));
    }
}

fn assert_matches_full(state: &LightState, grid: &Grid, edge: usize, label: &str) {
    let full = full_pass(
        edge,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );
    let last = edge as i32;
    for z in -1..=last {
        for y in -1..=last {
            for x in -1..=last {
                let got = state.with_apron(x, y, z);
                let expected = full.with_apron(x, y, z);
                assert_eq!(
                    got.sky_level(),
                    expected.sky_level(),
                    "{label}: sky at ({x}, {y}, {z})"
                );
                assert_eq!(
                    got.block_level(),
                    expected.block_level(),
                    "{label}: block at ({x}, {y}, {z})"
                );
            }
        }
    }
}

struct Grid {
    padded: usize,
    blocks: Vec<BlockLight>,
    sky: Vec<bool>,
}

impl Grid {
    fn new(edge: usize) -> Self {
        let padded = edge + 2;
        let mut random = 0x9E37_79B9_7F4A_7C15u64;
        let mut blocks = Vec::with_capacity(padded * padded * padded);
        let mut sky = Vec::with_capacity(padded * padded * padded);
        for _ in 0..padded * padded * padded {
            blocks.push(BlockLight::new(
                if next(&mut random).is_multiple_of(13) {
                    15
                } else {
                    0
                },
                (next(&mut random) % 5) as u8,
            ));
            sky.push(next(&mut random).is_multiple_of(9));
        }
        Self {
            padded,
            blocks,
            sky,
        }
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockLight {
        self.blocks[self.index(x, y, z)]
    }

    fn sky_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.sky[self.index(x, y, z)]
    }

    fn set(&mut self, x: i32, y: i32, z: i32, block: BlockLight, sky: bool) {
        let index = self.index(x, y, z);
        self.blocks[index] = block;
        self.sky[index] = sky;
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        (x + 1) as usize
            + (y + 1) as usize * self.padded
            + (z + 1) as usize * self.padded * self.padded
    }
}

fn next_edits(random: &mut u64, grid: &mut Grid, low: i32, high: i32) -> Vec<CellEdit> {
    let count = 1 + (next(random) % 3) as usize;
    let mut edits = Vec::with_capacity(count);
    for _ in 0..count {
        let x = low + (next(random) % (high - low) as u64) as i32;
        let y = low + (next(random) % (high - low) as u64) as i32;
        let z = low + (next(random) % (high - low) as u64) as i32;
        let block = BlockLight::new(
            if next(random).is_multiple_of(7) {
                15
            } else {
                0
            },
            (next(random) % 6) as u8,
        );
        let sky = next(random).is_multiple_of(5);
        grid.set(x, y, z, block, sky);
        edits.push(CellEdit::new(x, y, z, block, sky));
    }
    edits
}

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
