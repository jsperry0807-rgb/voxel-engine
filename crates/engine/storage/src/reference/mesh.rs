//! Count-only greedy mesher reference (M1-31).
//!
//! This is the slow, exact oracle for M2's production mesher. It scans a dense
//! voxel scene, counts the exposed opaque faces, merges same-material faces into
//! axis-aligned rectangles, and reports the result without building a vertex or
//! index buffer. Nothing here is written into GPU memory: the mesher only
//! accumulates quads, covered voxel faces and the byte cost of the format fixed
//! by ADR-010.
//!
//! # What is an exposed face?
//!
//! A non-air voxel face is exposed when the voxel on its outward side is air or
//! outside the scene. Any non-zero material is treated as solid; this reference
//! does not model transparency because water is a separate renderer at launch
//! (`docs/voxel-engine-architecture-spec.md` §14.2). Faces between two different
//! solid materials are interior and are not emitted.
//!
//! # Merge rule
//!
//! Quads are merged **inside one 16³ block** (ADR-010), never across a block
//! boundary. Within a block face, a quad may cover up to `16 × 16` voxel faces,
//! but its merge key is only the material of the exposed face. AO is not part of
//! this M1 reference; M2-05 extends the key to `(material, face, 4 × AO)`.
//!
//! For each block, face and layer the reference builds a small material mask.
//! It scans the two face axes in a fixed order, grows the rectangle along the
//! first axis and then the second, and clears the covered cells. The scan order
//! is part of the contract, so the quad count is deterministic even where more
//! than one maximal cover exists.
//!
//! # Orientation
//!
//! [`voxel_core::Face`] supplies the outward normal. The face axis is the normal
//! axis; the two merge axes are the remaining axes in cyclic `x → y → z` order.
//! The direction (positive or negative) selects which voxel is solid and which
//! is air, so every quad faces outward. Counts are kept per face orientation.
//!
//! # Bytes
//!
//! A quad owns four 16 B vertices and six `u16` indices: **76 B** per quad. The
//! reference counts that payload only. Meshlet records and the per-block record
//! table are chunk metadata and are not part of the per-quad byte comparison.

use voxel_core::Face;
use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM};

use crate::dag::Svdag;

/// Size of one vertex in the ADR-010 format.
pub const VERTEX_BYTES: usize = 16;

/// Vertices owned by one quad.
pub const VERTICES_PER_QUAD: usize = 4;

/// `u16` indices per quad.
pub const INDICES_PER_QUAD: usize = 6;

/// Size of one index.
pub const INDEX_BYTES: usize = 2;

/// Payload bytes for one indexed quad: four vertices plus six `u16` indices.
pub const QUAD_BYTES: usize = VERTEX_BYTES * VERTICES_PER_QUAD + INDEX_BYTES * INDICES_PER_QUAD;

/// Brute-force exposed-face counts, split by orientation.
///
/// This is the per-face oracle: it counts each exposed voxel face independently,
/// with no merging at all. Compare it with [`MeshCounts::covered_faces_for`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaceCounts {
    faces: [usize; 6],
}

impl FaceCounts {
    /// Exposed faces with the given outward orientation.
    pub const fn get(&self, face: Face) -> usize {
        self.faces[face.index()]
    }

    /// All exposed faces, summed over the six orientations.
    pub fn total(&self) -> usize {
        self.faces.iter().sum()
    }

    /// Iterates `(face, count)` in [`Face::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Face, usize)> + '_ {
        Face::ALL.into_iter().map(|face| (face, self.get(face)))
    }

    fn add(&mut self, face: Face) {
        self.faces[face.index()] += 1;
    }
}

/// Greedy-mesh counts for one scene.
///
/// `quads` is the number of emitted rectangles and `covered_faces` is the number
/// of exposed voxel faces those rectangles cover. Both are split by orientation
/// so a face-orientation bug cannot hide in the total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeshCounts {
    quads_by_face: [usize; 6],
    faces_by_face: [usize; 6],
}

impl MeshCounts {
    /// Empty counts: no quads and no exposed faces.
    pub const EMPTY: Self = Self {
        quads_by_face: [0; 6],
        faces_by_face: [0; 6],
    };

    /// Quads with the given outward orientation.
    pub const fn quads_for(&self, face: Face) -> usize {
        self.quads_by_face[face.index()]
    }

    /// Exposed voxel faces covered by quads of the given orientation.
    pub const fn covered_faces_for(&self, face: Face) -> usize {
        self.faces_by_face[face.index()]
    }

    /// Total quads, summed over the six orientations.
    pub fn quads(&self) -> usize {
        self.quads_by_face.iter().sum()
    }

    /// Total exposed voxel faces covered by the quads.
    pub fn covered_faces(&self) -> usize {
        self.faces_by_face.iter().sum()
    }

    /// Vertex bytes: four 16 B vertices per quad.
    pub fn vertex_bytes(&self) -> usize {
        self.quads() * VERTEX_BYTES * VERTICES_PER_QUAD
    }

    /// Index bytes: six `u16` indices per quad.
    pub fn index_bytes(&self) -> usize {
        self.quads() * INDICES_PER_QUAD * INDEX_BYTES
    }

    /// Total mesh payload bytes (`vertex_bytes + index_bytes`).
    pub fn bytes(&self) -> usize {
        self.quads() * QUAD_BYTES
    }

    /// Quads per exposed voxel face.
    ///
    /// `1.0` means no merging happened; lower is better. An empty scene has no
    /// faces to merge and is defined as `1.0`.
    pub fn merge_ratio(&self) -> f64 {
        let faces = self.covered_faces();
        if faces == 0 {
            1.0
        } else {
            self.quads() as f64 / faces as f64
        }
    }

    /// Fraction of exposed faces removed by merging, in `0.0 ..= 1.0`.
    pub fn merge_saving(&self) -> f64 {
        1.0 - self.merge_ratio()
    }

    /// Iterates `(face, quads, covered_faces)` in [`Face::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Face, usize, usize)> + '_ {
        Face::ALL
            .into_iter()
            .map(|face| (face, self.quads_for(face), self.covered_faces_for(face)))
    }

    fn add_quad(&mut self, face: Face, area: usize) {
        self.quads_by_face[face.index()] += 1;
        self.faces_by_face[face.index()] += area;
    }
}

/// Counts the greedy mesh of a scene through a material lookup.
///
/// `edge` is the scene edge in voxels. Blocks are `BLOCK_DIM`³, so a full chunk
/// passes [`CHUNK_DIM`]; a partial block at the scene edge is meshed with its
/// actual size. `material_at(x, y, z)` returns a material id, where `0` is air.
/// Coordinates outside the scene are air.
///
/// The lookup is called only for in-scene voxels and their in-scene neighbours.
/// Quads never cross a block boundary (ADR-010).
pub fn count_greedy<F>(edge: usize, mut material_at: F) -> MeshCounts
where
    F: FnMut(u32, u32, u32) -> u16,
{
    if edge == 0 {
        return MeshCounts::EMPTY;
    }

    let blocks = edge.div_ceil(BLOCK_DIM);
    let mut counts = MeshCounts::EMPTY;
    // One block face is at most 16 × 16 voxel faces.
    let mut mask = [0u16; BLOCK_DIM * BLOCK_DIM];

    for bz in 0..blocks {
        for by in 0..blocks {
            for bx in 0..blocks {
                let base = [bx * BLOCK_DIM, by * BLOCK_DIM, bz * BLOCK_DIM];
                let dims = [
                    (edge - base[0]).min(BLOCK_DIM),
                    (edge - base[1]).min(BLOCK_DIM),
                    (edge - base[2]).min(BLOCK_DIM),
                ];

                if !block_has_solid(&mut material_at, base, dims) {
                    continue;
                }

                for face in Face::ALL {
                    let (axis, sign) = face_axis(face);
                    let u_axis = (axis + 1) % 3;
                    let v_axis = (axis + 2) % 3;
                    let du = dims[u_axis];
                    let dv = dims[v_axis];

                    for layer in 0..dims[axis] {
                        mask.fill(0);

                        for v in 0..dv {
                            for u in 0..du {
                                let mut cell = base;
                                cell[axis] += layer;
                                cell[u_axis] += u;
                                cell[v_axis] += v;

                                let material =
                                    sample(&mut material_at, edge, cell.map(|cell| cell as i32));
                                if material == 0 {
                                    continue;
                                }

                                let mut neighbour = cell.map(|coordinate| coordinate as i32);
                                neighbour[axis] += sign;
                                if sample(&mut material_at, edge, neighbour) == 0 {
                                    mask[v * du + u] = material;
                                }
                            }
                        }

                        greedy_mask(&mut mask, du, dv, face, &mut counts);
                    }
                }
            }
        }
    }

    counts
}

/// Counts the greedy mesh of a dense scene.
///
/// The buffer is `x + y · edge + z · edge²`, matching [`crate::local_index`] when
/// `edge == CHUNK_DIM`.
///
/// # Panics
///
/// Panics if `materials.len() != edge³`.
pub fn count_greedy_dense(edge: usize, materials: &[u16]) -> MeshCounts {
    check_dense_len(edge, materials.len());
    count_greedy(edge, |x, y, z| materials[dense_index(edge, x, y, z)])
}

/// Counts the greedy mesh of a compiled chunk.
pub fn count_greedy_svdag(chunk: &Svdag) -> MeshCounts {
    count_greedy(CHUNK_DIM, |x, y, z| chunk.get(x, y, z))
}

/// Counts exposed opaque faces independently, with no merging.
///
/// This is the brute-force per-face oracle. `edge` and `material_at` follow
/// [`count_greedy`]; coordinates outside the scene are air.
pub fn count_exposed_faces<F>(edge: usize, mut material_at: F) -> FaceCounts
where
    F: FnMut(u32, u32, u32) -> u16,
{
    let mut counts = FaceCounts::default();
    if edge == 0 {
        return counts;
    }

    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                if material_at(x as u32, y as u32, z as u32) == 0 {
                    continue;
                }
                let cell = [x as i32, y as i32, z as i32];
                for face in Face::ALL {
                    let (axis, sign) = face_axis(face);
                    let mut neighbour = cell;
                    neighbour[axis] += sign;
                    if sample(&mut material_at, edge, neighbour) == 0 {
                        counts.add(face);
                    }
                }
            }
        }
    }

    counts
}

/// Counts exposed opaque faces of a dense scene, with no merging.
///
/// # Panics
///
/// Panics if `materials.len() != edge³`.
pub fn count_exposed_faces_dense(edge: usize, materials: &[u16]) -> FaceCounts {
    check_dense_len(edge, materials.len());
    count_exposed_faces(edge, |x, y, z| materials[dense_index(edge, x, y, z)])
}

/// Counts exposed opaque faces of a compiled chunk, with no merging.
pub fn count_exposed_faces_svdag(chunk: &Svdag) -> FaceCounts {
    count_exposed_faces(CHUNK_DIM, |x, y, z| chunk.get(x, y, z))
}

/// Greedily covers one block-face mask and adds the quads to `counts`.
///
/// The mask is `[v][u]`, row-major with `u` fastest. A non-zero value is the
/// material of one exposed face. The scan grows each rectangle in `u` first and
/// `v` second, then clears the covered cells. `du × dv` may be smaller than
/// `BLOCK_DIM × BLOCK_DIM` for a partial block at the scene edge.
fn greedy_mask(
    mask: &mut [u16; BLOCK_DIM * BLOCK_DIM],
    du: usize,
    dv: usize,
    face: Face,
    counts: &mut MeshCounts,
) {
    for v in 0..dv {
        for u in 0..du {
            let index = v * du + u;
            let material = mask[index];
            if material == 0 {
                continue;
            }

            let mut width = 1;
            while u + width < du && mask[index + width] == material {
                width += 1;
            }

            let mut height = 1;
            'grow: while v + height < dv {
                let row = (v + height) * du + u;
                for offset in 0..width {
                    if mask[row + offset] != material {
                        break 'grow;
                    }
                }
                height += 1;
            }

            for row in 0..height {
                let start = (v + row) * du + u;
                mask[start..start + width].fill(0);
            }

            counts.add_quad(face, width * height);
        }
    }
}

/// Returns true when a block contains at least one non-air voxel.
///
/// Empty blocks are skipped before the per-face mask pass. This matters at the
/// pre-registered 128³ edge, where a sparse scene has many more empty blocks
/// than non-empty ones.
fn block_has_solid<F>(material_at: &mut F, base: [usize; 3], dims: [usize; 3]) -> bool
where
    F: FnMut(u32, u32, u32) -> u16,
{
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                if material_at(
                    (base[0] + x) as u32,
                    (base[1] + y) as u32,
                    (base[2] + z) as u32,
                ) != 0
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Returns the outward axis and sign of a face.
fn face_axis(face: Face) -> (usize, i32) {
    let (x, y, z) = face.normal();
    if x != 0 {
        (0, x)
    } else if y != 0 {
        (1, y)
    } else {
        debug_assert_ne!(z, 0, "a face normal must use one axis");
        (2, z)
    }
}

/// Reads a voxel, treating coordinates outside the scene as air.
fn sample<F>(material_at: &mut F, edge: usize, [x, y, z]: [i32; 3]) -> u16
where
    F: FnMut(u32, u32, u32) -> u16,
{
    let edge = edge as i32;
    if x < 0 || y < 0 || z < 0 || x >= edge || y >= edge || z >= edge {
        0
    } else {
        material_at(x as u32, y as u32, z as u32)
    }
}

fn dense_index(edge: usize, x: u32, y: u32, z: u32) -> usize {
    x as usize + y as usize * edge + z as usize * edge * edge
}

fn check_dense_len(edge: usize, len: usize) {
    assert_eq!(
        len,
        edge * edge * edge,
        "a dense scene of edge {edge} needs {edge}³ cells"
    );
}

// The quad byte cost is the frozen ADR-010 format, not an implementation
// choice: four 16 B vertices plus six u16 indices.
const _: () = {
    assert!(VERTEX_BYTES == 16);
    assert!(VERTICES_PER_QUAD == 4);
    assert!(INDICES_PER_QUAD == 6);
    assert!(INDEX_BYTES == 2);
    assert!(QUAD_BYTES == VERTEX_BYTES * VERTICES_PER_QUAD + INDEX_BYTES * INDICES_PER_QUAD);
    assert!(QUAD_BYTES == 76);
};
