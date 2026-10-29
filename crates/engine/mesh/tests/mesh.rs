//! M2-11 emitter sanity: the vertex stream uses the expected block corners.

use voxel_core::Face;
use voxel_mesh::{Mesh, for_each_block, greedy_block, mesh_block};
use voxel_storage::{Apron, CHUNK_VOLUME, local_index};

#[test]
fn mesh_block_single_voxel_uses_outward_corners() {
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    voxels[local_index(8, 8, 8) as usize] = 1;
    let apron = Apron::new();
    let mut mesh = Mesh::default();

    for_each_block(&voxels, &apron, |block, scratch| {
        if block.x == 0 && block.y == 0 && block.z == 0 {
            let mut quads = Vec::new();
            greedy_block(scratch, &mut quads);
            mesh = mesh_block(&quads, block);
        }
    });

    assert_eq!(mesh.quad_count(), 6);
    assert_eq!(mesh.vertices.len(), 24);
    assert_eq!(mesh.indices.len(), 36);
    assert_eq!(mesh.byte_len(), 6 * 76);

    let top = mesh
        .vertices
        .iter()
        .find(|vertex| vertex.face == 2 && vertex.local[1] == 9)
        .expect("top face vertices at y = 9");
    assert_eq!(top.material, 1);
    assert_eq!(top.ao, [255; 4]);
    assert!(top.local[0] == 8 || top.local[0] == 9);
    assert!(top.local[2] == 8 || top.local[2] == 9);

    let bottom = mesh
        .vertices
        .iter()
        .find(|vertex| vertex.face == 3)
        .expect("bottom face vertices");
    assert_eq!(bottom.local[1], 8);

    // Every face appears exactly once and is a quad of four vertices.
    for face in Face::ALL {
        let code = match face {
            Face::East => 0,
            Face::West => 1,
            Face::Top => 2,
            Face::Bottom => 3,
            Face::South => 4,
            Face::North => 5,
        };
        assert_eq!(
            mesh.vertices.iter().filter(|v| v.face == code).count(),
            4,
            "{face}"
        );
    }
}
