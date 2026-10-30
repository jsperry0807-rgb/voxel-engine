//! M2-06: only outward exposed faces are emitted.
//!
//! The greedy core checks the outward neighbour before putting a cell in the
//! mask, so a solid neighbour removes the face regardless of material. These
//! tests pin the three cases from the task: isolated voxel, two adjacent
//! voxels, and a fully enclosed block.

use voxel_core::Face;
use voxel_mesh::{BlockPos, for_each_block, greedy_block};
use voxel_storage::{Apron, BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME, local_index};

#[test]
fn backface_isolated_voxel_has_six_outward_quads() {
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    set(&mut voxels, 8, 8, 8, 1);

    let quads = all_quads(&voxels, &Apron::new());
    assert_eq!(quads.len(), 6);
    for face in Face::ALL {
        assert_eq!(
            quads.iter().filter(|quad| quad.face == face).count(),
            1,
            "{face}"
        );
    }
}

#[test]
fn backface_two_adjacent_voxels_merge_the_shared_face_away() {
    // Same material merges the coplanar outer faces to six quads. Different
    // materials keep the face split, so the same ten exposed faces become ten
    // quads. The shared face is interior in both cases.
    for (materials, expected_quads) in [([1, 1], 6), ([1, 2], 10)] {
        let mut voxels = vec![0u16; CHUNK_VOLUME];
        set(&mut voxels, 8, 8, 8, materials[0]);
        set(&mut voxels, 9, 8, 8, materials[1]);

        let quads = all_quads(&voxels, &Apron::new());
        assert_eq!(quads.len(), expected_quads, "materials {materials:?}");
        assert_eq!(
            quads.iter().map(|quad| quad.area()).sum::<u32>(),
            10,
            "materials {materials:?}: the shared face must be culled"
        );
        assert_eq!(count_face(&quads, Face::East), 1);
        assert_eq!(count_face(&quads, Face::West), 1);
        let side_quads = if materials[0] == materials[1] { 1 } else { 2 };
        assert_eq!(count_face(&quads, Face::North), side_quads);
        assert_eq!(count_face(&quads, Face::South), side_quads);
        assert_eq!(count_face(&quads, Face::Top), side_quads);
        assert_eq!(count_face(&quads, Face::Bottom), side_quads);
    }
}

#[test]
fn backface_enclosed_block_has_no_quads() {
    // A solid chunk: an interior block has solid neighbours on all six sides,
    // so every one of its 24,576 faces (16³ × 6) is interior.
    let voxels = vec![1u16; CHUNK_VOLUME];
    let apron = Apron::new();
    let mut enclosed = Vec::new();

    for_each_block(&voxels, &apron, |pos, scratch| {
        if pos == BlockPos::new(1, 1, 1) {
            greedy_block(scratch, &mut enclosed);
        }
    });

    assert!(enclosed.is_empty(), "enclosed block emitted {enclosed:?}");
}

fn all_quads(voxels: &[u16], apron: &Apron) -> Vec<voxel_mesh::GreedyQuad> {
    let mut quads = Vec::new();
    for_each_block(voxels, apron, |_, scratch| {
        let mut block = Vec::new();
        greedy_block(scratch, &mut block);
        quads.extend(block);
    });
    quads
}

fn count_face(quads: &[voxel_mesh::GreedyQuad], face: Face) -> usize {
    quads.iter().filter(|quad| quad.face == face).count()
}

fn set(voxels: &mut [u16], x: usize, y: usize, z: usize, material: u16) {
    voxels[local_index(x as u32, y as u32, z as u32) as usize] = material;
}

const _: () = assert!(BLOCK_DIM == 16);
const _: () = assert!(CHUNK_DIM.is_multiple_of(BLOCK_DIM));
