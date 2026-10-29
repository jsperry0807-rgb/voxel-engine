//! The ADR-010 16 B mesh vertex (M2-10).
//!
//! The layout is fixed: three chunk-local `u16` coordinates, a `u16` material,
//! the face code, four `u8` corner AO values and three reserved zero bytes.
//! There is no UV, normal, tangent, colour or per-vertex light field.
//!
//! The explicit byte codec is the contract used by the golden-mesh tests and by
//! the GPU upload path; `size_of` and `align_of` are asserted at compile time.

use std::mem::{align_of, size_of};

/// One mesh vertex, exactly 16 B and four-byte aligned.
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    /// Chunk-local voxel coordinate, `0..=CHUNK_DIM`.
    pub local: [u16; 3],
    /// Atlas material id; air is never emitted.
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
