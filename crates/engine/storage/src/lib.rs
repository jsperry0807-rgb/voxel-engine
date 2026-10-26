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
pub mod leaf;
pub mod node_word;
pub mod overlay;
pub mod run;
pub mod serialize;
pub mod world;

pub use bands::Bands;
pub use chunk::ChunkDag;
pub use compact::{BandWaste, CompactBand, CompactionStats, FrameGuard, RootGuard};
pub use edit::EditStats;
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
    BLOCK_BITS, BLOCK_DIM, BLOCKS_PER_CHUNK, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME,
};

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
}

#[inline]
pub fn local_index(x: u32, y: u32, z: u32) -> u32 {
    x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS))
}
#[inline]
pub fn local_xyz(i: u32) -> (u32, u32, u32) {
    let mask = CHUNK_DIM as u32 - 1;
    (i & mask, (i >> CHUNK_BITS) & mask, i >> (2 * CHUNK_BITS))
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
