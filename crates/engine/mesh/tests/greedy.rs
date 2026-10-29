//! M2-04: greedy rectangles match the M1-31 reference on five fixtures.
//!
//! The reference is the slow count-only oracle in `voxel-storage::reference`.
//! The production path walks the same scenes through the block driver and the
//! `18³` scratch, then counts the rectangles it emits. Both must agree per face
//! orientation and cover exactly the same exposed faces.

use voxel_core::Face;
use voxel_mesh::{for_each_block, greedy_block};
use voxel_storage::reference::mesh::{count_exposed_faces_dense, count_greedy_dense};
use voxel_storage::{Apron, BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME, local_index};

const EDGE: usize = CHUNK_DIM;

#[test]
fn greedy_matches_reference() {
    for (name, voxels) in fixtures() {
        let reference = count_greedy_dense(EDGE, &voxels);
        let brute = count_exposed_faces_dense(EDGE, &voxels);
        let apron = Apron::new();
        let mut quads = Vec::new();
        let mut quads_by_face = [0usize; 6];
        let mut faces_by_face = [0usize; 6];

        for_each_block(&voxels, &apron, |_, scratch| {
            greedy_block(scratch, &mut quads);
            for quad in &quads {
                quads_by_face[quad.face.index()] += 1;
                faces_by_face[quad.face.index()] += quad.area() as usize;
            }
        });

        for face in Face::ALL {
            assert_eq!(
                quads_by_face[face.index()],
                reference.quads_for(face),
                "{name}: {face} quads"
            );
            assert_eq!(
                faces_by_face[face.index()],
                reference.covered_faces_for(face),
                "{name}: {face} covered faces"
            );
            assert_eq!(
                faces_by_face[face.index()],
                brute.get(face),
                "{name}: {face} brute-force faces"
            );
        }

        let total_quads: usize = quads_by_face.iter().sum();
        let total_faces: usize = faces_by_face.iter().sum();
        assert_eq!(total_quads, reference.quads());
        assert_eq!(total_faces, reference.covered_faces());
        assert!(
            total_quads <= brute.total(),
            "{name}: merging cannot add quads"
        );
    }
}

fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
    let mut isolated = vec![0u16; CHUNK_VOLUME];
    set(&mut isolated, 8, 8, 8, 1);

    let mut adjacent_voxels = vec![0u16; CHUNK_VOLUME];
    set(&mut adjacent_voxels, 8, 8, 8, 1);
    set(&mut adjacent_voxels, 9, 8, 8, 1);

    let mut solid_block = vec![0u16; CHUNK_VOLUME];
    fill_block(&mut solid_block, 0, 0, 0, 1);

    let mut adjacent_blocks = vec![0u16; CHUNK_VOLUME];
    fill_block(&mut adjacent_blocks, 0, 0, 0, 1);
    fill_block(&mut adjacent_blocks, 1, 0, 0, 1);

    let mut stripe = vec![0u16; CHUNK_VOLUME];
    for z in 0..BLOCK_DIM {
        for y in 0..BLOCK_DIM {
            for x in 0..BLOCK_DIM {
                let material = if x < BLOCK_DIM / 2 { 1 } else { 2 };
                set(&mut stripe, x, y, z, material);
            }
        }
    }

    vec![
        ("isolated_voxel", isolated),
        ("two_adjacent_voxels", adjacent_voxels),
        ("solid_block", solid_block),
        ("two_adjacent_blocks", adjacent_blocks),
        ("material_stripe_block", stripe),
    ]
}

fn set(voxels: &mut [u16], x: usize, y: usize, z: usize, material: u16) {
    voxels[local_index(x as u32, y as u32, z as u32) as usize] = material;
}

fn fill_block(voxels: &mut [u16], bx: usize, by: usize, bz: usize, material: u16) {
    for z in 0..BLOCK_DIM {
        for y in 0..BLOCK_DIM {
            for x in 0..BLOCK_DIM {
                set(
                    voxels,
                    bx * BLOCK_DIM + x,
                    by * BLOCK_DIM + y,
                    bz * BLOCK_DIM + z,
                    material,
                );
            }
        }
    }
}
