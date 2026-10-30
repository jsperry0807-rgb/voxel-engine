//! M2-11 emitter sanity: the vertex stream uses the expected block corners.

use voxel_core::Face;
use voxel_mesh::{Mesh, Vertex, for_each_block, greedy_block, mesh_block, mesh_face};
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

/// Every emitted quad must be counter-clockwise seen from outside the face:
/// the renderer culls back faces with `cull_mode: Back` and a CCW front face.
#[test]
fn faces_wind_outward() {
    let mesh = single_voxel_mesh();
    for face in Face::ALL {
        let code = mesh_face(face);
        let quad: Vec<&Vertex> = mesh
            .vertices
            .iter()
            .filter(|vertex| vertex.face == code)
            .collect();
        assert_eq!(quad.len(), 4, "{face}");
        let point = |vertex: &Vertex| {
            [
                vertex.local[0] as i64,
                vertex.local[1] as i64,
                vertex.local[2] as i64,
            ]
        };
        let [v0, v1, v2, _] = [quad[0], quad[1], quad[2], quad[3]];
        let edge1 = sub(point(v1), point(v0));
        let edge2 = sub(point(v2), point(v0));
        let normal = cross(edge1, edge2);
        let (nx, ny, nz) = face.normal();
        let dot = normal[0] * i64::from(nx) + normal[1] * i64::from(ny) + normal[2] * i64::from(nz);
        assert!(
            dot > 0,
            "{face} is wound inward: geometric normal {normal:?}"
        );
    }
}

fn single_voxel_mesh() -> Mesh {
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
    mesh
}

fn sub(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
