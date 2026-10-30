//! The one-voxel border reads the matching apron face on all six sides.
//!
//! Each face is filled with a texel-unique pattern, so swapping `u`/`v`, using
//! the wrong face plane, or transposing the two in-plane axes changes the
//! value. The AO seam test only covers the ±X faces, which is exactly the case
//! where such a bug cannot be seen.

use voxel_mesh::{BLOCK_EDGE, for_each_block};
use voxel_storage::{Apron, CHUNK_DIM, CHUNK_VOLUME, Face};

const BASE: u16 = 1_000;

#[test]
fn every_apron_face_reaches_its_border() {
    let mut apron = Apron::new();
    for (slot, face) in Face::ALL.into_iter().enumerate() {
        for v in 0..Apron::SIDE {
            for u in 0..Apron::SIDE {
                apron.set(face, u, v, texel(slot as u16, u, v));
            }
        }
    }

    let chunk = vec![0u16; CHUNK_VOLUME];
    let blocks = (CHUNK_DIM / BLOCK_EDGE) as u32;
    let last = blocks - 1;
    for_each_block(&chunk, &apron, |pos, scratch| {
        for local_z in 0..BLOCK_EDGE as u32 {
            for local_y in 0..BLOCK_EDGE as u32 {
                for local_x in 0..BLOCK_EDGE as u32 {
                    let x = pos.x * BLOCK_EDGE as u32 + local_x;
                    let y = pos.y * BLOCK_EDGE as u32 + local_y;
                    let z = pos.z * BLOCK_EDGE as u32 + local_z;
                    let scratch_x = 1 + local_x;
                    let scratch_y = 1 + local_y;
                    let scratch_z = 1 + local_z;

                    if pos.x == 0 {
                        assert_eq!(
                            scratch.get(0, scratch_y, scratch_z),
                            texel(0, z, y),
                            "NegX at ({x}, {y}, {z})"
                        );
                    }
                    if pos.x == last {
                        assert_eq!(
                            scratch.get(17, scratch_y, scratch_z),
                            texel(1, z, y),
                            "PosX at ({x}, {y}, {z})"
                        );
                    }
                    if pos.y == 0 {
                        assert_eq!(
                            scratch.get(scratch_x, 0, scratch_z),
                            texel(2, z, x),
                            "NegY at ({x}, {y}, {z})"
                        );
                    }
                    if pos.y == last {
                        assert_eq!(
                            scratch.get(scratch_x, 17, scratch_z),
                            texel(3, z, x),
                            "PosY at ({x}, {y}, {z})"
                        );
                    }
                    if pos.z == 0 {
                        assert_eq!(
                            scratch.get(scratch_x, scratch_y, 0),
                            texel(4, x, y),
                            "NegZ at ({x}, {y}, {z})"
                        );
                    }
                    if pos.z == last {
                        assert_eq!(
                            scratch.get(scratch_x, scratch_y, 17),
                            texel(5, x, y),
                            "PosZ at ({x}, {y}, {z})"
                        );
                    }
                }
            }
        }
    });
}

/// A per-face base plus an asymmetric `(u, v)` pattern.
fn texel(slot: u16, u: u32, v: u32) -> u16 {
    BASE + slot * 1_000 + (u as u16) * 16 + v as u16
}
