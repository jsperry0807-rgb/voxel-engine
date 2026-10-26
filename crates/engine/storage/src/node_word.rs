//! Internal node words and band-safe node references.
//!
//! The bit layout itself lives in [`voxel_core::node_word`] (M0-05); this module
//! gives it the storage-side types used by the banded layout:
//!
//! - [`Node`] is one 8 B internal node word. It is `#[repr(transparent)]` over
//!   the `u64` the GPU reads, and all field access goes through the M0 encoder
//!   and decoder, so there is exactly one definition of the layout.
//! - [`NodeRef`] is `(level, index)` with the level kept as part of the value.
//!   Two nodes at different levels never compare equal even when their indices
//!   match, which keeps a reference from being silently resolved in the wrong
//!   level band.
//!
//! A [`NodeRef`]'s index addresses one node slot in the shared pool. The `u48`
//! base field bounds the addressable slots at [`NODE_INDEX_MAX`]; child
//! addressing (`base + popcount(...)`) is M1-05, not this module.
//!
//! # Example
//!
//! ```
//! use voxel_storage::node_word::{Node, NodeRef};
//!
//! let node = Node::new(0b1010_0101, 3, 42);
//! assert_eq!(node.mask(), 0b1010_0101);
//! assert_eq!(node.level(), 3);
//! assert_eq!(node.base(), 42);
//! assert_eq!(Node::decode(node.encode()), node);
//!
//! let child = NodeRef::new(2, node.base());
//! assert_eq!((child.level(), child.index()), (2, 42));
//! assert_ne!(child, NodeRef::new(1, 42)); // same index, different band
//! ```

use core::fmt;

use voxel_core::constants::ROOT_LEVEL;
use voxel_core::node_word::{
    NODE_BASE_BITS, NODE_BASE_MAX, NODE_LEVEL_MAX, decode_node, encode_node,
};

/// Largest node-slot index addressable by the `u48` base field.
pub const NODE_INDEX_MAX: u64 = (1u64 << NODE_BASE_BITS) - 1;

// The index bound is the base width; the two constants must not drift apart.
const _: () = {
    assert!(NODE_INDEX_MAX == NODE_BASE_MAX);
    assert!(ROOT_LEVEL <= NODE_LEVEL_MAX);
};

/// One 8 B internal SVDAG node.
///
/// The word is stored exactly as the GPU reads it: `mask` in bits 0–7, `level`
/// in bits 8–10, reserved flags in bits 11–15 (always zero today) and the first
/// child's base index in bits 16–63. Use [`Node::new`] to encode and
/// [`Node::decode`] to inspect; [`Node::encode`] returns the raw word for
/// upload and serialization.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Node(u64);

impl Node {
    /// Packs `mask`, `level` and `base` into one node word.
    ///
    /// The fields must fit their M0 widths; the encoder asserts that in debug
    /// builds and masks them to width in release builds. The reserved flag bits
    /// are always written as zero.
    pub const fn new(mask: u8, level: u32, base: u64) -> Self {
        Self(encode_node(mask, level, base))
    }

    /// Decodes a raw node word.
    ///
    /// This is infallible: reserved flag bits are ignored, matching
    /// [`voxel_core::node_word::decode_node`].
    pub const fn decode(word: u64) -> Self {
        Self(word)
    }

    /// The raw 8 B word, ready for upload or serialization.
    pub const fn encode(self) -> u64 {
        self.0
    }

    /// The child-presence mask: bit *c* is set when child *c* exists.
    pub const fn mask(self) -> u8 {
        decode_node(self.0).0
    }

    /// The node's level, in the pool numbering where the root is
    /// [`voxel_core::constants::ROOT_LEVEL`].
    pub const fn level(self) -> u32 {
        decode_node(self.0).1
    }

    /// Index of the first present child slot in the next level band.
    ///
    /// Child *c* lives at `base + popcount(mask & ((1 << c) - 1))`.
    pub const fn base(self) -> u64 {
        decode_node(self.0).2
    }

    /// Number of present children carried by this node.
    pub const fn child_count(self) -> u32 {
        self.mask().count_ones()
    }
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mask, level, base) = decode_node(self.0);
        f.debug_struct("Node")
            .field("mask", &mask)
            .field("level", &level)
            .field("base", &base)
            .finish()
    }
}

impl From<u64> for Node {
    fn from(word: u64) -> Self {
        Self::decode(word)
    }
}

impl From<Node> for u64 {
    fn from(node: Node) -> Self {
        node.encode()
    }
}

/// A node address: the level band plus the index within the shared pool.
///
/// The level is part of the value so the same index in two bands is two
/// distinct references. A [`NodeRef`] is only valid in the band named by its
/// level; the `Bands` arena checks that on every access (M1-02). The
/// constructor rejects a level above [`ROOT_LEVEL`] so no reference can name a
/// band the chunk does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeRef {
    // Declaration order is the ordering: band first, then index within it.
    level: u32,
    index: u64,
}

impl NodeRef {
    /// Creates a reference to `index` in the band for `level`.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL` or `index > NODE_INDEX_MAX`; both would
    /// address outside the chunk's bands or outside the `u48` address space.
    pub const fn new(level: u32, index: u64) -> Self {
        assert!(
            level <= ROOT_LEVEL,
            "node reference level is outside the chunk's bands"
        );
        assert!(
            index <= NODE_INDEX_MAX,
            "node reference index exceeds the u48 address space"
        );
        Self { level, index }
    }

    /// The level band this reference belongs to.
    pub const fn level(self) -> u32 {
        self.level
    }

    /// The node-slot index within the shared pool.
    pub const fn index(self) -> u64 {
        self.index
    }
}

impl fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeRef")
            .field("level", &self.level)
            .field("index", &self.index)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::node_word::{NODE_FLAGS_BITS, NODE_FLAGS_SHIFT, NODE_MASK_MAX};

    /// The five flag bits, which M0 requires to stay zero.
    const RESERVED_MASK: u64 = ((1u64 << NODE_FLAGS_BITS) - 1) << NODE_FLAGS_SHIFT;

    /// The GPU reads a pool of 8 B words, so the typed node must not grow.
    #[test]
    fn node_word_node_stays_one_word() {
        assert_eq!(core::mem::size_of::<Node>(), core::mem::size_of::<u64>());
        assert_eq!(core::mem::align_of::<Node>(), core::mem::align_of::<u64>());
    }

    #[test]
    fn node_word_roundtrips_every_field_combination() {
        for mask in 0..=NODE_MASK_MAX {
            for level in 0..=NODE_LEVEL_MAX {
                for base in [0, 1, 2, NODE_BASE_MAX] {
                    let node = Node::new(mask, level, base);

                    assert_eq!(node.mask(), mask);
                    assert_eq!(node.level(), level);
                    assert_eq!(node.base(), base);
                    assert_eq!(node.child_count(), mask.count_ones());
                    assert_eq!(node.encode(), encode_node(mask, level, base));
                    assert_eq!(Node::decode(node.encode()), node);
                    assert_eq!(node.encode() & RESERVED_MASK, 0);
                }
            }
        }
    }

    #[test]
    fn node_word_fields_land_at_the_m0_bit_positions() {
        assert_eq!(Node::new(0b0000_0001, 0, 0).encode(), 1);
        assert_eq!(Node::new(0b1000_0000, 0, 0).encode(), 1 << 7);
        assert_eq!(Node::new(0, 1, 0).encode(), 1 << 8);
        assert_eq!(Node::new(0, 0, 1).encode(), 1 << 16);
        assert_eq!(
            Node::new(NODE_MASK_MAX, NODE_LEVEL_MAX, NODE_BASE_MAX).encode(),
            0xffff_ffff_ffff_07ff
        );
    }

    #[test]
    fn node_word_decode_ignores_reserved_bits() {
        let clean = Node::new(0x12, 5, 0x34);
        let dirty = Node::decode(clean.encode() | RESERVED_MASK);

        assert_eq!(dirty.mask(), clean.mask());
        assert_eq!(dirty.level(), clean.level());
        assert_eq!(dirty.base(), clean.base());
    }

    #[test]
    fn node_word_converts_to_and_from_the_raw_word() {
        let word = encode_node(0x5a, 4, 0x1234);
        let node = Node::from(word);

        assert_eq!(u64::from(node), word);
        assert_eq!(Node::decode(word), node);
    }

    #[test]
    fn node_word_helpers_work_in_const_context() {
        const NODE: Node = Node::new(0x5a, 4, 0x1234);
        const WORD: u64 = NODE.encode();
        const MASK: u8 = NODE.mask();
        const LEVEL: u32 = NODE.level();
        const BASE: u64 = NODE.base();
        const REF: NodeRef = NodeRef::new(3, 0x1234);
        const REF_LEVEL: u32 = REF.level();
        const REF_INDEX: u64 = REF.index();

        assert_eq!((MASK, LEVEL, BASE), (0x5a, 4, 0x1234));
        assert_eq!(WORD, encode_node(0x5a, 4, 0x1234));
        assert_eq!((REF_LEVEL, REF_INDEX), (3, 0x1234));
    }

    #[test]
    fn node_word_node_ref_keeps_level_and_index() {
        for level in 0..=ROOT_LEVEL {
            for index in [0, 1, 7, NODE_INDEX_MAX] {
                let node = NodeRef::new(level, index);
                assert_eq!(node.level(), level);
                assert_eq!(node.index(), index);
            }
        }
    }

    /// Same index, different level: the references must stay distinct so a
    /// lookup cannot silently resolve across a band boundary.
    #[test]
    fn node_word_node_ref_never_crosses_bands() {
        for index in [0, 1, 7, NODE_INDEX_MAX] {
            for level in 0..=ROOT_LEVEL {
                let here = NodeRef::new(level, index);
                for other in 0..=ROOT_LEVEL {
                    let there = NodeRef::new(other, index);
                    if level == other {
                        assert_eq!(here, there);
                    } else {
                        assert_ne!(here, there);
                    }
                }
            }
        }

        // Ordering groups by band before index, so a band is a contiguous range.
        assert!(NodeRef::new(0, NODE_INDEX_MAX) < NodeRef::new(1, 0));
    }

    #[test]
    #[should_panic(expected = "outside the chunk's bands")]
    fn node_word_node_ref_rejects_a_level_without_a_band() {
        let _ = NodeRef::new(ROOT_LEVEL + 1, 0);
    }

    #[test]
    #[should_panic(expected = "u48 address space")]
    fn node_word_node_ref_rejects_an_index_past_the_u48_space() {
        let _ = NodeRef::new(0, NODE_INDEX_MAX + 1);
    }
}
