//! Three-tier hierarchical sparse volumetric store.
//!
//! T1 `disk`    region files: append-only log of serialized SVDAGs + overlay snapshots
//! T2 `world`   `FxMap<ChunkPos, Chunk{Arc<Svdag>, Overlay}>`; edits hit the O(1) overlay only
//! T3 `gpu`     global deduped node pool + dynamic chunk hash table + per-chunk edit overlay
pub mod bands;
pub mod chunk;
pub mod compact;
pub mod dag;
pub mod disk;
pub mod edit;
pub mod gpu;
pub mod hash;
pub mod invariants;
pub mod leaf;
pub mod node_word;
pub mod overlay;
pub mod run;
pub mod scan;
pub mod serialize;
pub mod world;

pub use bands::Bands;
pub use chunk::ChunkDag;
pub use compact::{BandWaste, CompactBand, CompactionStats, FrameGuard, RootGuard};
pub use edit::EditStats;
pub use invariants::{CheckReport, InvariantError, InvariantKind, check_bands};
pub use leaf::{
    AIR, LEAF_MATERIAL_BYTES, LEAF_RUN_BYTES, LEAF_RUN_MATERIALS, LeafInterner, fetch_leaf,
    leaf_index, leaf_slot,
};
pub use node_word::{Node, NodeRef};
pub use run::{
    CHILD_SLOTS, RunAllocation, RunAllocator, RunInterner, child_index, child_slot, fetch_child,
};

/// Runtime material id stored in every voxel. Air is id `0`.
///
/// Named `MaterialId`, not `BlockId`: the SVDAG uses a 64-bit `BlockId` for
/// tree node pointers, while this `u16` is the voxel payload.
pub use voxel_block::MaterialId;

pub use voxel_core::constants::{
    BLOCK_BITS, BLOCK_DIM, BLOCK_LEVEL, BLOCKS_PER_CHUNK, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME,
};

/// Bits per axis in a region's `8x8x8` chunk-cell index.
///
/// This is region packing, not chunk geometry: it stays 3 regardless of
/// `CHUNK_BITS`.
const REGION_CELL_BITS: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
    /// 8x8x8-chunk region containing this chunk.
    pub fn region(self) -> (i32, i32, i32) {
        (self.x >> 3, self.y >> 3, self.z >> 3)
    }
    /// Cell index of this chunk inside its region (0..512).
    pub fn cell(self) -> u16 {
        ((self.x & 7) | ((self.y & 7) << 3) | ((self.z & 7) << 6)) as u16
    }
    /// Inverse of [`Self::region`] and [`Self::cell`].
    ///
    /// Chunk coordinates are signed and use arithmetic shifts, so this also
    /// reconstructs chunks in negative regions.
    pub const fn from_region_cell(region: (i32, i32, i32), cell: u16) -> Self {
        debug_assert!(cell < 512, "a region has 8x8x8 cells");
        Self::new(
            (region.0 << REGION_CELL_BITS) | (cell & 7) as i32,
            (region.1 << REGION_CELL_BITS) | ((cell >> REGION_CELL_BITS) & 7) as i32,
            (region.2 << REGION_CELL_BITS) | ((cell >> (2 * REGION_CELL_BITS)) & 7) as i32,
        )
    }
}

/// Packs chunk-local coordinates into a row-major index at an arbitrary edge.
///
/// Production code uses [`CHUNK_BITS`]; the explicit parameter lets the
/// round-trip test cover both the 64³ baseline and the pre-registered 128³
/// target from one build.
#[inline]
const fn local_index_at(chunk_bits: u32, x: u32, y: u32, z: u32) -> u32 {
    x | (y << chunk_bits) | (z << (2 * chunk_bits))
}

/// Inverse of [`local_index_at`]: extracts the three `chunk_bits`-wide axes.
#[inline]
const fn local_xyz_at(chunk_bits: u32, i: u32) -> (u32, u32, u32) {
    let mask = (1u32 << chunk_bits) - 1;
    (i & mask, (i >> chunk_bits) & mask, i >> (2 * chunk_bits))
}

/// Packs chunk-local coordinates into a row-major index at [`CHUNK_BITS`].
#[inline]
pub fn local_index(x: u32, y: u32, z: u32) -> u32 {
    local_index_at(CHUNK_BITS, x, y, z)
}

/// Extracts `(x, y, z)` from a [`local_index`] at [`CHUNK_BITS`].
#[inline]
pub fn local_xyz(i: u32) -> (u32, u32, u32) {
    local_xyz_at(CHUNK_BITS, i)
}

/// Interleaves the coordinate bits of a local index into a Morton key at an
/// arbitrary edge.
///
/// Bit `b` of `x`, `y` and `z` lands at key bits `3b`, `3b+1` and `3b+2`.
#[inline]
const fn morton_at(chunk_bits: u32, i: u32) -> u32 {
    let (x, y, z) = local_xyz_at(chunk_bits, i);
    let mut key = 0u32;
    let mut bit = 0;
    while bit < chunk_bits {
        key |= ((x >> bit) & 1) << (3 * bit);
        key |= ((y >> bit) & 1) << (3 * bit + 1);
        key |= ((z >> bit) & 1) << (3 * bit + 2);
        bit += 1;
    }
    key
}

/// Inverse of [`morton_at`]: extracts the three axes from a Morton key.
#[cfg(test)]
#[inline]
const fn unmorton_at(chunk_bits: u32, key: u32) -> (u32, u32, u32) {
    let mut x = 0;
    let mut y = 0;
    let mut z = 0;
    let mut bit = 0;
    while bit < chunk_bits {
        x |= ((key >> (3 * bit)) & 1) << bit;
        y |= ((key >> (3 * bit + 1)) & 1) << bit;
        z |= ((key >> (3 * bit + 2)) & 1) << bit;
        bit += 1;
    }
    (x, y, z)
}

/// Interleaves a [`local_index`] into a Morton key at [`CHUNK_BITS`].
///
/// The key is the octree path to the voxel: the top `3 * (CHUNK_BITS - depth)`
/// bits identify the node at `depth`, so all descendants of a node occupy one
/// contiguous key range. Child `c` at pool level `l` is `(key >> 3 * (l - 1)) & 7`,
/// matching the node layout in [`dag`](crate::dag).
#[inline]
pub fn morton(i: u32) -> u32 {
    morton_at(CHUNK_BITS, i)
}

/// World voxel coords -> (chunk, local index). Arithmetic shifts handle negatives.
#[inline]
pub fn split_voxel(x: i32, y: i32, z: i32) -> (ChunkPos, u32) {
    let mask = CHUNK_DIM as i32 - 1;
    (
        ChunkPos::new(x >> CHUNK_BITS, y >> CHUNK_BITS, z >> CHUNK_BITS),
        local_index((x & mask) as u32, (y & mask) as u32, (z & mask) as u32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline and pre-registered target edges. Both are checked from one
    /// build so a formula error cannot hide behind the `chunk128` feature flag.
    const EDGE_BITS: [u32; 2] = [6, 7];

    #[test]
    fn index_roundtrip_covers_both_chunk_edges() {
        for bits in EDGE_BITS {
            let dim = 1u32 << bits;
            let volume = dim * dim * dim;
            for i in 0..volume {
                let (x, y, z) = local_xyz_at(bits, i);
                assert!(
                    x < dim && y < dim && z < dim,
                    "axis out of range at {bits} bits"
                );
                assert_eq!(local_index_at(bits, x, y, z), i);
                // The layout is dense row-major: x fastest, then y, then z.
                assert_eq!(i, x + y * dim + z * dim * dim);
            }
        }
    }

    #[test]
    fn local_index_uses_the_compiled_edge() {
        let dim = CHUNK_DIM as u32;
        for i in 0..CHUNK_VOLUME as u32 {
            let (x, y, z) = local_xyz(i);
            assert_eq!(local_index(x, y, z), i);
            assert_eq!(i, x + y * dim + z * dim * dim);
            assert_eq!((x, y, z), local_xyz_at(CHUNK_BITS, i));
            assert_eq!(local_index(x, y, z), local_index_at(CHUNK_BITS, x, y, z));
        }
    }

    #[test]
    fn morton_roundtrip_covers_both_chunk_edges() {
        for bits in EDGE_BITS {
            let dim = 1u32 << bits;
            let volume = dim * dim * dim;
            let key_space = 1u32 << (3 * bits);
            assert_eq!(volume, key_space);
            for i in 0..volume {
                let (x, y, z) = local_xyz_at(bits, i);
                let key = morton_at(bits, i);
                // Every index maps into the key space and decodes back to the
                // coordinates it started from.
                assert!(key < key_space, "key out of range at {bits} bits");
                assert_eq!(unmorton_at(bits, key), (x, y, z));
                assert_eq!(local_index_at(bits, x, y, z), i);
            }
        }
    }

    #[test]
    fn morton_locality_covers_both_chunk_edges() {
        for bits in EDGE_BITS {
            let dim = 1u32 << bits;
            for i in 0..dim * dim * dim {
                let key = morton_at(bits, i);
                let (x, y, z) = local_xyz_at(bits, i);
                for depth in 1..=bits {
                    let ancestor = local_index_at(bits, x >> depth, y >> depth, z >> depth);
                    let ancestor_key = morton_at(bits, ancestor);
                    // Dropping the low `3 * depth` bits leaves the ancestor's
                    // key, so a node's descendants form one contiguous range.
                    assert_eq!(key >> (3 * depth), ancestor_key);
                    assert_eq!(
                        unmorton_at(bits, key >> (3 * depth)),
                        (x >> depth, y >> depth, z >> depth)
                    );
                    let base = ancestor_key << (3 * depth);
                    let span = 1u32 << (3 * depth);
                    assert!(base <= key && key < base + span);
                }
            }
        }
    }

    #[test]
    fn morton_uses_the_compiled_edge() {
        // Anchors the interleave order: bit 0 of each axis is the low triplet.
        assert_eq!(morton(local_index(1, 2, 3)), 53);
        for i in 0..CHUNK_VOLUME as u32 {
            let (x, y, z) = local_xyz(i);
            assert_eq!(morton(i), morton_at(CHUNK_BITS, i));
            assert_eq!(unmorton_at(CHUNK_BITS, morton(i)), (x, y, z));
        }
    }

    #[test]
    fn region_cell_roundtrips_signed_chunks() {
        for (x, y, z) in [
            (0, 0, 0),
            (7, 7, 7),
            (8, 0, 0),
            (-1, 0, 0),
            (-8, -9, -64),
            (i32::MAX >> 3, i32::MIN >> 3, 12345),
        ] {
            let p = ChunkPos::new(x, y, z);
            assert_eq!(ChunkPos::from_region_cell(p.region(), p.cell()), p);
        }

        // Every cell maps back to the chunk it was packed from.
        for cell in 0..512u16 {
            let p = ChunkPos::from_region_cell((-2, 3, -4), cell);
            assert_eq!(p.region(), (-2, 3, -4));
            assert_eq!(p.cell(), cell);
        }
    }
}
