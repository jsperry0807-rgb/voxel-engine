//! Block mesh assembly (M2-11 groundwork).
//!
//! Converts the greedy rectangles for one block into the ADR-010 indexed vertex
//! stream. Vertices are chunk-local and the index buffer is meshlet-local, so a
//! block can be uploaded or hashed independently of its neighbours.

use voxel_core::Face;

use crate::BLOCK_EDGE;
use crate::BlockPos;
use crate::greedy::{GreedyQuad, corner_signs, face_axis};
use crate::vertex::Vertex;

/// One block's vertex and index buffers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mesh {
    /// Four [`Vertex`] records per quad.
    pub vertices: Vec<Vertex>,
    /// Six meshlet-local indices per quad.
    pub indices: Vec<u16>,
}

impl Mesh {
    /// Number of quads in the mesh.
    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    /// Vertex bytes plus index bytes.
    pub fn byte_len(&self) -> usize {
        self.vertices.len() * Vertex::SIZE + self.indices.len() * 2
    }
}

/// Builds the mesh for one block's greedy rectangles.
///
/// `block` supplies the chunk-local origin. A full block emits at most 11,520
/// quads (16 × 16 faces × 6 orientations), which is 46,080 vertices and still
/// fits the meshlet-local `u16` index range.
///
/// # Panics
///
/// Panics when a quad is outside the block or has a zero or out-of-range
/// extent: `GreedyQuad` has public fields, so a hand-built quad can violate the
/// contract that [`greedy_block`] and [`crate::greedy_block_with_ao`] maintain.
pub fn mesh_block(quads: &[GreedyQuad], block: BlockPos) -> Mesh {
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(quads.len() * 4);
    mesh.indices.reserve(quads.len() * 6);
    let block_origin = block.origin();

    for quad in quads {
        assert!(
            quad.origin
                .iter()
                .all(|&corner| (1..=BLOCK_EDGE as u32).contains(&corner)),
            "quad origin {:?} is outside the 16³ block interior",
            quad.origin
        );
        assert!(
            quad.extent.iter().all(|&extent| extent > 0),
            "quad extent {:?} is empty",
            quad.extent
        );
        for axis in 0..3 {
            assert!(
                quad.origin[axis] + quad.extent[axis] <= BLOCK_EDGE as u32 + 1,
                "quad origin {:?} extent {:?} runs past the block",
                quad.origin,
                quad.extent
            );
        }

        let base = mesh.vertices.len();
        assert!(
            base + 4 <= u16::MAX as usize + 1,
            "block mesh exceeds the meshlet-local u16 vertex range"
        );
        let vertices = quad_vertices(quad, block_origin);
        mesh.vertices.extend_from_slice(&vertices);
        for index in [0u16, 1, 2, 0, 2, 3] {
            mesh.indices.push((base as u16) + index);
        }
    }
    mesh
}

fn quad_vertices(quad: &GreedyQuad, block_origin: [u32; 3]) -> [Vertex; 4] {
    let (axis, sign) = face_axis(quad.face);
    let u_axis = (axis + 1) % 3;
    let v_axis = (axis + 2) % 3;
    let cell_axis = quad.origin[axis] - 1;
    let plane = cell_axis + u32::from(sign > 0);
    let u0 = quad.origin[u_axis] - 1;
    let v0 = quad.origin[v_axis] - 1;
    let face = mesh_face(quad.face);

    corner_signs(quad.face).map(|(u_sign, v_sign)| {
        let mut local = [0u16; 3];
        local[axis] = (block_origin[axis] + plane) as u16;
        local[u_axis] =
            (block_origin[u_axis] + u0 + if u_sign < 0 { 0 } else { quad.extent[u_axis] }) as u16;
        local[v_axis] =
            (block_origin[v_axis] + v0 + if v_sign < 0 { 0 } else { quad.extent[v_axis] }) as u16;
        Vertex::new(local, quad.material, face, quad.ao)
    })
}

/// ADR-010 face code: `0..5 = +X, -X, +Y, -Y, +Z, -Z`.
pub const fn mesh_face(face: Face) -> u8 {
    match face {
        Face::East => 0,
        Face::West => 1,
        Face::Top => 2,
        Face::Bottom => 3,
        Face::South => 4,
        Face::North => 5,
    }
}
