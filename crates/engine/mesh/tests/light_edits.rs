//! Light edits are chunk-local plus the one-cell apron.
//!
//! Editing further out used to alias onto a different cell (and panic past the
//! second cell), because the padded index is a bare add. These tests pin the
//! rejection and the ingest clamping.

use voxel_mesh::light::{BlockLight, CellEdit, LightState};

fn new_state(edge: usize) -> LightState {
    LightState::new(edge, |_, _, _| BlockLight::AIR, |_, _, _| false)
}

#[test]
fn accepts_edits_inside_the_apron() {
    let mut state = new_state(4);
    for (x, y, z) in [(-1, 0, 0), (4, 4, 4), (0, -1, 3), (2, 2, 4)] {
        state.apply_edit(CellEdit::new(x, y, z, BlockLight::emitter(15), false));
        assert_eq!(
            state.with_apron(x, y, z).block_level(),
            15,
            "edit at ({x}, {y}, {z})"
        );
    }
}

#[test]
#[should_panic(expected = "outside the padded chunk")]
fn rejects_edits_past_the_apron() {
    let mut state = new_state(4);
    state.apply_edit(CellEdit::new(6, 0, 0, BlockLight::AIR, false));
}

#[test]
#[should_panic(expected = "outside the padded chunk")]
fn rejects_reads_past_the_apron() {
    let _ = new_state(4).with_apron(0, 0, -2);
}

#[test]
fn clamps_out_of_range_material_values() {
    // Emission: 200 is clamped to 15, and the cell still lights its neighbour.
    let mut state = new_state(4);
    state.apply_edit(CellEdit::new(
        2,
        2,
        2,
        BlockLight {
            emission: 200,
            opacity: 0,
        },
        false,
    ));
    assert_eq!(
        state.local(2, 2, 2).block_level(),
        15,
        "emission clamps to 15"
    );

    // Opacity: 255 is clamped to 15, so the neighbour's light cannot enter.
    // An unclamped value would overflow `1 + opacity` and leak through.
    let mut state = new_state(4);
    state.apply_edits(&[
        CellEdit::new(0, 1, 1, BlockLight::emitter(15), false),
        CellEdit::new(
            1,
            1,
            1,
            BlockLight {
                emission: 0,
                opacity: 255,
            },
            false,
        ),
    ]);
    assert_eq!(
        state.local(1, 1, 1).block_level(),
        0,
        "an opaque cell must not receive light"
    );
}
