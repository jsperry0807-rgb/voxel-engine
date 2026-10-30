//! Greedy block-face meshing (M2-04).
//!
//! The driver in [`crate`] supplies one `18³` [`BlockScratch`] at a time. This
//! module turns the block's 16³ interior into axis-aligned rectangles, merging
//! equal-material exposed faces inside the block only. It is deliberately the
//! count/rectangle core, not the final vertex emitter: AO (M2-07/M2-08) and the
//! 16-byte vertex packing (M2-10) sit on top of these rectangles.
//!
//! Backface culling happens before the mask is built: a face whose outward
//! neighbour is solid is never emitted, so interior faces and the inward half
//! of a solid boundary cannot reach the quad list (M2-06).
//!
//! The scan order matches the M1-31 reference exactly: face, then layer along
//! the normal, then `v`, then `u`; each rectangle grows in `u` first and `v`
//! second. That keeps the quad count deterministic where more than one maximal
//! cover exists.

use voxel_core::Face;

use crate::{BLOCK_EDGE, BlockScratch};

/// The merge key for one exposed voxel face: material, orientation and the four
/// corner AO values. Two adjacent mask cells merge only when the whole key is
/// equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeKey {
    /// Outward face orientation.
    pub face: Face,
    /// Block material id from the chunk volume (resolved to an atlas slot
    /// before upload).
    pub material: u16,
    /// Four corner AO values in `0..=255`.
    pub ao: [u8; 4],
}

impl MergeKey {
    /// Builds a merge key.
    pub const fn new(face: Face, material: u16, ao: [u8; 4]) -> Self {
        Self { face, material, ao }
    }
}

/// One greedy rectangle inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GreedyQuad {
    /// Outward face orientation.
    pub face: Face,
    /// Block material id of the exposed faces.
    pub material: u16,
    /// Four corner AO values copied from the merge key.
    pub ao: [u8; 4],
    /// Scratch-local coordinate of the rectangle's minimum corner.
    pub origin: [u32; 3],
    /// Rectangle extent; the normal axis is always `1`.
    pub extent: [u32; 3],
}

impl GreedyQuad {
    /// Number of exposed voxel faces covered by the rectangle.
    pub fn area(&self) -> u32 {
        self.extent[0] * self.extent[1] * self.extent[2]
    }
}

/// Emits the greedy rectangles for one block scratch.
///
/// `out` is cleared first and reused across blocks so the caller can keep one
/// vector per worker. Every rectangle stays inside the 16³ block interior.
pub fn greedy_block(scratch: &BlockScratch, out: &mut Vec<GreedyQuad>) {
    greedy_block_with_ao(scratch, out, corner_ao);
}

/// Like [`greedy_block`], but with a caller-supplied AO sampler.
///
/// Passing `|_, _, _, _| u8::MAX` gives the AO-off merge for the M2-08
/// comparison; the normal path uses [`corner_ao`].
pub fn greedy_block_with_ao<F>(scratch: &BlockScratch, out: &mut Vec<GreedyQuad>, mut sample_ao: F)
where
    F: FnMut(&BlockScratch, Face, [u32; 3], usize) -> u8,
{
    out.clear();
    let mut mask: [Option<MergeKey>; BLOCK_EDGE * BLOCK_EDGE] = [None; BLOCK_EDGE * BLOCK_EDGE];

    for face in Face::ALL {
        let (axis, sign) = face_axis(face);
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;

        for layer in 0..BLOCK_EDGE {
            mask.fill(None);
            for v in 0..BLOCK_EDGE {
                for u in 0..BLOCK_EDGE {
                    let mut cell = [0u32; 3];
                    cell[axis] = 1 + layer as u32;
                    cell[u_axis] = 1 + u as u32;
                    cell[v_axis] = 1 + v as u32;

                    let material = scratch.get(cell[0], cell[1], cell[2]);
                    if material == 0 {
                        continue;
                    }

                    let mut neighbour = cell;
                    neighbour[axis] = (neighbour[axis] as i32 + sign) as u32;
                    if scratch.get(neighbour[0], neighbour[1], neighbour[2]) == 0 {
                        let ao =
                            std::array::from_fn(|corner| sample_ao(scratch, face, cell, corner));
                        mask[v * BLOCK_EDGE + u] = Some(MergeKey::new(face, material, ao));
                    }
                }
            }

            greedy_mask(&mut mask, layer, axis, u_axis, v_axis, out);
        }
    }
}

fn greedy_mask(
    mask: &mut [Option<MergeKey>; BLOCK_EDGE * BLOCK_EDGE],
    layer: usize,
    axis: usize,
    u_axis: usize,
    v_axis: usize,
    out: &mut Vec<GreedyQuad>,
) {
    for v in 0..BLOCK_EDGE {
        for u in 0..BLOCK_EDGE {
            let index = v * BLOCK_EDGE + u;
            let Some(key) = mask[index] else {
                continue;
            };

            let mut width = 1;
            while u + width < BLOCK_EDGE && mask[index + width] == Some(key) {
                width += 1;
            }

            let mut height = 1;
            'grow: while v + height < BLOCK_EDGE {
                let row = (v + height) * BLOCK_EDGE + u;
                for offset in 0..width {
                    if mask[row + offset] != Some(key) {
                        break 'grow;
                    }
                }
                height += 1;
            }

            for row in 0..height {
                let start = (v + row) * BLOCK_EDGE + u;
                mask[start..start + width].fill(None);
            }

            let mut origin = [0u32; 3];
            origin[axis] = 1 + layer as u32;
            origin[u_axis] = 1 + u as u32;
            origin[v_axis] = 1 + v as u32;
            let mut extent = [1u32; 3];
            extent[u_axis] = width as u32;
            extent[v_axis] = height as u32;

            out.push(GreedyQuad {
                face: key.face,
                material: key.material,
                ao: key.ao,
                origin,
                extent,
            });
        }
    }
}

pub(crate) fn face_axis(face: Face) -> (usize, i32) {
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

/// Four AO levels: fully occluded, one edge, two edges, fully open.
const AO_LEVELS: [u8; 4] = [0, 85, 170, 255];

/// The `(u, v)` signs of a face's four quad corners, matching the vertex order
/// used by the M1.5 mesher (`voxel_viewer::naive_mesh::face_corners`).
///
/// The order is counter-clockwise seen from outside the face, which is what the
/// renderer's back-face culling expects (`cull_mode: Back`, CCW front). The
/// vertex emitter and the AO sampler both index this table, so a wrong entry
/// would flip a face inward and move its occlusion at the same time.
pub(crate) fn corner_signs(face: Face) -> [(i32, i32); 4] {
    match face {
        Face::West | Face::Bottom | Face::North => [(-1, -1), (-1, 1), (1, 1), (1, -1)],
        Face::East | Face::Top | Face::South => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
    }
}

/// Samples one vertex AO value from the `18³` scratch.
///
/// `cell` is the scratch-local coordinate of the exposed face's voxel (`1..=16`
/// on the block interior). The three samples are the two edge neighbours and the
/// diagonal in the plane one voxel outward; two solid edges mean full occlusion,
/// otherwise the count of solid samples reduces the four-level value. The
/// result is in [`AO_LEVELS`], with `255` fully open.
pub fn corner_ao(scratch: &BlockScratch, face: Face, cell: [u32; 3], corner: usize) -> u8 {
    let (axis, sign) = face_axis(face);
    let u_axis = (axis + 1) % 3;
    let v_axis = (axis + 2) % 3;
    let (du, dv) = corner_signs(face)[corner];
    let outer = (cell[axis] as i32 + sign) as u32;
    let u = cell[u_axis] as i32 + du;
    let v = cell[v_axis] as i32 + dv;

    let sample = |u: i32, v: i32| {
        let mut sample = [0u32; 3];
        sample[axis] = outer;
        sample[u_axis] = u as u32;
        sample[v_axis] = v as u32;
        scratch.get(sample[0], sample[1], sample[2])
    };

    let side_u = sample(u, cell[v_axis] as i32) != 0;
    let side_v = sample(cell[u_axis] as i32, v) != 0;
    let diagonal = sample(u, v) != 0;
    let level = if side_u && side_v {
        0
    } else {
        3 - usize::from(side_u) - usize::from(side_v) - usize::from(diagonal)
    };
    AO_LEVELS[level]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask_with(keys: &[(usize, MergeKey)]) -> [Option<MergeKey>; BLOCK_EDGE * BLOCK_EDGE] {
        let mut mask = [None; BLOCK_EDGE * BLOCK_EDGE];
        for (index, key) in keys {
            mask[*index] = Some(*key);
        }
        mask
    }

    #[test]
    fn merge_key_splits_on_any_difference() {
        let key = MergeKey::new(Face::Top, 3, [255; 4]);
        let mut out = Vec::new();

        // Same material, orientation and AO: one 2x1 rectangle.
        let mut mask = mask_with(&[(0, key), (1, key)]);
        greedy_mask(&mut mask, 0, 1, 2, 0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].area(), 2);
        assert_eq!(out[0].ao, [255; 4]);

        // AO differs on one corner: split.
        out.clear();
        let mut mask = mask_with(&[
            (0, key),
            (1, MergeKey::new(Face::Top, 3, [0, 255, 255, 255])),
        ]);
        greedy_mask(&mut mask, 0, 1, 2, 0, &mut out);
        assert_eq!(out.len(), 2);

        // Material differs: split.
        out.clear();
        let mut mask = mask_with(&[(0, key), (1, MergeKey::new(Face::Top, 4, [255; 4]))]);
        greedy_mask(&mut mask, 0, 1, 2, 0, &mut out);
        assert_eq!(out.len(), 2);
    }
}
