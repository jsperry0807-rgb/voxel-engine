//! M2-15: incremental light equals a full recompute after every edit set.
//!
//! A 8³-cell grid starts from a deterministic mixed field. Each of 1,000 sets
//! changes one to three cells (emission, opacity and sky exposure) and then
//! compares every chunk and apron cell with a fresh full pass over the same
//! data.

use voxel_mesh::light::{BlockLight, CellEdit, LightState, full_pass};

const EDGE: usize = 8;
const PADDED: usize = EDGE + 2;

#[test]
fn light_incremental_matches_full_recompute() {
    let mut grid = Grid::new();
    let mut state = LightState::new(
        EDGE,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );
    let mut random = 0x4D31_2F28_2026_1026u64;

    for set in 0..1_000 {
        let edits = next_edits(&mut random, &mut grid);
        state.apply_edits(&edits);
        let full = full_pass(
            EDGE,
            |x, y, z| grid.block_at(x, y, z),
            |x, y, z| grid.sky_at(x, y, z),
        );

        let last = EDGE as i32;
        for z in -1..=last {
            for y in -1..=last {
                for x in -1..=last {
                    let got = state.with_apron(x, y, z);
                    let expected = full.with_apron(x, y, z);
                    assert_eq!(
                        got.sky_level(),
                        expected.sky_level(),
                        "set {set}: sky at ({x}, {y}, {z})"
                    );
                    assert_eq!(
                        got.block_level(),
                        expected.block_level(),
                        "set {set}: block at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }
}

struct Grid {
    blocks: Vec<BlockLight>,
    sky: Vec<bool>,
}

impl Grid {
    fn new() -> Self {
        let mut random = 0x9E37_79B9_7F4A_7C15u64;
        let mut blocks = Vec::with_capacity(PADDED * PADDED * PADDED);
        let mut sky = Vec::with_capacity(PADDED * PADDED * PADDED);
        for _ in 0..PADDED * PADDED * PADDED {
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
        Self { blocks, sky }
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockLight {
        self.blocks[index(x, y, z)]
    }

    fn sky_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.sky[index(x, y, z)]
    }

    fn set(&mut self, x: i32, y: i32, z: i32, block: BlockLight, sky: bool) {
        let index = index(x, y, z);
        self.blocks[index] = block;
        self.sky[index] = sky;
    }
}

fn next_edits(random: &mut u64, grid: &mut Grid) -> Vec<CellEdit> {
    let count = 1 + (next(random) % 3) as usize;
    let mut edits = Vec::with_capacity(count);
    for _ in 0..count {
        let x = (next(random) % EDGE as u64) as i32;
        let y = (next(random) % EDGE as u64) as i32;
        let z = (next(random) % EDGE as u64) as i32;
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

fn index(x: i32, y: i32, z: i32) -> usize {
    (x + 1) as usize + (y + 1) as usize * PADDED + (z + 1) as usize * PADDED * PADDED
}

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
