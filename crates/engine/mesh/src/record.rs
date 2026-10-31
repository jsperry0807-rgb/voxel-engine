//! Packed 8 B quad records for the instanced-quad spike (M3-00 Phase 1).
//!
//! One record describes one greedy rectangle: a chunk-local origin, a 14-bit
//! material, a face code, two 7-bit extents (stored as `size - 1`) and four
//! 2-bit AO levels. The record is two `u32` words so a WGSL storage buffer can
//! read it as `array<vec2<u32>>` or a two-word struct.
//!
//! The encoding is lossless for everything the mesher emits: positions are
//! `0..=CHUNK_DIM`, extents are `1..=16` today (7 bits allow 128), AO is exactly
//! `[0, 85, 170, 255]`, and material is at most 14 bits. [`QuadRecord::vertices`]
//! reconstructs the same four [`Vertex`] records [`mesh_block`] emits, which is
//! the Gate A exactness check.

use crate::BlockPos;
use crate::greedy::{GreedyQuad, face_axis};
use crate::mesh::{face_from_code, mesh_block, mesh_face};
use crate::vertex::Vertex;
use voxel_core::constants::CHUNK_DIM;

/// One packed quad record, exactly 8 B.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadRecord {
    words: [u32; 2],
}

impl QuadRecord {
    /// Encoded size in bytes.
    pub const SIZE: usize = 8;
    /// Encoded alignment in bytes.
    pub const ALIGN: usize = 4;

    /// Builds a record from its two words.
    pub const fn from_words(words: [u32; 2]) -> Self {
        Self { words }
    }

    /// The two words, in storage-buffer order.
    pub const fn words(self) -> [u32; 2] {
        self.words
    }

    /// Packs one greedy rectangle from `block`.
    ///
    /// # Panics
    ///
    /// Panics if the origin leaves the chunk, an extent is empty or above 128,
    /// the material exceeds 14 bits, or an AO value is not one of the four
    /// canonical levels.
    pub fn from_quad(quad: &GreedyQuad, block: BlockPos) -> Self {
        let block_origin = block.origin();
        let mut origin = [0u8; 3];
        for axis in 0..3 {
            let local = block_origin[axis] + quad.origin[axis] - 1;
            assert!(
                local <= CHUNK_DIM as u32,
                "quad origin {local} is outside the chunk"
            );
            origin[axis] = local as u8;
        }

        let (axis, _) = face_axis(quad.face);
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        let extent_u = quad.extent[u_axis];
        let extent_v = quad.extent[v_axis];
        assert!(
            (1..=128).contains(&extent_u) && (1..=128).contains(&extent_v),
            "quad extent ({extent_u}, {extent_v}) is outside 1..=128"
        );

        assert!(
            quad.material <= 0x3FFF,
            "material {} exceeds the 14-bit record field",
            quad.material
        );
        let mut ao = 0u32;
        for (corner, value) in quad.ao.iter().enumerate() {
            let level = match value {
                0 => 0,
                85 => 1,
                170 => 2,
                255 => 3,
                other => panic!("AO value {other} is not a canonical level"),
            };
            ao |= (level as u32) << (17 + 2 * corner);
        }

        let face = mesh_face(quad.face) as u32;
        let packed = face
            | ((extent_u - 1) << 3)
            | ((extent_v - 1) << 10)
            | ao
            | (((quad.material >> 8) as u32) << 26);
        let origin_word = u32::from(origin[0])
            | (u32::from(origin[1]) << 8)
            | (u32::from(origin[2]) << 16)
            | (u32::from(quad.material & 0xFF) << 24);
        Self {
            words: [origin_word, packed],
        }
    }

    /// Chunk-local origin of the rectangle's minimum corner.
    pub const fn origin(self) -> [u8; 3] {
        [
            (self.words[0] & 0xFF) as u8,
            ((self.words[0] >> 8) & 0xFF) as u8,
            ((self.words[0] >> 16) & 0xFF) as u8,
        ]
    }

    /// 14-bit material.
    pub const fn material(self) -> u16 {
        ((self.words[0] >> 24) as u16) | (((self.words[1] >> 26) as u16) << 8)
    }

    /// Face code, `0..5`.
    pub const fn face(self) -> u8 {
        (self.words[1] & 7) as u8
    }

    /// The two extents, in the face's `(u, v)` order.
    pub const fn extents(self) -> (u8, u8) {
        (
            (((self.words[1] >> 3) & 0x7F) + 1) as u8,
            (((self.words[1] >> 10) & 0x7F) + 1) as u8,
        )
    }

    /// Four AO levels, `0..=3`.
    pub const fn ao_levels(self) -> [u8; 4] {
        [
            ((self.words[1] >> 17) & 3) as u8,
            ((self.words[1] >> 19) & 3) as u8,
            ((self.words[1] >> 21) & 3) as u8,
            ((self.words[1] >> 23) & 3) as u8,
        ]
    }

    /// The block containing this record.
    pub fn block(self) -> BlockPos {
        let origin = self.origin();
        BlockPos::new(
            u32::from(origin[0]) / 16,
            u32::from(origin[1]) / 16,
            u32::from(origin[2]) / 16,
        )
    }

    /// Decodes into the scratch-local greedy rectangle `mesh_block` consumes.
    pub fn to_quad(self) -> GreedyQuad {
        let face = face_from_code(self.face());
        let (axis, _) = face_axis(face);
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        let (extent_u, extent_v) = self.extents();
        let origin = self.origin();
        let block = self.block().origin();
        let mut quad_origin = [0u32; 3];
        let mut extent = [1u32; 3];
        for coordinate in 0..3 {
            quad_origin[coordinate] = u32::from(origin[coordinate]) - block[coordinate] + 1;
        }
        extent[u_axis] = u32::from(extent_u);
        extent[v_axis] = u32::from(extent_v);
        GreedyQuad {
            face,
            material: self.material(),
            ao: self.ao_levels().map(|level| level * 85),
            origin: quad_origin,
            extent,
        }
    }

    /// Reconstructs the four vertices `mesh_block` would emit for this record.
    pub fn vertices(self) -> [Vertex; 4] {
        let mesh = mesh_block(&[self.to_quad()], self.block());
        mesh.vertices
            .try_into()
            .expect("one quad always emits exactly four vertices")
    }

    /// Encodes to the fixed 8 B little-endian layout.
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..4].copy_from_slice(&self.words[0].to_le_bytes());
        bytes[4..8].copy_from_slice(&self.words[1].to_le_bytes());
        bytes
    }

    /// Decodes the fixed 8 B little-endian layout.
    pub fn decode(bytes: [u8; Self::SIZE]) -> Self {
        Self {
            words: [
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ],
        }
    }
}

const _: () = assert!(QuadRecord::SIZE == 8);
const _: () = assert!(QuadRecord::ALIGN == 4);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::for_each_block;
    use crate::greedy::greedy_block;
    use voxel_core::constants::CHUNK_VOLUME;
    use voxel_storage::Apron;

    const FACES: [voxel_core::Face; 6] = [
        voxel_core::Face::East,
        voxel_core::Face::West,
        voxel_core::Face::Top,
        voxel_core::Face::Bottom,
        voxel_core::Face::South,
        voxel_core::Face::North,
    ];

    fn quad(face: voxel_core::Face, extent: (u32, u32), ao: [u8; 4]) -> GreedyQuad {
        let (axis, _) = face_axis(face);
        let u_axis = (axis + 1) % 3;
        let v_axis = (axis + 2) % 3;
        let mut quad = GreedyQuad {
            face,
            material: 0x2A,
            ao,
            origin: [1, 1, 1],
            extent: [1, 1, 1],
        };
        quad.extent[u_axis] = extent.0;
        quad.extent[v_axis] = extent.1;
        quad
    }

    #[test]
    fn record_covers_every_face_extent_and_ao_level() {
        let levels = [0u8, 1, 2, 3].map(|level| level * 85);
        for face in FACES {
            for extent_u in 1..=16 {
                for extent_v in 1..=16 {
                    for ao in [levels, [levels[3]; 4], [levels[0]; 4]] {
                        let original = quad(face, (extent_u, extent_v), ao);
                        let block = BlockPos::new(0, 0, 0);
                        let record = QuadRecord::from_quad(&original, block);
                        assert_eq!(record.to_quad(), original);
                        assert_eq!(record.face(), mesh_face(face));
                        assert_eq!(record.extents(), (extent_u as u8, extent_v as u8));
                        assert_eq!(record.ao_levels(), ao.map(|value| value / 85));
                        assert_eq!(record.material(), 0x2A);
                        let mesh = mesh_block(&[original], block);
                        assert_eq!(record.vertices(), mesh.vertices.as_slice());
                        assert_eq!(QuadRecord::decode(record.encode()), record);
                    }
                }
            }
        }
    }

    #[test]
    fn record_matches_every_quad_of_a_mixed_chunk() {
        let mut volume = vec![0u16; CHUNK_VOLUME];
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for material in volume.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *material = (state % 7) as u16;
        }
        let apron = Apron::new();
        let mut quads = Vec::new();
        let mut checked = 0usize;
        for_each_block(&volume, &apron, |block, scratch| {
            greedy_block(scratch, &mut quads);
            let mesh = mesh_block(&quads, block);
            for (index, original) in quads.iter().enumerate() {
                let record = QuadRecord::from_quad(original, block);
                assert_eq!(record.to_quad(), *original);
                let start = index * 4;
                assert_eq!(
                    record.vertices(),
                    mesh.vertices[start..start + 4],
                    "block {block:?} quad {index}"
                );
                checked += 1;
            }
        });
        assert!(checked > 10_000, "the mixed chunk must exercise many quads");
    }

    #[test]
    #[should_panic(expected = "14-bit")]
    fn record_rejects_a_material_above_14_bits() {
        let mut original = quad(voxel_core::Face::East, (1, 1), [255; 4]);
        original.material = 0x4000;
        let _ = QuadRecord::from_quad(&original, BlockPos::new(0, 0, 0));
    }
}
