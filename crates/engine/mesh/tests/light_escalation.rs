//! M2-16: removals near a chunk edge escalate to a full relight.
//!
//! Two geometries:
//!
//! - **synthetic 40³ cells**: the smallest size where a removal front can stay
//!   inside the chunk, so the clipped-window path is reachable at all. This is
//!   a unit test of the window arithmetic; production never uses this size.
//! - **production 4³/8³ cells**: the real light grids. A removal is at most
//!   `MAX_LEVEL` cells from a face here, so every removal escalates. That is the
//!   behaviour the engine actually relies on, and it is pinned separately.

use voxel_mesh::light::{BlockLight, CellEdit, LightState, full_pass};

const SYNTHETIC_EDGE: usize = 40;
const BOUNDARY: [i32; 3] = [0, 20, 20];
const CENTRE: [i32; 3] = [20, 20, 20];

#[test]
fn light_escalation() {
    let mut grid = Grid::new(SYNTHETIC_EDGE);
    grid.set(BOUNDARY, BlockLight::emitter(15));
    grid.set(CENTRE, BlockLight::emitter(15));

    let mut state = LightState::new(
        SYNTHETIC_EDGE,
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
    assert_matches_full(&state, &grid, SYNTHETIC_EDGE);

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
        "a central removal stays on the clipped-window path at this synthetic size"
    );
    assert_matches_full(&state, &grid, SYNTHETIC_EDGE);
}

/// At the real 4³ and 8³ light grids every removal reaches a face, so the dirty
/// window is never clipped. A future optimisation must not quietly change that.
#[test]
fn production_edges_escalate_every_removal() {
    for edge in [4usize, 8] {
        let mut grid = Grid::new(edge);
        grid.set([0, 0, 0], BlockLight::emitter(15));

        let mut state = LightState::new(
            edge,
            |x, y, z| grid.block_at(x, y, z),
            |x, y, z| grid.sky_at(x, y, z),
        );
        assert_eq!(state.full_relights(), 0, "edge {edge}");

        grid.set([0, 0, 0], BlockLight::AIR);
        state.apply_edit(CellEdit::new(0, 0, 0, BlockLight::AIR, false));
        assert_eq!(
            state.full_relights(),
            1,
            "edge {edge}: a removal is always within MAX_LEVEL of a face"
        );
        assert_matches_full(&state, &grid, edge);
    }
}

fn assert_matches_full(state: &LightState, grid: &Grid, edge: usize) {
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
    padded: usize,
    blocks: Vec<BlockLight>,
    sky: Vec<bool>,
}

impl Grid {
    fn new(edge: usize) -> Self {
        let padded = edge + 2;
        Self {
            padded,
            blocks: vec![BlockLight::AIR; padded * padded * padded],
            sky: vec![false; padded * padded * padded],
        }
    }

    fn set(&mut self, [x, y, z]: [i32; 3], block: BlockLight) {
        let index = self.index(x, y, z);
        self.blocks[index] = block;
    }

    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockLight {
        self.blocks[self.index(x, y, z)]
    }

    fn sky_at(&self, x: i32, y: i32, z: i32) -> bool {
        self.sky[self.index(x, y, z)]
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        (x + 1) as usize
            + (y + 1) as usize * self.padded
            + (z + 1) as usize * self.padded * self.padded
    }
}
