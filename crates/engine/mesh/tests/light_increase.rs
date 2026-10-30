//! M2-17: the increase-only fast path matches a full pass.
//!
//! `LightState::apply_edits` skips the dirty-region reseed when every edit only
//! adds emission, lowers opacity or gains sky. These cases pin the fast path
//! separately from the random mixed batches in `light_incremental.rs`: an
//! emitter addition, an opacity decrease, a sky gain in the apron and a
//! concatenated sequence. None may escalate, and every padded cell must match a
//! fresh full pass.

use voxel_mesh::light::{BlockLight, CellEdit, LightState, full_pass};

const EDGE: usize = 8;

#[test]
fn increase_only_edits_match_full_without_escalation() {
    let mut grid = Grid::new(EDGE);
    // A lit starting world: a border torch and an opaque plug the later edits
    // make transparent.
    grid.set([EDGE as i32 - 1, 4, 4], BlockLight::emitter(12));
    grid.set([3, 3, 3], BlockLight::OPAQUE);

    let mut state = LightState::new(
        EDGE,
        |x, y, z| grid.block_at(x, y, z),
        |x, y, z| grid.sky_at(x, y, z),
    );

    let edits = [
        // Emission increase in the interior.
        CellEdit::new(4, 4, 4, BlockLight::emitter(15), false),
        // Opacity decrease: light can now enter and leave the plug.
        CellEdit::new(3, 3, 3, BlockLight::new(0, 4), false),
        // Sky gain in the apron.
        CellEdit::new(-1, 2, 2, BlockLight::AIR, true),
        // Emission increase in the apron.
        CellEdit::new(-1, 5, 5, BlockLight::emitter(9), false),
    ];

    for (index, edit) in edits.into_iter().enumerate() {
        grid.set([edit.x, edit.y, edit.z], edit.block);
        grid.set_sky([edit.x, edit.y, edit.z], edit.sky);
        state.apply_edit(edit);
        assert_eq!(
            state.full_relights(),
            0,
            "edit {index} must stay on the increase-only path"
        );
        assert_matches_full(&state, &grid, EDGE, index);
    }
}

fn assert_matches_full(state: &LightState, grid: &Grid, edge: usize, step: usize) {
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
                    "step {step}: sky ({x}, {y}, {z})"
                );
                assert_eq!(
                    got.block_level(),
                    expected.block_level(),
                    "step {step}: block ({x}, {y}, {z})"
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

    fn set_sky(&mut self, [x, y, z]: [i32; 3], value: bool) {
        let index = self.index(x, y, z);
        self.sky[index] = value;
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
