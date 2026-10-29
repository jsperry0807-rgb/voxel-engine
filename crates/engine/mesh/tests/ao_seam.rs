//! M2-09: AO must agree across a chunk border.
//!
//! Two chunks hold a surface that is constant along x and steps in z. The
//! shared x face is copied into the aprons in both directions. The border
//! blocks on A and B must produce the same four corner AO values; shifting the
//! copied plane by one z voxel must make them differ.

use voxel_core::Face;
use voxel_mesh::{BLOCK_EDGE, corner_ao, for_each_block};
use voxel_storage::{Apron, CHUNK_DIM, CHUNK_VOLUME, Face as ApronFace, local_index};

#[test]
fn ao_seam() {
    let a = surface();
    let b = a.clone();
    let blocks = (CHUNK_DIM / BLOCK_EDGE) as u32;

    let mut apron_a = Apron::new();
    let mut apron_b = Apron::new();
    copy_column(&mut apron_a, ApronFace::PosX, &b, 0, 0);
    copy_column(&mut apron_b, ApronFace::NegX, &a, CHUNK_DIM as u32 - 1, 0);

    let a_border = border_ao(&a, &apron_a, blocks - 1);
    let b_border = border_ao(&b, &apron_b, 0);
    assert_eq!(a_border, b_border, "the two sides must see the same AO");

    let mut shifted = Apron::new();
    copy_column(&mut shifted, ApronFace::PosX, &b, 0, 1);
    let shifted_border = border_ao(&a, &shifted, blocks - 1);
    assert_ne!(
        shifted_border, a_border,
        "an off-by-one apron must change the border AO"
    );
}

/// AO of the top face of every surface column in the border block at `bx`.
fn border_ao(voxels: &[u16], apron: &Apron, bx: u32) -> Vec<(u32, [u8; 4])> {
    let mut out = Vec::new();
    for_each_block(voxels, apron, |pos, scratch| {
        if pos.x != bx {
            return;
        }
        for local_z in 0..BLOCK_EDGE as u32 {
            let z = pos.z * BLOCK_EDGE as u32 + local_z;
            let y = height(z) - 1;
            if y / BLOCK_EDGE as u32 != pos.y {
                continue;
            }
            let scratch_x = if bx == 0 { 1 } else { BLOCK_EDGE as u32 };
            let cell = [scratch_x, 1 + y % BLOCK_EDGE as u32, 1 + local_z];
            let ao = std::array::from_fn(|corner| corner_ao(scratch, Face::Top, cell, corner));
            out.push((z, ao));
        }
    });
    out.sort_unstable_by_key(|(z, _)| *z);
    out
}

/// A surface constant in x with a height step every z voxel.
fn surface() -> Vec<u16> {
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        let top = height(z as u32);
        for x in 0..CHUNK_DIM {
            for y in 0..top {
                voxels[local_index(x as u32, y, z as u32) as usize] = 1;
            }
        }
    }
    voxels
}

fn height(z: u32) -> u32 {
    24 + z % 4
}

fn copy_column(apron: &mut Apron, face: ApronFace, source: &[u16], x: u32, z_shift: i32) {
    for y in 0..CHUNK_DIM as u32 {
        for z in 0..CHUNK_DIM as u32 {
            let source_z = (z as i32 + z_shift).rem_euclid(CHUNK_DIM as i32) as u32;
            apron.set(face, z, y, source[local_index(x, y, source_z) as usize]);
        }
    }
}

const _: () = assert!(CHUNK_DIM.is_multiple_of(BLOCK_EDGE));
const _: () = assert!(CHUNK_DIM > 32);
