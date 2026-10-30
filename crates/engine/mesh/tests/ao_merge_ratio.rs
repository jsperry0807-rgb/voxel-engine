//! M2-08: measure the merge-ratio cost of AO-safe quads.
//!
//! The same terrain scene is meshed twice: once with the real four-level AO
//! sampler and once with all corner AO forced open. The covered-face count must
//! be identical; the AO-on quad count may only be higher because AO differences
//! split otherwise mergeable rectangles.

use voxel_mesh::{greedy_block, greedy_block_with_ao};
use voxel_storage::{Apron, CHUNK_DIM, CHUNK_VOLUME, local_index};

#[test]
fn ao_merge_ratio() {
    let voxels = terrain();
    let apron = Apron::new();
    let (ao_quads, ao_faces) = measure(&voxels, &apron, true);
    let (plain_quads, plain_faces) = measure(&voxels, &apron, false);

    assert_eq!(ao_faces, plain_faces, "AO must not change covered faces");
    assert!(
        ao_quads > plain_quads,
        "the fixture must contain at least one AO split"
    );

    let ao_ratio = ao_quads as f64 / ao_faces as f64;
    let plain_ratio = plain_quads as f64 / plain_faces as f64;
    let cost = ao_ratio / plain_ratio;
    println!(
        "ao_merge_ratio: faces={ao_faces} quads_ao={ao_quads} ratio_ao={ao_ratio:.4} \
         quads_plain={plain_quads} ratio_plain={plain_ratio:.4} cost={cost:.2}x"
    );
    // M2-08 asks for the cost, not for a limit, but an unguarded ratio would
    // let a merge regression through: the fixture measures ~1.65x, so 2x is
    // headroom without accepting a real regression.
    assert!(
        cost <= 2.0,
        "AO-safe quads cost {cost:.2}x the merge ratio, above the 2x budget"
    );
}

fn measure(voxels: &[u16], apron: &Apron, ao_on: bool) -> (usize, usize) {
    let mut block = Vec::new();
    let mut quads = 0;
    let mut faces = 0;
    voxel_mesh::for_each_block(voxels, apron, |_, scratch| {
        if ao_on {
            greedy_block(scratch, &mut block);
        } else {
            greedy_block_with_ao(scratch, &mut block, |_, _, _, _| u8::MAX);
        }
        quads += block.len();
        faces += block.iter().map(|quad| quad.area() as usize).sum::<usize>();
    });
    (quads, faces)
}

fn terrain() -> Vec<u16> {
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let height = 24.0
                + (x as f32 * 0.15).sin() * 5.0
                + (z as f32 * 0.11).cos() * 4.0
                + ((x + z) as f32 * 0.07).sin() * 3.0;
            let height = height as usize;
            for y in 0..height {
                let material = if y + 1 == height {
                    3
                } else if y + 4 >= height {
                    2
                } else {
                    1
                };
                set(&mut voxels, x, y, z, material);
            }
        }
    }

    // A small stone outcrop creates concave corners and therefore AO splits.
    for z in 20..24 {
        for x in 20..24 {
            for y in 30..35 {
                set(&mut voxels, x, y, z, 1);
            }
        }
    }
    voxels
}

fn set(voxels: &mut [u16], x: usize, y: usize, z: usize, material: u16) {
    voxels[local_index(x as u32, y as u32, z as u32) as usize] = material;
}

const _: () = assert!(CHUNK_DIM > 32);
