//! Chunk serialization v2 (M1-13).
//!
//! The v1 format was the variable-length `u32` pool in [`crate::dag`]. The
//! banded layout needs a stable container for three things: the per-level node
//! bands, the leaf arena, and enough header data to validate a load before it
//! touches a caller's chunk. This module defines that container.
//!
//! Every integer is little-endian. The header is fixed for a given
//! [`CHUNK_BITS`](crate::CHUNK_BITS), so a reader can reject a foreign edge
//! before it interprets any lengths:
//!
//! | offset | bytes | field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `VXV2` ([`MAGIC`]) |
//! | 4 | 2 | format version ([`VERSION`], currently 2) |
//! | 6 | 1 | `CHUNK_BITS`, as a `u8` ([`CHUNK_BITS_OFFSET`]) |
//! | 7 | 1 | root present: `0` for an all-air chunk, `1` for a root |
//! | 8 | 8 | root index in the root band (`0` when absent) |
//! | 16 | 8 × (`ROOT_LEVEL` + 1) | node length of each band, level 0 first |
//! | … | 8 | leaf arena length, in `u16` slots |
//! | … | 8 × nodes | band words, level 0 first, append order within each band |
//! | … | 2 × leaves | leaf material IDs, append order |
//!
//! The leading magic/version/`CHUNK_BITS` fields are readable from the first
//! seven bytes alone. A mismatch in any of them is a rejected file, never a
//! reinterpretation: a 64³ build will not load a 128³ file, and a v1
//! [`Svdag`](crate::dag::Svdag) payload is not mistaken for a v2 chunk.
//!
//! The explicit leaf length is what makes truncation unambiguous. Deriving it
//! from "the rest of the file" would let a file cut on an even byte boundary
//! look like a valid chunk with a shorter leaf arena.
//!
//! # Validation
//!
//! [`ChunkDag::from_bytes`] returns `None` for every malformed input. It checks
//! the header magic, version and edge; requires the node and leaf lengths to
//! exactly account for the payload; rejects a root outside the root band; and
//! rejects a node whose encoded level does not match the band it was stored in.
//! It never panics on arbitrary bytes, and it never allocates from a length that
//! is not backed by the input slice.
//!
//! # Example
//!
//! ```
//! use voxel_storage::{CHUNK_VOLUME, ChunkDag};
//!
//! let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
//! let bytes = dag.to_bytes();
//! assert_eq!(ChunkDag::from_bytes(&bytes), Some(dag));
//!
//! // A prefix is always rejected, even when it cuts the leaf arena on a
//! // two-byte boundary.
//! assert!(ChunkDag::from_bytes(&bytes[..bytes.len() - 2]).is_none());
//! ```

use voxel_core::constants::ROOT_LEVEL;

use crate::CHUNK_BITS;
use crate::bands::BAND_COUNT;
use crate::chunk::ChunkDag;
use crate::node_word::{NODE_INDEX_MAX, Node, NodeRef};

/// Magic number of a serialized chunk: bytes `VXV2` in little-endian order.
pub const MAGIC: u32 = 0x3256_5856;

/// Current chunk serialization version.
pub const VERSION: u16 = 2;

/// Offset of the [`CHUNK_BITS`] byte in the header.
pub const CHUNK_BITS_OFFSET: usize = 6;

/// Byte length of the fixed header, including the level table and leaf length.
pub const HEADER_BYTES: usize = 16 + BAND_COUNT * core::mem::size_of::<u64>() + 8;

/// Node words start here, after the header.
const NODES_OFFSET: usize = HEADER_BYTES;

// The edge must fit the one-byte header field, and the level table must cover
// every band plus the leaf length at its tail.
const _: () = {
    assert!(CHUNK_BITS <= u8::MAX as u32);
    assert!(HEADER_BYTES == 16 + BAND_COUNT * 8 + 8);
};

impl ChunkDag {
    /// Serializes this chunk into the v2 byte format.
    ///
    /// The output is canonical: bands are emitted level `0` through
    /// [`ROOT_LEVEL`], each in append order, followed by the leaf arena. The
    /// root keeps its band-local index, so deserializing reproduces the same
    /// [`NodeRef`]s.
    ///
    /// The header records the explicit node length of every band and the leaf
    /// length, which lets [`Self::from_bytes`] reject truncation and trailing
    /// bytes rather than accepting a shorter arena.
    pub fn to_bytes(&self) -> Vec<u8> {
        let bands = self.bands();
        let lengths: [u64; BAND_COUNT] = core::array::from_fn(|level| bands.len(level as u32));
        let leaf_len = bands.leaf_len();
        let total_nodes: u64 = lengths.iter().sum();
        let payload_bytes = total_nodes
            .checked_mul(8)
            .and_then(|nodes| nodes.checked_add(leaf_len.checked_mul(2)?))
            .expect("chunk node and leaf lengths fit in u64");
        let capacity = HEADER_BYTES
            + usize::try_from(payload_bytes)
                .expect("a chunk that fits memory has a byte length that fits usize");

        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.push(CHUNK_BITS as u8);
        match self.root() {
            Some(root) => {
                debug_assert_eq!(root.level(), ROOT_LEVEL);
                out.push(1);
                out.extend_from_slice(&root.index().to_le_bytes());
            }
            None => {
                out.push(0);
                out.extend_from_slice(&0u64.to_le_bytes());
            }
        }
        for length in lengths {
            out.extend_from_slice(&length.to_le_bytes());
        }
        out.extend_from_slice(&leaf_len.to_le_bytes());

        for level in 0..=ROOT_LEVEL {
            for node in bands.band(level) {
                out.extend_from_slice(&node.encode().to_le_bytes());
            }
        }
        for &material in bands.leaves() {
            out.extend_from_slice(&material.to_le_bytes());
        }
        debug_assert_eq!(out.len(), capacity);
        out
    }

    /// Deserializes a v2 chunk, or returns `None` if the bytes are not a valid,
    /// complete chunk for this build's [`CHUNK_BITS`].
    ///
    /// Rejection is total: a foreign magic or version, a different chunk edge,
    /// a truncated or over-long payload, a root outside the root band, and a
    /// node stored in the wrong level band all return `None`. The function
    /// never panics on untrusted bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        // The identifying prefix is readable before the rest of the header.
        if bytes.len() < CHUNK_BITS_OFFSET + 1 {
            return None;
        }
        if read_u32(bytes, 0)? != MAGIC || read_u16(bytes, 4)? != VERSION {
            return None;
        }
        if bytes[CHUNK_BITS_OFFSET] != CHUNK_BITS as u8 {
            return None;
        }
        if bytes.len() < HEADER_BYTES {
            return None;
        }

        let root_present = match bytes[7] {
            0 => false,
            1 => true,
            _ => return None,
        };
        let root_index = read_u64(bytes, 8)?;
        let mut lengths = [0u64; BAND_COUNT];
        let mut nodes = 0u64;
        for (level, length) in lengths.iter_mut().enumerate() {
            *length = read_u64(bytes, 16 + level * 8)?;
            if *length > NODE_INDEX_MAX {
                return None;
            }
            nodes = nodes.checked_add(*length)?;
        }
        let leaf_len = read_u64(bytes, 16 + BAND_COUNT * 8)?;

        // Check the declared sections against the actual slice before reading
        // any of them. This also bounds every loop below by the input length.
        let node_bytes = nodes.checked_mul(8)?;
        let leaf_bytes = leaf_len.checked_mul(2)?;
        let expected = (HEADER_BYTES as u64)
            .checked_add(node_bytes)?
            .checked_add(leaf_bytes)?;
        if expected != bytes.len() as u64 {
            return None;
        }

        if !root_present && root_index != 0 {
            return None;
        }
        let root = if root_present {
            if root_index >= lengths[ROOT_LEVEL as usize] {
                return None;
            }
            Some(NodeRef::new(ROOT_LEVEL, root_index))
        } else {
            None
        };

        let mut dag = Self::empty();
        let mut offset = NODES_OFFSET;
        for (level, &length) in lengths.iter().enumerate() {
            let level = level as u32;
            for _ in 0..length {
                let word = read_u64(bytes, offset)?;
                let node = Node::decode(word);
                // `Bands::append` asserts this; checking first keeps a corrupt
                // word from panicking the loader.
                if node.level() != level {
                    return None;
                }
                dag.bands_mut().append(level, node);
                offset += 8;
            }
        }

        let leaf_slots = usize::try_from(leaf_len).ok()?;
        for _ in 0..leaf_slots {
            let material = read_u16(bytes, offset)?;
            dag.bands_mut().append_leaf(material);
            offset += 2;
        }
        debug_assert_eq!(offset, bytes.len());

        dag.set_root(root);
        Some(dag)
    }
}

#[inline]
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

#[inline]
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[inline]
fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CHUNK_DIM, CHUNK_VOLUME, local_index};

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// The five M1-07 fixtures: empty, uniform, layered, alternating and noisy.
    fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
        let air = vec![0u16; CHUNK_VOLUME];
        let solid = vec![3u16; CHUNK_VOLUME];

        let mut slab = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if y < 17 {
                        slab[local_index(x as u32, y as u32, z as u32) as usize] =
                            1 + (y % 3) as u16;
                    }
                }
            }
        }

        let mut checker = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    checker[local_index(x as u32, y as u32, z as u32) as usize] =
                        if (x + y + z) % 2 == 0 { 1 } else { 2 };
                }
            }
        }

        let mut noise = vec![0u16; CHUNK_VOLUME];
        let mut state = 0x5eed_1e55_c0de_1234u64;
        for voxel in &mut noise {
            let sample = rng(&mut state);
            if sample % 100 < 30 {
                *voxel = 1 + ((sample >> 8) % 7) as u16;
            }
        }

        vec![
            ("air", air),
            ("solid", solid),
            ("slab", slab),
            ("checker", checker),
            ("noise", noise),
        ]
    }

    /// M1-13 acceptance: every fixture round-trips into an equal chunk, the
    /// encoding is canonical, and expansion still reproduces the dense input.
    #[test]
    fn bytes_v2_roundtrip_on_five_fixtures() {
        for (name, dense) in fixtures() {
            let dag = ChunkDag::from_dense(&dense);
            let bytes = dag.to_bytes();
            let reloaded = ChunkDag::from_bytes(&bytes)
                .unwrap_or_else(|| panic!("{name}: valid bytes were rejected"));

            assert_eq!(reloaded, dag, "{name}: chunk differs after round-trip");
            assert_eq!(
                reloaded.to_bytes(),
                bytes,
                "{name}: encoding is not canonical"
            );
            assert_eq!(reloaded.root(), dag.root(), "{name}");
            assert_eq!(reloaded.node_words(), dag.node_words(), "{name}");
            assert_eq!(reloaded.leaf_words(), dag.leaf_words(), "{name}");

            let mut rebuilt = vec![0u16; CHUNK_VOLUME];
            reloaded.expand(&mut rebuilt);
            assert_eq!(rebuilt, dense, "{name}: expansion differs");
        }
    }

    /// A COW chunk keeps stale runs and moves the root to a later slot in the
    /// root band. The format must preserve that root index, not just slot zero.
    #[test]
    fn bytes_v2_roundtrip_preserves_a_cow_root_index() {
        let dense = vec![3u16; CHUNK_VOLUME];
        let mut dag = ChunkDag::from_dense(&dense);
        assert_eq!(dag.root().expect("solid chunk has a root").index(), 0);

        dag.apply_edits([(local_index(3, 4, 5), 9), (local_index(17, 33, 41), 11)]);
        let root_index = dag.root().expect("the edit keeps a root").index();
        assert!(root_index > 0, "a COW edit must publish a new root slot");
        assert_eq!(dag.bands().len(ROOT_LEVEL), root_index + 1);

        let bytes = dag.to_bytes();
        let reloaded = ChunkDag::from_bytes(&bytes).expect("COW bytes must load");
        assert_eq!(reloaded, dag);
        assert_eq!(reloaded.root(), dag.root());
        assert_eq!(reloaded.get(3, 4, 5), 9);
        assert_eq!(reloaded.get(17, 33, 41), 11);
        assert_eq!(reloaded.get(0, 0, 0), 3);
    }

    /// M1-13 acceptance: every truncation of a valid encoding returns `None`.
    /// The uniform fixture keeps the O(n²) all-prefix sweep cheap while still
    /// covering a non-empty root, every internal band and the leaf arena.
    #[test]
    fn bytes_v2_truncation_returns_none_never_panics() {
        let dag = ChunkDag::from_dense(&vec![7u16; CHUNK_VOLUME]);
        let bytes = dag.to_bytes();
        assert_eq!(ChunkDag::from_bytes(&bytes), Some(dag.clone()));

        for end in 0..bytes.len() {
            assert!(
                ChunkDag::from_bytes(&bytes[..end]).is_none(),
                "a {end}-byte prefix was accepted"
            );
        }

        // In particular, cutting a whole trailing leaf slot is not a valid
        // shorter arena: the header's leaf length must agree exactly.
        let mut missing_leaf = bytes.clone();
        missing_leaf.truncate(bytes.len() - 2);
        assert!(ChunkDag::from_bytes(&missing_leaf).is_none());

        // Trailing bytes are rejected too.
        let mut extended = bytes.clone();
        extended.extend_from_slice(&[0, 0]);
        assert!(ChunkDag::from_bytes(&extended).is_none());
    }

    /// M1-13 acceptance: a file written at another chunk edge is rejected even
    /// when everything else about it is well formed.
    #[test]
    fn bytes_v2_rejects_chunk_bits_mismatch() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let bytes = dag.to_bytes();
        assert_eq!(ChunkDag::from_bytes(&bytes), Some(dag));

        let mut foreign = bytes.clone();
        foreign[CHUNK_BITS_OFFSET] = (CHUNK_BITS as u8).wrapping_add(1);
        assert!(
            ChunkDag::from_bytes(&foreign).is_none(),
            "a different CHUNK_BITS must be rejected"
        );

        // The check happens before the level table is interpreted, so even a
        // header-only prefix with a foreign edge is rejected rather than read.
        let header_prefix = &foreign[..CHUNK_BITS_OFFSET + 1];
        assert!(ChunkDag::from_bytes(header_prefix).is_none());
    }

    /// Magic and version are part of the same gate as the edge: a v1 payload or
    /// an unknown future version is not guessed at.
    #[test]
    fn bytes_v2_rejects_wrong_magic_and_version() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let bytes = dag.to_bytes();

        let mut bad_magic = bytes.clone();
        bad_magic[0] ^= 0xff;
        assert!(ChunkDag::from_bytes(&bad_magic).is_none());

        let mut bad_version = bytes.clone();
        bad_version[4] = VERSION.wrapping_add(1) as u8;
        assert!(ChunkDag::from_bytes(&bad_version).is_none());
    }

    /// A root index outside its band is corrupt, not an empty chunk. The empty
    /// chunk is the explicit `root present = 0` encoding instead.
    #[test]
    fn bytes_v2_rejects_a_root_outside_the_root_band() {
        let dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let out_of_range = dag.bands().len(ROOT_LEVEL) + 1;
        bytes[8..16].copy_from_slice(&out_of_range.to_le_bytes());
        assert!(ChunkDag::from_bytes(&bytes).is_none());
    }
}
