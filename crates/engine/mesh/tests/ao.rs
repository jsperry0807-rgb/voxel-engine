//! M2-07: AO sampling matches a brute-force corner reference.
//!
//! The reference below repeats the three-neighbour rule from the scratch rather
//! than calling the production sampler. The random scratch has solid and open
//! cells, so all four AO levels and all six face orientations are exercised.

use voxel_core::Face;
use voxel_mesh::{BlockScratch, SCRATCH_EDGE, SCRATCH_VOXELS, corner_ao};

#[test]
fn ao_matches_bruteforce() {
    let mut state = 0x4D31_2F28_2026_1026u64;
    let mut voxels = [0u16; SCRATCH_VOXELS];
    for voxel in &mut voxels {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *voxel = if state % 10 < 3 {
            1 + (state % 3) as u16
        } else {
            0
        };
    }
    let scratch = BlockScratch::from_voxels(voxels);

    for _ in 0..10_000 {
        let face = Face::ALL[(next(&mut state) % 6) as usize];
        let cell = [
            (1 + next(&mut state) % BLOCK_EDGE) as u32,
            (1 + next(&mut state) % BLOCK_EDGE) as u32,
            (1 + next(&mut state) % BLOCK_EDGE) as u32,
        ];
        let corner = (next(&mut state) % 4) as usize;

        let got = corner_ao(&scratch, face, cell, corner);
        let expected = brute_force(&scratch, face, cell, corner);
        assert_eq!(
            got, expected,
            "{face:?} cell {cell:?} corner {corner}: production {got}, brute force {expected}"
        );
    }
}

const BLOCK_EDGE: u64 = (SCRATCH_EDGE - 2) as u64;

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn brute_force(scratch: &BlockScratch, face: Face, cell: [u32; 3], corner: usize) -> u8 {
    let (axis, sign) = axis_sign(face);
    let u_axis = (axis + 1) % 3;
    let v_axis = (axis + 2) % 3;
    let (du, dv) = corner_signs(face)[corner];
    let outer = (cell[axis] as i32 + sign) as u32;
    let u = cell[u_axis] as i32 + du;
    let v = cell[v_axis] as i32 + dv;

    let sample = |u: i32, v: i32| {
        let mut sample = [0u32; 3];
        sample[axis] = outer;
        sample[u_axis] = u as u32;
        sample[v_axis] = v as u32;
        scratch.get(sample[0], sample[1], sample[2])
    };

    let side_u = sample(u, cell[v_axis] as i32) != 0;
    let side_v = sample(cell[u_axis] as i32, v) != 0;
    let diagonal = sample(u, v) != 0;
    let level = if side_u && side_v {
        0
    } else {
        3 - usize::from(side_u) - usize::from(side_v) - usize::from(diagonal)
    };
    [0, 85, 170, 255][level]
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

fn corner_signs(face: Face) -> [(i32, i32); 4] {
    match face {
        Face::East | Face::Bottom | Face::North => [(-1, -1), (-1, 1), (1, 1), (1, -1)],
        Face::West | Face::Top | Face::South => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
    }
}
