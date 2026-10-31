//! The logical ADR-010 16 B mesh vertex and the M3 packed 8 B upload vertex.
//!
//! The layout is fixed: three chunk-local `u16` coordinates, a `u16` material,
//! the face code, four `u8` corner AO values and three reserved zero bytes.
//! There is no UV, normal, tangent, colour or per-vertex light field.
//!
//! The explicit byte codec is the contract used by the golden-mesh tests and by
//! the GPU upload path; `size_of` and `align_of` are asserted at compile time.

use std::mem::{align_of, size_of};

use voxel_core::constants::CHUNK_DIM;

/// One mesh vertex, exactly 16 B and four-byte aligned.
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    /// Chunk-local voxel coordinate, `0..=CHUNK_DIM`.
    pub local: [u16; 3],
    /// Block material id from the chunk volume; air is never emitted.
    ///
    /// ADR-010's meshlet stream carries an *atlas* id here. Call
    /// [`Mesh::resolve_materials`] with the registry before upload to convert
    /// this value per face (`MaterialRegistry::face_materials`); the `face`
    /// field selects the slot.
    pub material: u16,
    /// Mesh face code, `0..5` = `+X, -X, +Y, -Y, +Z, -Z`.
    pub face: u8,
    /// Four corner AO values, `0..=255`.
    pub ao: [u8; 4],
    /// Reserved; always zero.
    pub reserved: [u8; 3],
}

impl Vertex {
    /// Encoded size in bytes.
    pub const SIZE: usize = 16;
    /// Encoded alignment in bytes.
    pub const ALIGN: usize = 4;

    /// Builds a vertex with the reserved bytes zeroed.
    pub const fn new(local: [u16; 3], material: u16, face: u8, ao: [u8; 4]) -> Self {
        Self {
            local,
            material,
            face,
            ao,
            reserved: [0; 3],
        }
    }

    /// Encodes the fixed 16 B layout, little-endian.
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..2].copy_from_slice(&self.local[0].to_le_bytes());
        bytes[2..4].copy_from_slice(&self.local[1].to_le_bytes());
        bytes[4..6].copy_from_slice(&self.local[2].to_le_bytes());
        bytes[6..8].copy_from_slice(&self.material.to_le_bytes());
        bytes[8] = self.face;
        bytes[9..13].copy_from_slice(&self.ao);
        bytes[13..16].copy_from_slice(&self.reserved);
        bytes
    }

    /// Decodes the fixed 16 B layout, little-endian.
    pub fn decode(bytes: [u8; Self::SIZE]) -> Self {
        Self {
            local: [
                u16::from_le_bytes([bytes[0], bytes[1]]),
                u16::from_le_bytes([bytes[2], bytes[3]]),
                u16::from_le_bytes([bytes[4], bytes[5]]),
            ],
            material: u16::from_le_bytes([bytes[6], bytes[7]]),
            face: bytes[8],
            ao: [bytes[9], bytes[10], bytes[11], bytes[12]],
            reserved: [bytes[13], bytes[14], bytes[15]],
        }
    }
}

const _: () = assert!(size_of::<Vertex>() == Vertex::SIZE);
const _: () = assert!(align_of::<Vertex>() == Vertex::ALIGN);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_pack() {
        assert_eq!(size_of::<Vertex>(), 16);
        assert_eq!(align_of::<Vertex>(), 4);

        let vertex = Vertex::new([0x1234, 0x5678, 0x9ABC], 0xBEEF, 5, [0, 85, 170, 255]);
        let bytes = vertex.encode();
        assert_eq!(&bytes[0..2], &[0x34, 0x12]);
        assert_eq!(&bytes[2..4], &[0x78, 0x56]);
        assert_eq!(&bytes[4..6], &[0xBC, 0x9A]);
        assert_eq!(&bytes[6..8], &[0xEF, 0xBE]);
        assert_eq!(bytes[8], 5);
        assert_eq!(&bytes[9..13], &[0, 85, 170, 255]);
        assert_eq!(&bytes[13..16], &[0, 0, 0]);
        assert_eq!(Vertex::decode(bytes), vertex);

        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let vertex = Vertex::new(
                [
                    (state & 0xFFFF) as u16,
                    ((state >> 16) & 0xFFFF) as u16,
                    ((state >> 32) & 0xFFFF) as u16,
                ],
                ((state >> 48) & 0xFFFF) as u16,
                (state % 6) as u8,
                [
                    (state & 0xFF) as u8,
                    (state >> 8) as u8,
                    (state >> 16) as u8,
                    (state >> 24) as u8,
                ],
            );
            assert_eq!(Vertex::decode(vertex.encode()), vertex);
        }
    }
}

/// The M3-00 packed 8 B upload vertex (4 per quad + six `u16` indices = 44 B).
///
/// The logical [`Vertex`] stays 16 B for goldens and determinism. This is the
/// GPU encoding: local xyz `u8`, spare, material `u16`, and one `u16` with
/// face (3 bits) + four 2-bit AO levels. The three spare bytes of the logical
/// vertex are not needed here.
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedVertex {
    /// Chunk-local voxel coordinate, `0..=255`.
    pub local: [u8; 3],
    /// Reserved, zero.
    pub spare: u8,
    /// Material id (atlas slot after resolution).
    pub material: u16,
    /// Face 3 bits + AO levels.
    pub face_ao: u16,
}

impl PackedVertex {
    /// Encoded size in bytes.
    pub const SIZE: usize = 8;
    /// Encoded alignment in bytes.
    pub const ALIGN: usize = 4;

    /// Packs a logical vertex.
    ///
    /// # Panics
    ///
    /// Panics if a local coordinate exceeds `u8`, the face is above 5, or an
    /// AO value is not one of the four canonical levels.
    pub fn from_vertex(vertex: &Vertex) -> Self {
        assert!(
            vertex
                .local
                .iter()
                .all(|&coordinate| coordinate <= u8::MAX as u16),
            "local coordinate exceeds the packed 8 B vertex"
        );
        assert!(vertex.face <= 5, "face {} is out of range", vertex.face);
        let mut face_ao = u16::from(vertex.face);
        for (corner, value) in vertex.ao.iter().enumerate() {
            let level: u16 = match value {
                0 => 0,
                85 => 1,
                170 => 2,
                255 => 3,
                other => panic!("AO value {other} is not a canonical level"),
            };
            face_ao |= level << (3 + 2 * corner);
        }
        Self {
            local: [
                vertex.local[0] as u8,
                vertex.local[1] as u8,
                vertex.local[2] as u8,
            ],
            spare: 0,
            material: vertex.material,
            face_ao,
        }
    }

    /// Unpacks to the logical vertex.
    pub fn to_vertex(self) -> Vertex {
        let mut ao = [0u8; 4];
        for (corner, slot) in ao.iter_mut().enumerate() {
            *slot = ((self.face_ao >> (3 + 2 * corner)) & 3) as u8 * 85;
        }
        Vertex::new(
            [
                u16::from(self.local[0]),
                u16::from(self.local[1]),
                u16::from(self.local[2]),
            ],
            self.material,
            (self.face_ao & 7) as u8,
            ao,
        )
    }

    /// Encodes the fixed 8 B little-endian layout.
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..3].copy_from_slice(&self.local);
        bytes[3] = self.spare;
        bytes[4..6].copy_from_slice(&self.material.to_le_bytes());
        bytes[6..8].copy_from_slice(&self.face_ao.to_le_bytes());
        bytes
    }

    /// Decodes the fixed 8 B little-endian layout.
    pub fn decode(bytes: [u8; Self::SIZE]) -> Self {
        Self {
            local: [bytes[0], bytes[1], bytes[2]],
            spare: bytes[3],
            material: u16::from_le_bytes([bytes[4], bytes[5]]),
            face_ao: u16::from_le_bytes([bytes[6], bytes[7]]),
        }
    }
}

const _: () = assert!(size_of::<PackedVertex>() == PackedVertex::SIZE);
const _: () = assert!(CHUNK_DIM <= u8::MAX as usize);
const _: () = assert!(align_of::<PackedVertex>() == PackedVertex::ALIGN);

#[cfg(test)]
mod packed_tests {
    use super::*;

    #[test]
    fn packed_vertex_round_trips_every_field() {
        let levels = [0u8, 85, 170, 255];
        for face in 0..=5u8 {
            for material in [0u16, 1, 255, 0xBEEF] {
                for ao in [levels, [0; 4], [255; 4], [85, 170, 255, 0]] {
                    let vertex = Vertex::new([0, 64, 128], material, face, ao);
                    let packed = PackedVertex::from_vertex(&vertex);
                    assert_eq!(packed.to_vertex(), vertex);
                    assert_eq!(PackedVertex::decode(packed.encode()), packed);
                    assert_eq!(packed.spare, 0);
                    assert_eq!(packed.face_ao & 7, u16::from(face));
                }
            }
        }
        let local = Vertex::new([255, 0, 1], 7, 5, [85; 4]);
        assert_eq!(
            PackedVertex::decode(PackedVertex::from_vertex(&local).encode()).to_vertex(),
            local
        );
    }

    #[test]
    fn packed_vertex_has_a_literal_byte_layout() {
        let vertex = Vertex::new([1, 2, 3], 0xBEEF, 5, [0, 85, 170, 255]);
        let bytes = PackedVertex::from_vertex(&vertex).encode();
        assert_eq!(bytes, [1, 2, 3, 0, 0xEF, 0xBE, 0x25, 0x07]);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn packed_vertex_rejects_a_face_above_five() {
        let _ = PackedVertex::from_vertex(&Vertex::new([0, 0, 0], 1, 6, [255; 4]));
    }

    #[test]
    #[should_panic(expected = "exceeds")]
    fn packed_vertex_rejects_a_local_above_u8() {
        let _ = PackedVertex::from_vertex(&Vertex::new([256, 0, 0], 1, 0, [255; 4]));
    }

    #[test]
    #[should_panic(expected = "canonical")]
    fn packed_vertex_rejects_a_non_canonical_ao() {
        let _ = PackedVertex::from_vertex(&Vertex::new([0, 0, 0], 1, 0, [1; 4]));
    }

    #[test]
    fn packed_vertex_is_eight_bytes() {
        assert_eq!(size_of::<PackedVertex>(), 8);
        assert_eq!(align_of::<PackedVertex>(), 4);
    }
}
