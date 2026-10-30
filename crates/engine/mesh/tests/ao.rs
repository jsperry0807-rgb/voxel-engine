//! M2-07: AO follows the geometry of the emitted quad.
//!
//! Each case places one solid voxel with a randomised 3×3 occlusion pattern in
//! its outward plane, then reads the mesh back. For every emitted vertex the
//! expected AO is computed from that vertex's *position*: which of the three
//! neighbouring cells (two edges and the diagonal) are solid. The expectation
//! therefore never uses the implementation's corner table, so a corner order
//! that disagrees with the vertex emitter fails here.

use voxel_core::Face;
use voxel_mesh::{
    BlockPos, BlockScratch, SCRATCH_VOXELS, greedy_block, index as scratch_index, mesh_block,
};

const CELLS: [[u32; 3]; 3] = [[8, 8, 8], [2, 2, 2], [15, 15, 15]];
const PATTERNS: usize = 40;

#[test]
fn ao_matches_the_occlusion_around_each_emitted_vertex() {
    let mut state = 0x4D31_2F28_2026_1026u64;
    for face in Face::ALL {
        let (axis, sign) = axis_sign(face);
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        for cell in CELLS {
            for _ in 0..PATTERNS {
                check(face, axis, sign, u_axis, v_axis, cell, &mut state);
            }
        }
    }
}

fn check(
    face: Face,
    axis: usize,
    sign: i32,
    u_axis: usize,
    v_axis: usize,
    cell: [u32; 3],
    state: &mut u64,
) {
    let mut voxels = [0u16; SCRATCH_VOXELS];
    voxels[scratch_index(cell[0], cell[1], cell[2])] = 1;

    // The centre of the outward plane stays air so the face is exposed; the
    // eight surrounding cells are the AO samples.
    let plane = (cell[axis] as i32 + sign) as u32;
    for du in -1..=1 {
        for dv in -1..=1 {
            if du == 0 && dv == 0 {
                continue;
            }
            let mut sample = cell;
            sample[axis] = plane;
            sample[u_axis] = (cell[u_axis] as i32 + du) as u32;
            sample[v_axis] = (cell[v_axis] as i32 + dv) as u32;
            if next(state).is_multiple_of(2) {
                voxels[scratch_index(sample[0], sample[1], sample[2])] = 1;
            }
        }
    }

    let scratch = BlockScratch::from_voxels(voxels);
    let mut quads = Vec::new();
    greedy_block(&scratch, &mut quads);
    let quad = quads
        .iter()
        .copied()
        .find(|quad| quad.face == face && quad.origin == cell)
        .unwrap_or_else(|| panic!("{face} has no quad at {cell:?}: {quads:?}"));
    let mesh = mesh_block(&[quad], BlockPos::new(0, 0, 0));
    assert_eq!(mesh.vertices.len(), 4, "{face} {cell:?}");

    let mut expected = [0u8; 4];
    for (corner, vertex) in mesh.vertices.iter().enumerate() {
        // Scratch cell of the vertex, and which side of the voxel it sits on.
        let vertex_u = i32::from(vertex.local[u_axis]) + 1;
        let vertex_v = i32::from(vertex.local[v_axis]) + 1;
        let dir_u = corner_direction(vertex_u, cell[u_axis] as i32);
        let dir_v = corner_direction(vertex_v, cell[v_axis] as i32);

        let solid = |u: i32, v: i32| {
            let mut sample = [0u32; 3];
            sample[axis] = plane;
            sample[u_axis] = u as u32;
            sample[v_axis] = v as u32;
            scratch.get(sample[0], sample[1], sample[2]) != 0
        };
        let side_u = solid(cell[u_axis] as i32 + dir_u, cell[v_axis] as i32);
        let side_v = solid(cell[u_axis] as i32, cell[v_axis] as i32 + dir_v);
        let diagonal = solid(cell[u_axis] as i32 + dir_u, cell[v_axis] as i32 + dir_v);
        expected[corner] = if side_u && side_v {
            0
        } else {
            [0, 85, 170, 255][3 - usize::from(side_u) - usize::from(side_v) - usize::from(diagonal)]
        };
    }

    assert_eq!(
        mesh.vertices[0].ao, expected,
        "{face} {cell:?}: AO does not match the occlusion around the emitted vertices"
    );
}

fn axis_sign(face: Face) -> (usize, i32) {
    let (x, y, z) = face.normal();
    if x != 0 {
        (0, x)
    } else if y != 0 {
        (1, y)
    } else {
        (2, z)
    }
}

/// `-1` when the vertex sits on the voxel's minimum side, `+1` on its maximum
/// side. Scratch coordinates run one cell past the block origin, so the min
/// side is the voxel's own cell and the max side is one further along.
fn corner_direction(vertex: i32, cell: i32) -> i32 {
    if vertex == cell {
        -1
    } else {
        assert_eq!(vertex, cell + 1, "vertex is not on a voxel corner");
        1
    }
}

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
