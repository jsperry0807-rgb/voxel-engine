//! Three-tier hierarchical sparse volumetric store.
//!
//! T1 `disk`    region files: append-only log of serialized SVDAGs + overlay snapshots
//! T2 `world`   FxMap<ChunkPos, Chunk{Arc<Svdag>, Overlay}>; edits hit the O(1) overlay only
//! T3 `gpu`     global deduped node pool + dynamic chunk hash table, batched COW consolidation
pub mod dag;
pub mod disk;
pub mod gpu;
pub mod hash;
pub mod overlay;
pub mod world;

/// Runtime material id stored in every voxel. Air is id `0`.
///
/// Named `MaterialId`, not `BlockId`: the SVDAG uses a 64-bit `BlockId` for
/// tree node pointers, while this `u16` is the voxel payload.
pub use voxel_block::MaterialId;

pub const CHUNK_BITS: u32 = 6;
pub const CHUNK_DIM: usize = 1 << CHUNK_BITS; // 64
pub const CHUNK_VOLUME: usize = CHUNK_DIM * CHUNK_DIM * CHUNK_DIM;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }
    /// 8x8x8-chunk region containing this chunk.
    pub fn region(self) -> (i32, i32, i32) { (self.x >> 3, self.y >> 3, self.z >> 3) }
    /// Cell index of this chunk inside its region (0..512).
    pub fn cell(self) -> u16 {
        ((self.x & 7) | ((self.y & 7) << 3) | ((self.z & 7) << 6)) as u16
    }
}

#[inline]
pub fn local_index(x: u32, y: u32, z: u32) -> u32 { x | (y << 6) | (z << 12) }
#[inline]
pub fn local_xyz(i: u32) -> (u32, u32, u32) { (i & 63, (i >> 6) & 63, i >> 12) }

/// World voxel coords -> (chunk, local index). Arithmetic shifts handle negatives.
#[inline]
pub fn split_voxel(x: i32, y: i32, z: i32) -> (ChunkPos, u32) {
    (
        ChunkPos::new(x >> 6, y >> 6, z >> 6),
        local_index((x & 63) as u32, (y & 63) as u32, (z & 63) as u32),
    )
}
