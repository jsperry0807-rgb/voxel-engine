//! Naive per-face mesher for the M1.5 slice.
//!
//! This is the correctness reference, not the production mesher: every exposed
//! voxel face emits one quad, with no greedy merging and no AO calculation.
//! The quad and vertex types follow ADR-010 so M2's block meshlets can reuse
//! them.
//!
//! Surfaces outside the supplied chunk are treated as air by [`naive_mesh`].
//! [`naive_mesh_with_outside`] exists for tests and for callers that model an
//! enclosed sub-volume.

use bytemuck::{Pod, Zeroable};
use voxel_core::Face;
use voxel_core::constants::{CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME};

/// One 16-byte mesh vertex, laid out exactly as ADR-010 specifies.
///
/// `local` is a chunk-local voxel coordinate (`0..=CHUNK_DIM`), `material` is
/// the atlas id, `face` is the mesh face code, and `ao` is four corner values
/// in `0..=255`. M1.5-10 has no AO pass, so every corner is fully open (255).
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Vertex {
    pub local: [u16; 3],
    pub material: u16,
    pub face: u8,
    pub ao: [u8; 4],
    pub reserved: [u8; 3],
}

const _: () = assert!(std::mem::size_of::<Vertex>() == 16);
const _: () = assert!(std::mem::align_of::<Vertex>() == 4);

/// One quad: four vertices plus an implicit `(0,1,2), (0,2,3)` index pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quad {
    pub vertices: [Vertex; 4],
    pub face: Face,
    pub material: u16,
}

impl Quad {
    /// Local indices into this quad's four vertices.
    pub const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];
}

/// A flat mesh: four vertices and six indices per quad.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}

impl Mesh {
    /// Flattens quads into the vertex/index buffers while offsetting each
    /// quad's local indices.
    ///
    /// The naive chunk mesh uses `u16` indices, so it must stay below 16 384
    /// quads. M2's block meshlets keep that bound per block.
    pub fn from_quads(quads: &[Quad]) -> Self {
        let vertex_count = quads.len().checked_mul(4).expect("quad count overflow");
        assert!(
            vertex_count <= u16::MAX as usize + 1,
            "naive chunk mesh exceeds the u16 index range; split into meshlets"
        );
        let mut mesh = Self {
            vertices: Vec::with_capacity(vertex_count),
            indices: Vec::with_capacity(quads.len() * 6),
        };
        for (quad, source) in quads.iter().enumerate() {
            let base = (quad * 4) as u16;
            mesh.vertices.extend_from_slice(&source.vertices);
            mesh.indices.extend(Quad::INDICES.map(|index| base + index));
        }
        mesh
    }

    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    pub fn byte_len(&self) -> usize {
        self.vertices.len() * std::mem::size_of::<Vertex>() + self.indices.len() * 2
    }
}

/// Meshes a chunk volume, treating everything outside it as air.
///
/// `volume` is a dense `CHUNK_DIM³` array in chunk-local order; material `0`
/// is air. An isolated solid voxel emits six quads, and a fully enclosed voxel
/// emits none.
pub fn naive_mesh(volume: &[u16]) -> Mesh {
    Mesh::from_quads(&naive_quads_with_outside(volume, 0))
}

/// Like [`naive_mesh`], but lets callers choose the material outside the
/// volume. Pass a solid id to model an enclosed sub-volume.
pub fn naive_mesh_with_outside(volume: &[u16], outside: u16) -> Mesh {
    Mesh::from_quads(&naive_quads_with_outside(volume, outside))
}

/// Emits one quad per exposed voxel face.
pub fn naive_quads_with_outside(volume: &[u16], outside: u16) -> Vec<Quad> {
    assert_eq!(
        volume.len(),
        CHUNK_VOLUME,
        "volume must contain exactly CHUNK_VOLUME voxels"
    );
    let mut quads = Vec::new();
    let outside_solid = outside != 0;

    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let material = volume[chunk_index(x, y, z)];
                if material == 0 {
                    continue;
                }
                debug_assert!(material <= 255, "material id must fit the atlas");
                for face in Face::ALL {
                    if neighbor_is_solid(volume, x, y, z, face, outside_solid) {
                        continue;
                    }
                    let corners = face_corners(face, x, y, z);
                    let vertices = corners.map(|local| Vertex {
                        local,
                        material,
                        face: mesh_face(face),
                        ao: [u8::MAX; 4],
                        reserved: [0; 3],
                    });
                    quads.push(Quad {
                        vertices,
                        face,
                        material,
                    });
                }
            }
        }
    }
    quads
}

/// Mesh face code from ADR-010: `0..5 = +X, -X, +Y, -Y, +Z, -Z`.
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

fn chunk_index(x: usize, y: usize, z: usize) -> usize {
    debug_assert!(x < CHUNK_DIM && y < CHUNK_DIM && z < CHUNK_DIM);
    x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS))
}

fn neighbor_is_solid(
    volume: &[u16],
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    outside: bool,
) -> bool {
    let (dx, dy, dz) = face.normal();
    let nx = x as isize + dx as isize;
    let ny = y as isize + dy as isize;
    let nz = z as isize + dz as isize;
    if nx < 0
        || ny < 0
        || nz < 0
        || nx >= CHUNK_DIM as isize
        || ny >= CHUNK_DIM as isize
        || nz >= CHUNK_DIM as isize
    {
        return outside;
    }
    volume[chunk_index(nx as usize, ny as usize, nz as usize)] != 0
}

/// Four corners in counter-clockwise order seen from outside the face.
fn face_corners(face: Face, x: usize, y: usize, z: usize) -> [[u16; 3]; 4] {
    let x = x as u16;
    let y = y as u16;
    let z = z as u16;
    match face {
        Face::East => [
            [x + 1, y, z],
            [x + 1, y + 1, z],
            [x + 1, y + 1, z + 1],
            [x + 1, y, z + 1],
        ],
        Face::West => [[x, y, z], [x, y, z + 1], [x, y + 1, z + 1], [x, y + 1, z]],
        Face::Top => [
            [x, y + 1, z],
            [x, y + 1, z + 1],
            [x + 1, y + 1, z + 1],
            [x + 1, y + 1, z],
        ],
        Face::Bottom => [[x, y, z], [x + 1, y, z], [x + 1, y, z + 1], [x, y, z + 1]],
        Face::South => [
            [x, y, z + 1],
            [x + 1, y, z + 1],
            [x + 1, y + 1, z + 1],
            [x, y + 1, z + 1],
        ],
        Face::North => [[x, y, z], [x, y + 1, z], [x + 1, y + 1, z], [x + 1, y, z]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: (usize, usize, usize) = (8, 9, 10);

    fn empty_volume() -> Vec<u16> {
        vec![0; CHUNK_VOLUME]
    }

    fn single_voxel() -> Vec<u16> {
        let mut volume = empty_volume();
        let (x, y, z) = CENTER;
        volume[chunk_index(x, y, z)] = 1;
        volume
    }

    #[test]
    fn naive_mesh_emits_six_quads_for_one_isolated_voxel() {
        let mesh = naive_mesh(&single_voxel());

        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.byte_len(), 24 * 16 + 36 * 2);
    }

    #[test]
    fn naive_mesh_emits_nothing_for_an_enclosed_voxel() {
        // A fully solid volume with solid outside models an enclosed
        // sub-volume: no face is exposed and no quad is emitted.
        let volume = vec![1u16; CHUNK_VOLUME];
        let mesh = naive_mesh_with_outside(&volume, 1);

        assert_eq!(mesh.quad_count(), 0);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.indices.is_empty());
    }

    #[test]
    fn adjacent_voxels_remove_the_shared_pair() {
        let mut volume = empty_volume();
        let (x, y, z) = CENTER;
        volume[chunk_index(x, y, z)] = 1;
        volume[chunk_index(x + 1, y, z)] = 1;

        assert_eq!(naive_mesh(&volume).quad_count(), 10);
    }

    #[test]
    fn isolated_voxel_has_one_quad_per_face() {
        let quads = naive_quads_with_outside(&single_voxel(), 0);
        for face in Face::ALL {
            assert_eq!(
                quads.iter().filter(|quad| quad.face == face).count(),
                1,
                "{face} quad count"
            );
        }
    }

    #[test]
    fn quad_winding_points_outward() {
        let quads = naive_quads_with_outside(&single_voxel(), 0);
        for quad in quads {
            let a = quad.vertices[0].local.map(i64::from);
            let b = quad.vertices[1].local.map(i64::from);
            let c = quad.vertices[2].local.map(i64::from);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let expected = face_normal_i64(quad.face);
            assert_eq!(cross, expected, "winding for {}", quad.face);
        }
    }

    fn face_normal_i64(face: Face) -> [i64; 3] {
        let (x, y, z) = face.normal();
        [i64::from(x), i64::from(y), i64::from(z)]
    }

    #[test]
    fn vertex_layout_matches_adr_010() {
        assert_eq!(std::mem::size_of::<Vertex>(), 16);
        assert_eq!(std::mem::align_of::<Vertex>(), 4);

        let quad = naive_quads_with_outside(&single_voxel(), 0)[0];
        assert_eq!(quad.vertices[0].face, mesh_face(quad.face));
        assert!(quad.vertices[0].face <= 5);
        assert_eq!(quad.vertices[0].ao, [255; 4]);
        assert_eq!(quad.vertices[0].reserved, [0; 3]);
        assert!(quad.vertices.iter().all(|vertex| vertex.material == 1));
        assert!(quad.vertices.iter().all(|vertex| {
            vertex
                .local
                .iter()
                .all(|value| *value as usize <= CHUNK_DIM)
        }));
    }

    #[test]
    fn mesh_indices_offset_each_quad() {
        let mesh = naive_mesh(&single_voxel());
        assert_eq!(&mesh.indices[..6], &Quad::INDICES[..]);
        assert_eq!(&mesh.indices[6..12], &[4, 5, 6, 4, 6, 7]);
        assert!(
            mesh.indices
                .iter()
                .all(|index| (*index as usize) < mesh.vertices.len())
        );
    }
}
