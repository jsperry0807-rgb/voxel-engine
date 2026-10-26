//! Bit layout of one internal SVDAG node word.
//!
//! An internal node is a single `u64` (spec §10.1). The fields are packed from
//! the least-significant bit upward:
//!
//! | field | bits | width | meaning |
//! | --- | --- | --- | --- |
//! | [`NODE_MASK_SHIFT`] | 0–7 | 8 | one presence bit per child |
//! | [`NODE_LEVEL_SHIFT`] | 8–10 | 3 | level of the node |
//! | [`NODE_FLAGS_SHIFT`] | 11–15 | 5 | flags, reserved and zero in M0 |
//! | [`NODE_BASE_SHIFT`] | 16–63 | 48 | index of the first child slot |
//!
//! `mask` bit *c* says child *c* is present. Present children are compacted in
//! ascending child order, so child *c* lives at
//! `base + popcount(mask & ((1 << c) - 1))`; absent children cost no slot.
//!
//! [`encode_node`] always writes the flag bits as zero, and [`decode_node`]
//! ignores them. They are reserved for a later milestone, so a word produced
//! today decodes to the same three fields regardless of what the flags become.
//!
//! # Example
//!
//! ```
//! use voxel_core::node_word::{decode_node, encode_node, NODE_BASE_SHIFT};
//!
//! let word = encode_node(0b1010_0101, 3, 42);
//! assert_eq!(decode_node(word), (0b1010_0101, 3, 42));
//! assert_eq!(word >> NODE_BASE_SHIFT, 42);
//! ```

/// Bit offset of the child-presence mask.
pub const NODE_MASK_SHIFT: u32 = 0;

/// Width of the child-presence mask: one bit per child.
pub const NODE_MASK_BITS: u32 = 8;

/// Bit offset of the node level.
pub const NODE_LEVEL_SHIFT: u32 = 8;

/// Width of the node level: `u3`, enough for the 128³ target's level 7.
pub const NODE_LEVEL_BITS: u32 = 3;

/// Bit offset of the node flags.
///
/// The flags are reserved in M0 and must stay zero.
pub const NODE_FLAGS_SHIFT: u32 = 11;

/// Width of the node flags.
pub const NODE_FLAGS_BITS: u32 = 5;

/// Bit offset of the first-child base index.
pub const NODE_BASE_SHIFT: u32 = 16;

/// Width of the first-child base index: `u48`.
pub const NODE_BASE_BITS: u32 = 48;

/// Largest representable child-presence mask.
pub const NODE_MASK_MAX: u8 = u8::MAX;

/// Largest representable node level.
pub const NODE_LEVEL_MAX: u32 = (1 << NODE_LEVEL_BITS) - 1;

/// Largest representable first-child base index.
pub const NODE_BASE_MAX: u64 = (1 << NODE_BASE_BITS) - 1;

const MASK_FIELD_MAX: u64 = (1 << NODE_MASK_BITS) - 1;
const LEVEL_FIELD_MAX: u64 = (1 << NODE_LEVEL_BITS) - 1;
const BASE_FIELD_MAX: u64 = (1 << NODE_BASE_BITS) - 1;

/// Packs `mask`, `level` and `base` into one node word.
///
/// The fields must fit their widths ([`NODE_MASK_MAX`], [`NODE_LEVEL_MAX`],
/// [`NODE_BASE_MAX`]); debug builds assert that they do. Values are masked to
/// their field width so they can never overwrite a neighbouring field. The
/// flag bits are always zero.
pub const fn encode_node(mask: u8, level: u32, base: u64) -> u64 {
    debug_assert!(level <= NODE_LEVEL_MAX);
    debug_assert!(base <= NODE_BASE_MAX);

    (((mask as u64) & MASK_FIELD_MAX) << NODE_MASK_SHIFT)
        | (((level as u64) & LEVEL_FIELD_MAX) << NODE_LEVEL_SHIFT)
        | ((base & BASE_FIELD_MAX) << NODE_BASE_SHIFT)
}

/// Unpacks a node word into `(mask, level, base)`.
///
/// Flag bits are ignored, so a word with them set decodes exactly like the
/// same word with them cleared.
pub const fn decode_node(word: u64) -> (u8, u32, u64) {
    (
        ((word >> NODE_MASK_SHIFT) & MASK_FIELD_MAX) as u8,
        ((word >> NODE_LEVEL_SHIFT) & LEVEL_FIELD_MAX) as u32,
        (word >> NODE_BASE_SHIFT) & BASE_FIELD_MAX,
    )
}

// The bit layout is a contract with the GPU traversal and the serialized chunk
// format. Check that the fields tile the word exactly and that every level the
// engine can build fits the level field.
const _: () = {
    assert!(NODE_MASK_SHIFT == 0);
    assert!(NODE_MASK_SHIFT + NODE_MASK_BITS == NODE_LEVEL_SHIFT);
    assert!(NODE_LEVEL_SHIFT + NODE_LEVEL_BITS == NODE_FLAGS_SHIFT);
    assert!(NODE_FLAGS_SHIFT + NODE_FLAGS_BITS == NODE_BASE_SHIFT);
    assert!(NODE_BASE_SHIFT + NODE_BASE_BITS == u64::BITS);

    assert!(NODE_MASK_MAX == 0xff);
    assert!(NODE_LEVEL_MAX == 7);
    assert!(NODE_BASE_MAX == (1u64 << 48) - 1);
    assert!(crate::constants::ROOT_LEVEL <= NODE_LEVEL_MAX);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The five flag bits, which M0 requires to stay zero.
    const RESERVED_MASK: u64 = ((1u64 << NODE_FLAGS_BITS) - 1) << NODE_FLAGS_SHIFT;

    #[test]
    fn node_word_fields_tile_the_word() {
        assert_eq!(NODE_MASK_SHIFT, 0);
        assert_eq!(NODE_MASK_SHIFT + NODE_MASK_BITS, NODE_LEVEL_SHIFT);
        assert_eq!(NODE_LEVEL_SHIFT + NODE_LEVEL_BITS, NODE_FLAGS_SHIFT);
        assert_eq!(NODE_FLAGS_SHIFT + NODE_FLAGS_BITS, NODE_BASE_SHIFT);
        assert_eq!(NODE_BASE_SHIFT + NODE_BASE_BITS, u64::BITS);
    }

    /// Locks the exact bit positions from spec §10.1.
    #[test]
    fn node_word_fields_land_at_the_specified_bits() {
        assert_eq!(encode_node(0b0000_0001, 0, 0), 1);
        assert_eq!(encode_node(0b1000_0000, 0, 0), 1 << 7);
        assert_eq!(encode_node(0, 1, 0), 1 << NODE_LEVEL_SHIFT);
        assert_eq!(encode_node(0, 0, 1), 1 << NODE_BASE_SHIFT);

        // All defined fields set, with the reserved flag bits clear.
        assert_eq!(
            encode_node(NODE_MASK_MAX, NODE_LEVEL_MAX, NODE_BASE_MAX),
            0xffff_ffff_ffff_07ff
        );
    }

    #[test]
    fn node_word_roundtrips_all_masks() {
        for mask in 0..=NODE_MASK_MAX {
            let word = encode_node(mask, 3, 0x1234);
            assert_eq!(decode_node(word), (mask, 3, 0x1234));
            assert_eq!(word & RESERVED_MASK, 0);
        }
    }

    #[test]
    fn node_word_roundtrips_all_levels() {
        for level in 0..=NODE_LEVEL_MAX {
            let word = encode_node(0xa5, level, 0x1234);
            assert_eq!(decode_node(word), (0xa5, level, 0x1234));
            assert_eq!(word & RESERVED_MASK, 0);
        }
    }

    #[test]
    fn node_word_roundtrips_base_extremes() {
        for base in [0, 1, NODE_BASE_MAX] {
            let word = encode_node(NODE_MASK_MAX, NODE_LEVEL_MAX, base);
            assert_eq!(decode_node(word), (NODE_MASK_MAX, NODE_LEVEL_MAX, base));
            assert_eq!(word & RESERVED_MASK, 0);
        }
    }

    /// Every mask, every level and the boundary bases, including the maximum.
    #[test]
    fn node_word_roundtrips_every_field_combination() {
        for mask in 0..=NODE_MASK_MAX {
            for level in 0..=NODE_LEVEL_MAX {
                for base in [0, 1, 2, NODE_BASE_MAX] {
                    let word = encode_node(mask, level, base);
                    assert_eq!(decode_node(word), (mask, level, base));
                    assert_eq!(word & RESERVED_MASK, 0);
                }
            }
        }
    }

    #[test]
    fn node_word_reserved_bits_stay_zero() {
        let word = encode_node(NODE_MASK_MAX, NODE_LEVEL_MAX, NODE_BASE_MAX);
        assert_eq!(word & RESERVED_MASK, 0);
    }

    #[test]
    fn node_word_decode_ignores_reserved_bits() {
        let clean = encode_node(0x12, 5, 0x34);
        let dirty = clean | RESERVED_MASK;
        assert_eq!(decode_node(dirty), decode_node(clean));
    }

    /// The helpers are `const`, so buffer sizes and shader constants can use
    /// them at compile time.
    #[test]
    fn node_word_helpers_work_in_const_context() {
        const WORD: u64 = encode_node(0x5a, 4, 0x1234);
        const DECODED: (u8, u32, u64) = decode_node(WORD);

        assert_eq!(DECODED, (0x5a, 4, 0x1234));
    }
}
