//! M2-16: removals near a chunk edge escalate to a full relight.
//!
//! A boundary emitter is removed, forcing the escalation path; a central
//! emitter removal stays on the dirty-region path. Both results must still
//! equal a fresh full pass.

use voxel_mesh::light::{BlockLight, CellEdit, LightState, full_pass};

const EDGE: usize = 40;
const PADDED: usize = EDGE + 2;
const BOUNDARY: [i32; 3] = [0, 20, 20];
const CENTRE: [i32; 3] = [20, 20, 20];

#[test]
fn light_escalation() {
    let mut grid = Grid::new();
    grid.set(BOUNDARY, BlockLight::emitter(15));
    grid.set(CENTRE, BlockLight::emitter(15));

    let mut state = LightState::new(
        EDGE,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );
    assert_eq!(state.full_relights(), 0);

    grid.set(BOUNDARY, BlockLight::AIR);
    state.apply_edit(CellEdit::new(
        BOUNDARY[0],
        BOUNDARY[1],
        BOUNDARY[2],
        BlockLight::AIR,
        false,
    ));
    assert_eq!(
        state.full_relights(),
        1,
        "a removal at the edge must escalate"
    );
    assert_matches_full(&state, &grid);

    grid.set(CENTRE, BlockLight::AIR);
    state.apply_edit(CellEdit::new(
        CENTRE[0],
        CENTRE[1],
        CENTRE[2],
        BlockLight::AIR,
        false,
    ));
    assert_eq!(
        state.full_relights(),
        1,
        "a central removal must stay incremental"
    );
    assert_matches_full(&state, &grid);
}

fn assert_matches_full(state: &LightState, grid: &Grid) {
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
                assert_eq!(got.sky_level(), expected.sky_level(), "sky ({x}, {y}, {z})");
                assert_eq!(
                    got.block_level(),
                    expected.block_level(),
                    "block ({x}, {y}, {z})"
                );
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
        Self {
            blocks: vec![BlockLight::AIR; PADDED * PADDED * PADDED],
            sky: vec![false; PADDED * PADDED * PADDED],
        }
    }

    fn set(&mut self, [x, y, z]: [i32; 3], block: BlockLight) {
        self.blocks[index(x, y, z)] = block;
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockLight {
        self.blocks[index(x, y, z)]
    }

    fn sky_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.sky[index(x, y, z)]
    }
}

fn index(x: i32, y: i32, z: i32) -> usize {
    (x + 1) as usize + (y + 1) as usize * PADDED + (z + 1) as usize * PADDED * PADDED
}
