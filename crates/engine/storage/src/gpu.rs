//! Tier 3: GPU-resident SVDAG plus the edit overlay.
//!
//! * `pool`   : global u32 node pool, same layout as `dag.rs`, absolute offsets, shared by all chunks
//!   (cross-chunk dedup via content-hash table). Per-node refcounts live CPU-side only.
//! * `table`  : dynamic open-addressing chunk table, 5 u32 per slot `[x, y, z, root, blocks]`,
//!   root == 0xFFFFFFFF means "no DAG", blocks == 0xFFFFFFFF means "no overlay", and a
//!   tombstone is root == 0xFFFFFFFE. A slot is occupied when it has a DAG root or a block
//!   table, so a chunk whose edits carved every voxel out of an empty DAG is still visible.
//!   Power-of-two capacity, linear probe, load <= 0.5 (tombstones included),
//!   grows/rehashes -> full re-upload.
//! * overlay  : per edited 16^3 block, a 4 096-entry override buffer. A chunk's block table
//!   has [`BLOCKS_PER_CHUNK`] words (64 at 64³, 512 at 128³), one per block, each pointing
//!   at an 8 KiB overlay slot (2 048 words of packed `u16` material ids followed by 128
//!   words of presence bits). The slot is sized by the fixed block edge, never by the chunk
//!   edge. The bit, not the value, means "edited": a carved voxel stores material 0 and
//!   must shadow the DAG with air. Block tables and overlay slots are allocated lazily,
//!   uploaded as coalesced dirty ranges, and retired by epoch like nodes.
//! * COW      : `consolidate` imports each new chunk DAG into fresh/free slots (never in place),
//!   repoints the table root, and defers decref of the old root until `reclaim(epoch)`
//!   so in-flight frames never see freed nodes.
//! * upload   : dirty word ranges are coalesced and pushed through `GpuSink` in `flush`.
//!
//! WGSL probe (matches `slot_hash`):
//! ```wgsl
//! fn slot_hash(p: vec3<i32>) -> u32 {
//!   var h = u32(p.x)*73856093u ^ u32(p.y)*19349663u ^ u32(p.z)*83492791u;
//!   h ^= h>>16u; h *= 0x7feb352du; h ^= h>>15u; h *= 0x846ca68bu; h ^= h>>16u; return h;
//! }
//! // i = slot_hash(p) & (cap-1); loop { s = table[i*5..]; if s.root==EMPTY && s.blocks==NONE {miss}
//! //   if s.root!=TOMB && s.xyz==p {hit}; i=(i+1)&(cap-1); }
//! ```
use crate::ChunkPos;
use crate::dag::{NONE, ROOT_LEVEL, Svdag, dag_get, hdr_level, hdr_mask, header, morton};
use crate::hash::{FxMap, hash_words};
use crate::{BLOCK_BITS, BLOCK_DIM, CHUNK_BITS, CHUNK_VOLUME};
use crate::{local_index, local_xyz_at};

/// 16^3-voxel blocks per chunk, one block table word each.
///
/// Derived in `voxel-core` from the chunk and block edges: 64 at 64³ and 512
/// at the pre-registered 128³ target.
pub use crate::BLOCKS_PER_CHUNK;

/// No DAG attached to a chunk-table slot.
pub const EMPTY: u32 = u32::MAX;
/// A vacated chunk-table slot, distinguishable from an untouched [`EMPTY`] one
/// while probing past it.
pub const TOMB: u32 = u32::MAX - 1;
/// No overlay attached to a block-table entry.
pub const NO_OVERLAY: u32 = u32::MAX;
/// No block table attached to a chunk-table slot.
pub const NO_TABLE: u32 = u32::MAX;
/// Words per chunk-table slot: `[x, y, z, root, block_table]`.
pub const SLOT_WORDS: usize = 5;
/// Word offset of a slot's x coordinate.
pub const SLOT_X: usize = 0;
/// Word offset of a slot's y coordinate.
pub const SLOT_Y: usize = 1;
/// Word offset of a slot's z coordinate.
pub const SLOT_Z: usize = 2;
/// Word offset of a slot's DAG root.
pub const SLOT_ROOT: usize = 3;
/// Word offset of a slot's block-table slot.
pub const SLOT_TABLE: usize = 4;
/// Slots in a freshly allocated chunk table.
///
/// One slot holds one chunk at any chunk edge, so table capacity follows the
/// number of resident chunks, never [`CHUNK_VOLUME`] or [`BLOCKS_PER_CHUNK`].
pub const INITIAL_SLOTS: usize = 1024;

// The slot layout is a shader contract: positions first, then the DAG root and
// block-table words. Capacity is a power of two so probing can mask the hash,
// and the initial 1024 slots are independent of the compiled chunk edge.
const _: () = {
    assert!(SLOT_X == 0);
    assert!(SLOT_Y == SLOT_X + 1);
    assert!(SLOT_Z == SLOT_Y + 1);
    assert!(SLOT_ROOT == SLOT_Z + 1);
    assert!(SLOT_TABLE == SLOT_ROOT + 1);
    assert!(SLOT_WORDS == SLOT_TABLE + 1);
    assert!(INITIAL_SLOTS.is_power_of_two());
};

/// Base word of chunk-table slot `slot`.
#[inline]
const fn slot_base(slot: usize) -> usize {
    slot * SLOT_WORDS
}

/// Encodes one chunk coordinate axis into its slot word.
///
/// This is a two's-complement bit cast, matching the shader's
/// `bitcast<u32>(p.x)`: negative coordinates keep the same bits the GPU reads,
/// with no bias or sign extension.
#[inline]
const fn slot_coord(v: i32) -> u32 {
    v as u32
}

/// Decodes a chunk coordinate axis written by [`slot_coord`].
#[inline]
const fn coord_from_slot(word: u32) -> i32 {
    word as i32
}

/// Voxels in one block.
const BLOCK_VOLUME: usize = BLOCK_DIM * BLOCK_DIM * BLOCK_DIM;
/// Packed `u16` overlay values for one block, two voxels per word.
pub const OVERLAY_VALUES: usize = BLOCK_VOLUME / 2;
/// Overlay presence bits for one block, one per voxel.
pub const OVERLAY_BITS: usize = BLOCK_VOLUME / 32;
/// Words per block-overlay slot: values first, then the presence bitmap.
pub const OVERLAY_WORDS: usize = OVERLAY_VALUES + OVERLAY_BITS;

// Block-table sizing follows the chunk edge; overlay sizing follows only the
// fixed block edge. A 128³ chunk therefore gets four times as many blocks,
// never larger per-block overlay slots.
const _: () = {
    assert!(BLOCKS_PER_CHUNK == 1usize << (3 * (CHUNK_BITS - BLOCK_BITS)));
    assert!(BLOCKS_PER_CHUNK * BLOCK_VOLUME == CHUNK_VOLUME);
    assert!(OVERLAY_VALUES * 2 == BLOCK_VOLUME);
    assert!(OVERLAY_BITS * 32 == BLOCK_VOLUME);
};

/// Block index (0..`BLOCKS_PER_CHUNK`) inside a chunk at an arbitrary edge.
///
/// Blocks are ordered row-major over the block grid (`bx` fastest, then `by`,
/// then `bz`), so the block table stays one contiguous word per 16³ block.
#[inline]
const fn chunk_block_at(chunk_bits: u32, idx: u32) -> u32 {
    let (x, y, z) = local_xyz_at(chunk_bits, idx);
    let stride = chunk_bits - BLOCK_BITS;
    (x >> BLOCK_BITS) | ((y >> BLOCK_BITS) << stride) | ((z >> BLOCK_BITS) << (2 * stride))
}

/// Block index (0..[`BLOCKS_PER_CHUNK`]) for a chunk-local voxel index.
#[inline]
pub fn chunk_block(idx: u32) -> u32 {
    chunk_block_at(CHUNK_BITS, idx)
}

/// Voxel index (0..`BLOCK_VOLUME`) inside its block at an arbitrary edge.
#[inline]
const fn block_index_at(chunk_bits: u32, idx: u32) -> u32 {
    let (x, y, z) = local_xyz_at(chunk_bits, idx);
    let mask = BLOCK_DIM as u32 - 1;
    (x & mask) | ((y & mask) << BLOCK_BITS) | ((z & mask) << (2 * BLOCK_BITS))
}

/// Voxel index (0..`BLOCK_VOLUME`) inside its block for a chunk-local voxel index.
#[inline]
pub fn block_index(idx: u32) -> u32 {
    block_index_at(CHUNK_BITS, idx)
}

/// Word holding the packed `u16` value for `within`, relative to its overlay slot.
#[inline]
const fn overlay_value_word(within: u32) -> usize {
    (within >> 1) as usize
}

/// Bit offset of `within` inside its packed value word.
#[inline]
const fn overlay_value_shift(within: u32) -> u32 {
    (within & 1) * 16
}

/// Presence-bitmap word for `within`, relative to its overlay slot.
#[inline]
const fn overlay_bit_word(within: u32) -> usize {
    OVERLAY_VALUES + (within >> 5) as usize
}

/// Presence bit for `within` inside [`overlay_bit_word`].
#[inline]
const fn overlay_bit_mask(within: u32) -> u32 {
    1 << (within & 31)
}

/// Implemented by voxel-render over wgpu buffers. `resize_*` may reallocate and discard
/// contents; the store re-uploads everything right after any resize.
pub trait GpuSink {
    fn resize_nodes(&mut self, words: usize);
    fn write_nodes(&mut self, word_off: usize, data: &[u32]);
    fn resize_table(&mut self, words: usize);
    fn write_table(&mut self, word_off: usize, data: &[u32]);
    fn resize_overlays(&mut self, words: usize);
    fn write_overlays(&mut self, word_off: usize, data: &[u32]);
    fn resize_block_tables(&mut self, words: usize);
    fn write_block_tables(&mut self, word_off: usize, data: &[u32]);
}

/// In-memory sink for tests/headless.
#[derive(Default)]
pub struct MemSink {
    pub nodes: Vec<u32>,
    pub table: Vec<u32>,
    pub overlays: Vec<u32>,
    pub block_tables: Vec<u32>,
    pub bytes_written: usize,
}

impl GpuSink for MemSink {
    fn resize_nodes(&mut self, words: usize) {
        self.nodes.resize(words, 0)
    }
    fn write_nodes(&mut self, o: usize, d: &[u32]) {
        self.nodes[o..o + d.len()].copy_from_slice(d);
        self.bytes_written += d.len() * 4;
    }
    fn resize_table(&mut self, words: usize) {
        self.table.resize(words, 0)
    }
    fn write_table(&mut self, o: usize, d: &[u32]) {
        self.table[o..o + d.len()].copy_from_slice(d);
        self.bytes_written += d.len() * 4;
    }
    fn resize_overlays(&mut self, words: usize) {
        self.overlays.resize(words, 0)
    }
    fn write_overlays(&mut self, o: usize, d: &[u32]) {
        self.overlays[o..o + d.len()].copy_from_slice(d);
        self.bytes_written += d.len() * 4;
    }
    fn resize_block_tables(&mut self, words: usize) {
        self.block_tables.resize(words, NO_OVERLAY)
    }
    fn write_block_tables(&mut self, o: usize, d: &[u32]) {
        self.block_tables[o..o + d.len()].copy_from_slice(d);
        self.bytes_written += d.len() * 4;
    }
}

/// Hash of a chunk position, mixed per axis and avalanched.
///
/// The shader mirrors this bit for bit; both sides read the coordinate words
/// written by `slot_coord`.
pub fn slot_hash(p: ChunkPos) -> u32 {
    let mut h = slot_coord(p.x).wrapping_mul(73856093)
        ^ slot_coord(p.y).wrapping_mul(19349663)
        ^ slot_coord(p.z).wrapping_mul(83492791);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846ca68b);
    h ^ (h >> 16)
}

pub struct GpuStore {
    pool: Vec<u32>,
    rc: Vec<u32>,
    free: [Vec<u32>; 10], // by node size in words (2..=9)
    dedup: FxMap<u64, u32>,
    /// Additional live nodes sharing a hash with the [`Self::dedup`] entry.
    ///
    /// A content hash is not an identity: when two different nodes collide,
    /// both must stay addressable or every later insert of the colliding
    /// content allocates again and the pool grows without bound.
    dedup_more: FxMap<u64, Vec<u32>>,
    table: Vec<u32>,
    overlays: Vec<u32>,
    overlay_counts: Vec<u32>,
    overlay_free: Vec<u32>,
    block_tables: Vec<u32>,
    table_counts: Vec<u32>,
    table_free: Vec<u32>,
    live: usize,
    tombs: usize,
    node_dirty: Vec<(usize, usize)>,
    table_dirty: Vec<(usize, usize)>,
    overlay_dirty: Vec<(usize, usize)>,
    block_table_dirty: Vec<(usize, usize)>,
    gpu_node_cap: usize,
    gpu_table_cap: usize,
    gpu_overlay_cap: usize,
    gpu_block_table_cap: usize,
    retire: Vec<(u64, u32)>,
    retire_overlay: Vec<(u64, u32)>,
    retire_table: Vec<(u64, u32)>,
    epoch: u64,
    import_memo: FxMap<u32, u32>,
}

fn coalesce(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    v.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (s, l) in v {
        match out.last_mut() {
            Some(last) if s <= last.0 + last.1 => last.1 = last.1.max(s + l - last.0),
            _ => out.push((s, l)),
        }
    }
    out
}

impl Default for GpuStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuStore {
    pub fn new() -> Self {
        Self {
            pool: Vec::new(),
            rc: Vec::new(),
            free: Default::default(),
            dedup: FxMap::default(),
            dedup_more: FxMap::default(),
            table: vec![EMPTY; INITIAL_SLOTS * SLOT_WORDS],
            overlays: Vec::new(),
            overlay_counts: Vec::new(),
            overlay_free: Vec::new(),
            block_tables: Vec::new(),
            table_counts: Vec::new(),
            table_free: Vec::new(),
            live: 0,
            tombs: 0,
            node_dirty: Vec::new(),
            table_dirty: vec![(0, INITIAL_SLOTS * SLOT_WORDS)],
            overlay_dirty: Vec::new(),
            block_table_dirty: Vec::new(),
            gpu_node_cap: 0,
            gpu_table_cap: 0,
            gpu_overlay_cap: 0,
            gpu_block_table_cap: 0,
            retire: Vec::new(),
            retire_overlay: Vec::new(),
            retire_table: Vec::new(),
            epoch: 0,
            import_memo: FxMap::default(),
        }
    }

    // ---------- chunk table ----------

    fn table_slots(&self) -> usize {
        self.table.len() / SLOT_WORDS
    }

    fn probe(&self, p: ChunkPos) -> Result<usize, usize> {
        let mask = self.table_slots() - 1;
        let mut i = slot_hash(p) as usize & mask;
        let mut tomb = None;
        loop {
            let b = slot_base(i);
            let root = self.table[b + SLOT_ROOT];
            let blocks = self.table[b + SLOT_TABLE];
            if root == TOMB {
                tomb.get_or_insert(i);
            } else if root == EMPTY && blocks == NO_TABLE {
                return Err(tomb.unwrap_or(i));
            } else if self.table[b + SLOT_X] == slot_coord(p.x)
                && self.table[b + SLOT_Y] == slot_coord(p.y)
                && self.table[b + SLOT_Z] == slot_coord(p.z)
            {
                return Ok(i);
            }
            i = (i + 1) & mask;
        }
    }

    /// Grows or compacts the table.
    ///
    /// The capacity policy is a function of the number of occupied slots only,
    /// so the table grows identically at every chunk edge. If live occupancy is
    /// at most a quarter, the slot count is kept and tombstones are dropped;
    /// otherwise the table doubles.
    fn rehash(&mut self) {
        let cap = self.table_slots();
        let new_cap = if self.live * 4 > cap { cap * 2 } else { cap };
        let old = std::mem::replace(&mut self.table, vec![EMPTY; new_cap * SLOT_WORDS]);
        self.live = 0;
        self.tombs = 0;
        for s in old.as_chunks::<SLOT_WORDS>().0 {
            let root = s[SLOT_ROOT];
            let occupied = root != TOMB && (root != EMPTY || s[SLOT_TABLE] != NO_TABLE);
            if occupied {
                let p = ChunkPos::new(
                    coord_from_slot(s[SLOT_X]),
                    coord_from_slot(s[SLOT_Y]),
                    coord_from_slot(s[SLOT_Z]),
                );
                let i = self.probe(p).unwrap_err();
                let b = slot_base(i);
                self.table[b..b + SLOT_WORDS].copy_from_slice(s);
                self.live += 1;
            }
        }
        self.table_dirty.clear();
        self.table_dirty.push((0, self.table.len()));
    }

    /// Room check plus probe, so callers can tell `Ok(existing)` from `Err(insert_at)`.
    fn ensure_slot(&mut self, p: ChunkPos) -> Result<usize, usize> {
        if (self.live + self.tombs + 1) * 2 > self.table_slots() {
            self.rehash();
        }
        self.probe(p)
    }

    fn insert_slot_at(&mut self, i: usize, p: ChunkPos, root: u32, blocks: u32) {
        let b = slot_base(i);
        if self.table[b + SLOT_ROOT] == TOMB {
            self.tombs -= 1;
        }
        self.table[b..b + SLOT_WORDS].copy_from_slice(&[
            slot_coord(p.x),
            slot_coord(p.y),
            slot_coord(p.z),
            root,
            blocks,
        ]);
        self.live += 1;
        self.table_dirty.push((b, SLOT_WORDS));
    }

    fn tombstone_at(&mut self, i: usize) {
        let b = slot_base(i);
        self.table[b + SLOT_ROOT] = TOMB;
        self.table[b + SLOT_TABLE] = NO_TABLE;
        self.live -= 1;
        self.tombs += 1;
        self.table_dirty.push((b + SLOT_ROOT, 2));
    }

    fn retire_root(&mut self, old: u32) {
        if old < TOMB {
            self.retire.push((self.epoch, old));
        }
    }

    /// Writes the DAG root for `p`, inserting a slot if needed. An empty root keeps the
    /// slot alive only while an overlay exists.
    fn table_set_root(&mut self, p: ChunkPos, root: u32) -> Option<u32> {
        match self.ensure_slot(p) {
            Ok(i) => {
                let b = slot_base(i);
                let old = self.table[b + SLOT_ROOT];
                self.table[b + SLOT_ROOT] = root;
                self.table_dirty.push((b + SLOT_ROOT, 1));
                if root == EMPTY && self.table[b + SLOT_TABLE] == NO_TABLE {
                    self.tombstone_at(i);
                }
                Some(old)
            }
            Err(i) => {
                if root == EMPTY {
                    return None;
                }
                self.insert_slot_at(i, p, root, NO_TABLE);
                None
            }
        }
    }

    // ---------- per-chunk block tables, then per-block overlays ----------

    fn alloc_table(&mut self) -> u32 {
        let slot = match self.table_free.pop() {
            Some(s) => s,
            None => {
                let s = (self.block_tables.len() / BLOCKS_PER_CHUNK) as u32;
                self.block_tables
                    .resize(self.block_tables.len() + BLOCKS_PER_CHUNK, NO_OVERLAY);
                self.table_counts.push(0);
                s
            }
        };
        let base = slot as usize * BLOCKS_PER_CHUNK;
        self.block_tables[base..base + BLOCKS_PER_CHUNK].fill(NO_OVERLAY);
        self.table_counts[slot as usize] = 0;
        self.block_table_dirty.push((base, BLOCKS_PER_CHUNK));
        slot
    }

    fn alloc_overlay(&mut self) -> u32 {
        let slot = match self.overlay_free.pop() {
            Some(s) => s,
            None => {
                let s = (self.overlays.len() / OVERLAY_WORDS) as u32;
                self.overlays.resize(self.overlays.len() + OVERLAY_WORDS, 0);
                self.overlay_counts.push(0);
                s
            }
        };
        let base = slot as usize * OVERLAY_WORDS;
        // A recycled slot must start with an empty bitmap, or stale presence bits
        // would shadow DAG voxels that were never edited.
        self.overlays[base..base + OVERLAY_WORDS].fill(0);
        self.overlay_counts[slot as usize] = 0;
        self.overlay_dirty.push((base, OVERLAY_WORDS));
        slot
    }

    /// Chunk-table index plus the chunk's block-table slot, allocating either if needed.
    fn ensure_block_table(&mut self, p: ChunkPos) -> (usize, u32) {
        match self.ensure_slot(p) {
            Ok(i) => {
                let b = slot_base(i);
                if self.table[b + SLOT_TABLE] == NO_TABLE {
                    let slot = self.alloc_table();
                    self.table[b + SLOT_TABLE] = slot;
                    self.table_dirty.push((b + SLOT_TABLE, 1));
                }
                (i, self.table[b + SLOT_TABLE])
            }
            Err(i) => {
                let slot = self.alloc_table();
                self.insert_slot_at(i, p, EMPTY, slot);
                (i, slot)
            }
        }
    }

    fn overlay_set(&mut self, slot: u32, idx: u32, v: u16) {
        let base = slot as usize * OVERLAY_WORDS;
        let word = base + overlay_value_word(idx);
        let shift = overlay_value_shift(idx);
        let old = self.overlays[word];
        let new = (old & !(0xffff << shift)) | ((v as u32) << shift);
        if new != old {
            self.overlays[word] = new;
            self.overlay_dirty.push((word, 1));
        }

        let bit_word = base + overlay_bit_word(idx);
        let bit = overlay_bit_mask(idx);
        if self.overlays[bit_word] & bit == 0 {
            self.overlays[bit_word] |= bit;
            self.overlay_dirty.push((bit_word, 1));
            self.overlay_counts[slot as usize] += 1;
        }
    }

    /// Returns true when that was the last presence bit in the block slot.
    fn overlay_clear(&mut self, slot: u32, idx: u32) -> bool {
        let base = slot as usize * OVERLAY_WORDS;
        let bit_word = base + overlay_bit_word(idx);
        let bit = overlay_bit_mask(idx);
        if self.overlays[bit_word] & bit != 0 {
            self.overlays[bit_word] &= !bit;
            self.overlay_dirty.push((bit_word, 1));
            self.overlay_counts[slot as usize] -= 1;
        }
        self.overlay_counts[slot as usize] == 0
    }

    fn set_in_table(&mut self, table_slot: u32, idx: u32, v: u16) {
        assert!((idx as usize) < CHUNK_VOLUME, "voxel index out of range");
        let bt = table_slot as usize * BLOCKS_PER_CHUNK + chunk_block(idx) as usize;
        let slot = match self.block_tables[bt] {
            NO_OVERLAY => {
                let slot = self.alloc_overlay();
                self.block_tables[bt] = slot;
                self.block_table_dirty.push((bt, 1));
                self.table_counts[table_slot as usize] += 1;
                slot
            }
            slot => slot,
        };
        self.overlay_set(slot, block_index(idx), v);
    }

    fn clear_in_table(&mut self, i: usize, table_slot: u32, idx: u32) {
        assert!((idx as usize) < CHUNK_VOLUME, "voxel index out of range");
        let bt = table_slot as usize * BLOCKS_PER_CHUNK + chunk_block(idx) as usize;
        let slot = self.block_tables[bt];
        if slot == NO_OVERLAY {
            return;
        }
        if self.overlay_clear(slot, block_index(idx)) {
            self.retire_overlay.push((self.epoch, slot));
            self.overlay_counts[slot as usize] = 0;
            self.block_tables[bt] = NO_OVERLAY;
            self.block_table_dirty.push((bt, 1));
            self.table_counts[table_slot as usize] -= 1;
            if self.table_counts[table_slot as usize] == 0 {
                self.drop_table(i, table_slot);
            }
        }
    }

    fn drop_table_slots(&mut self, table_slot: u32) {
        let base = table_slot as usize * BLOCKS_PER_CHUNK;
        for k in 0..BLOCKS_PER_CHUNK {
            let slot = self.block_tables[base + k];
            if slot != NO_OVERLAY {
                self.retire_overlay.push((self.epoch, slot));
                self.overlay_counts[slot as usize] = 0;
                self.block_tables[base + k] = NO_OVERLAY;
                self.block_table_dirty.push((base + k, 1));
            }
        }
        self.table_counts[table_slot as usize] = 0;
        self.retire_table.push((self.epoch, table_slot));
    }

    fn drop_table(&mut self, i: usize, table_slot: u32) {
        self.drop_table_slots(table_slot);
        let b = slot_base(i);
        if self.table[b + SLOT_ROOT] == EMPTY {
            self.tombstone_at(i);
        } else {
            self.table[b + SLOT_TABLE] = NO_TABLE;
            self.table_dirty.push((b + SLOT_TABLE, 1));
        }
    }

    fn drop_chunk_overlay(&mut self, i: usize) {
        let b = slot_base(i);
        let table_slot = self.table[b + SLOT_TABLE];
        if table_slot == NO_TABLE {
            return;
        }
        self.drop_table(i, table_slot);
    }

    // ---------- node pool ----------

    fn alloc(&mut self, words: &[u32]) -> u32 {
        let n = words.len();
        let o = match self.free[n].pop() {
            Some(o) => o as usize,
            None => {
                let o = self.pool.len();
                self.pool.resize(o + n, 0);
                self.rc.resize(o + n, 0);
                o
            }
        };
        self.pool[o..o + n].copy_from_slice(words);
        self.rc[o] = 0;
        self.node_dirty.push((o, n));
        o as u32
    }

    /// returns (offset, newly_created)
    fn intern(&mut self, words: &[u32]) -> (u32, bool) {
        let h = hash_words(words);
        let n = words.len();
        if let Some(&o) = self.dedup.get(&h) {
            let u = o as usize;
            if u + n <= self.pool.len() && self.pool[u..u + n] == *words {
                return (o, false);
            }
            // Hash collision: reuse a sibling with identical content instead of
            // allocating a private copy (which would repeat on every insert).
            if let Some(extras) = self.dedup_more.get(&h) {
                for &candidate in extras {
                    let u = candidate as usize;
                    if u + n <= self.pool.len() && self.pool[u..u + n] == *words {
                        return (candidate, false);
                    }
                }
            }
            let o = self.alloc(words);
            self.dedup_more.entry(h).or_default().push(o);
            return (o, true);
        }
        let o = self.alloc(words);
        self.dedup.insert(h, o);
        (o, true)
    }

    fn import(&mut self, src: &[u32], node: u32, memo: &mut FxMap<u32, u32>) -> u32 {
        if let Some(&g) = memo.get(&node) {
            return g;
        }
        let n = node as usize;
        let hdr = src[n];
        let cnt = hdr_mask(hdr).count_ones() as usize;
        let level = hdr_level(hdr);
        let mut words = [0u32; 9];
        words[0] = hdr;
        for k in 0..cnt {
            let w = src[n + 1 + k];
            words[1 + k] = if level == 1 {
                w
            } else {
                self.import(src, w, memo)
            };
        }
        let (g, created) = self.intern(&words[..=cnt]);
        if created && level > 1 {
            for k in 0..cnt {
                self.rc[words[1 + k] as usize] += 1;
            }
        }
        memo.insert(node, g);
        g
    }

    fn decref(&mut self, off: u32) {
        let o = off as usize;
        let Some(rc) = self.rc.get_mut(o) else {
            debug_assert!(false, "refcount for node {o} is out of range");
            return;
        };
        let Some(next) = rc.checked_sub(1) else {
            // An unbalanced retire must never hand a live node to the allocator.
            debug_assert!(false, "refcount underflow for node {o}");
            return;
        };
        *rc = next;
        if next == 0 {
            self.free_node(o);
        }
    }

    fn free_node(&mut self, o: usize) {
        let hdr = self.pool[o];
        let cnt = hdr_mask(hdr).count_ones() as usize;
        let n = cnt + 1;
        let h = hash_words(&self.pool[o..o + n]);
        if self.dedup.get(&h) == Some(&(o as u32)) {
            self.dedup.remove(&h);
            // Promote a colliding sibling so lookups still find a live node.
            if let Some(extras) = self.dedup_more.get_mut(&h)
                && let Some(next) = extras.pop()
            {
                self.dedup.insert(h, next);
            }
        } else if let Some(extras) = self.dedup_more.get_mut(&h) {
            extras.retain(|&candidate| candidate as usize != o);
            if extras.is_empty() {
                self.dedup_more.remove(&h);
            }
        }
        if hdr_level(hdr) > 1 {
            for k in 0..cnt {
                let c = self.pool[o + 1 + k];
                self.decref(c);
            }
        }
        self.free[n].push(o as u32);
    }

    // ---------- public API ----------

    /// Point `p` at `dag` (copy-on-write). An empty DAG clears the root but keeps the chunk
    /// visible while it still has an overlay.
    pub fn set_chunk(&mut self, p: ChunkPos, dag: &Svdag) {
        let root = if dag.is_empty() {
            EMPTY
        } else {
            let mut memo = std::mem::take(&mut self.import_memo);
            memo.clear();
            let g = self.import(&dag.pool, dag.root, &mut memo);
            self.import_memo = memo;
            self.rc[g as usize] += 1;
            g
        };
        if let Some(old) = self.table_set_root(p, root) {
            self.retire_root(old);
        }
    }

    /// Records one `(local voxel index, material)` override and uploads it on the next `flush`.
    ///
    /// The overlay shadows the DAG, so the edit is visible to the next frame without a
    /// consolidation pass. Material `0` carves the voxel to air; the presence bit, not the
    /// value, marks it as edited. Storage is allocated per 16^3 block, so a single edit costs
    /// one 8 KiB overlay slot, not one per chunk.
    pub fn set_overlay(&mut self, p: ChunkPos, idx: u32, v: u16) {
        assert!((idx as usize) < CHUNK_VOLUME, "voxel index out of range");
        let (_, table_slot) = self.ensure_block_table(p);
        self.set_in_table(table_slot, idx, v);
    }

    /// Removes one override, falling back to the DAG value. Frees the block overlay when the
    /// last entry in that block goes away, and the chunk-table slot when it has no DAG.
    pub fn clear_overlay(&mut self, p: ChunkPos, idx: u32) {
        assert!((idx as usize) < CHUNK_VOLUME, "voxel index out of range");
        let Ok(i) = self.probe(p) else {
            return;
        };
        let table_slot = self.table[slot_base(i) + SLOT_TABLE];
        if table_slot == NO_TABLE {
            return;
        }
        self.clear_in_table(i, table_slot, idx);
    }

    /// Installs a whole overlay snapshot, e.g. when a chunk is loaded from disk.
    pub fn set_chunk_overlay(&mut self, p: ChunkPos, edits: impl IntoIterator<Item = (u32, u16)>) {
        let mut edits = edits.into_iter();
        let Some((first_idx, first_value)) = edits.next() else {
            return;
        };
        assert!(
            (first_idx as usize) < CHUNK_VOLUME,
            "voxel index out of range"
        );
        let (_, table_slot) = self.ensure_block_table(p);
        self.set_in_table(table_slot, first_idx, first_value);
        for (idx, v) in edits {
            self.set_in_table(table_slot, idx, v);
        }
    }

    /// Drops the overlay without folding it into the DAG. Used after `apply_edits` has folded
    /// it, and by eviction.
    pub fn clear_chunk_overlay(&mut self, p: ChunkPos) {
        if let Ok(i) = self.probe(p) {
            self.drop_chunk_overlay(i);
        }
    }

    /// Drops the chunk, its DAG root and its overlay. In-flight frames keep reading the old
    /// nodes and overlay until `reclaim(epoch)`.
    pub fn remove_chunk(&mut self, p: ChunkPos) {
        let Ok(i) = self.probe(p) else {
            return;
        };
        let b = slot_base(i);
        self.retire_root(self.table[b + SLOT_ROOT]);
        let table_slot = self.table[b + SLOT_TABLE];
        if table_slot != NO_TABLE {
            self.drop_table_slots(table_slot);
        }
        self.tombstone_at(i);
    }

    /// In-place COW edit of a chunk already (or not yet) in the pool: only the paths to edited
    /// voxels are rebuilt, untouched subtrees are reused by offset without being walked.
    /// `edits` are (local voxel index, material; 0 = air) applied on top of the chunk's current
    /// GPU DAG (air if the chunk is absent). Cost is O(edits * 6), independent of chunk size.
    pub fn apply_edits(&mut self, p: ChunkPos, edits: impl IntoIterator<Item = (u32, u16)>) {
        let mut e: Vec<(u32, u16)> = edits.into_iter().map(|(i, v)| (morton(i), v)).collect();
        if e.is_empty() {
            return;
        }
        e.sort_unstable_by_key(|x| x.0);
        let old = self
            .probe(p)
            .ok()
            .map(|i| self.table[slot_base(i) + SLOT_ROOT])
            .filter(|&root| root != EMPTY);
        match self.gedit(old, ROOT_LEVEL, &e) {
            Some(g) => {
                self.rc[g as usize] += 1;
                if let Some(o) = self.table_set_root(p, g) {
                    self.retire_root(o);
                }
            }
            None => {
                if let Some(o) = self.table_set_root(p, EMPTY) {
                    self.retire_root(o);
                }
            }
        }
    }

    fn gedit(&mut self, node: Option<u32>, level: u32, edits: &[(u32, u16)]) -> Option<u32> {
        if edits.is_empty() {
            return node; // untouched subtree: reuse as-is, no walk, no refcount change here
        }
        let shift = 3 * (level - 1);
        let (omask, obase) = match node {
            Some(n) => (hdr_mask(self.pool[n as usize]), n as usize + 1),
            None => (0, 0),
        };
        let mut words = [0u32; 9];
        let mut mask = 0u32;
        let mut cnt = 0usize;
        let mut rest = edits;
        for c in 0..8u32 {
            let split = rest.partition_point(|e| ((e.0 >> shift) & 7) == c);
            let (sub, r) = rest.split_at(split);
            rest = r;
            let ow = if (omask >> c) & 1 == 1 {
                let k = (omask & ((1 << c) - 1)).count_ones() as usize;
                Some(self.pool[obase + k])
            } else {
                None
            };
            let res = if level == 1 {
                match sub.last() {
                    Some(&(_, v)) => (v != 0).then_some(v as u32),
                    None => ow,
                }
            } else {
                self.gedit(ow, level - 1, sub)
            };
            if let Some(w) = res {
                mask |= 1 << c;
                cnt += 1;
                words[cnt] = w;
            }
        }
        if mask == 0 {
            return None;
        }
        words[0] = header(mask, level);
        let (g, created) = self.intern(&words[..=cnt]);
        if created && level > 1 {
            for k in 0..cnt {
                self.rc[words[1 + k] as usize] += 1;
            }
        }
        Some(g)
    }

    /// Close a batch: returns the epoch tag for `reclaim`, then advances the epoch.
    pub fn end_batch(&mut self) -> u64 {
        let tag = self.epoch;
        self.epoch += 1;
        tag
    }

    /// Batched COW consolidation. Returns the epoch tag of the retirees from this batch:
    /// call `reclaim(tag)` once every frame submitted before the following `flush` has completed.
    pub fn consolidate<'a>(
        &mut self,
        batch: impl IntoIterator<Item = (ChunkPos, &'a Svdag)>,
    ) -> u64 {
        for (p, d) in batch {
            self.set_chunk(p, d);
        }
        self.end_batch()
    }

    /// Free nodes and overlays retired at epochs <= `completed`.
    pub fn reclaim(&mut self, completed: u64) {
        let mut i = 0;
        while i < self.retire.len() {
            if self.retire[i].0 <= completed {
                let (_, root) = self.retire.swap_remove(i);
                self.decref(root);
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.retire_overlay.len() {
            if self.retire_overlay[i].0 <= completed {
                let (_, slot) = self.retire_overlay.swap_remove(i);
                self.overlay_free.push(slot);
            } else {
                i += 1;
            }
        }
        let mut i = 0;
        while i < self.retire_table.len() {
            if self.retire_table[i].0 <= completed {
                let (_, slot) = self.retire_table.swap_remove(i);
                self.table_free.push(slot);
            } else {
                i += 1;
            }
        }
    }

    pub fn flush<S: GpuSink>(&mut self, sink: &mut S) {
        if self.pool.len() > self.gpu_node_cap {
            let cap = self.pool.len().next_power_of_two().max(1 << 16);
            sink.resize_nodes(cap);
            self.gpu_node_cap = cap;
            self.node_dirty.clear();
            self.node_dirty.push((0, self.pool.len()));
        }
        if self.table.len() > self.gpu_table_cap {
            sink.resize_table(self.table.len());
            self.gpu_table_cap = self.table.len();
            self.table_dirty.clear();
            self.table_dirty.push((0, self.table.len()));
        }
        if self.overlays.len() > self.gpu_overlay_cap {
            let cap = self.overlays.len().next_power_of_two().max(1 << 12);
            sink.resize_overlays(cap);
            self.gpu_overlay_cap = cap;
            self.overlay_dirty.clear();
            self.overlay_dirty.push((0, self.overlays.len()));
        }
        if self.block_tables.len() > self.gpu_block_table_cap {
            let cap = self.block_tables.len().next_power_of_two().max(1 << 10);
            sink.resize_block_tables(cap);
            self.gpu_block_table_cap = cap;
            self.block_table_dirty.clear();
            self.block_table_dirty.push((0, self.block_tables.len()));
        }
        for (s, l) in coalesce(std::mem::take(&mut self.node_dirty)) {
            sink.write_nodes(s, &self.pool[s..s + l]);
        }
        for (s, l) in coalesce(std::mem::take(&mut self.table_dirty)) {
            sink.write_table(s, &self.table[s..s + l]);
        }
        for (s, l) in coalesce(std::mem::take(&mut self.overlay_dirty)) {
            sink.write_overlays(s, &self.overlays[s..s + l]);
        }
        for (s, l) in coalesce(std::mem::take(&mut self.block_table_dirty)) {
            sink.write_block_tables(s, &self.block_tables[s..s + l]);
        }
    }

    /// CPU replica of the shader traversal (tests / CPU fallback renderer).
    pub fn lookup(&self, p: ChunkPos, x: u32, y: u32, z: u32) -> u16 {
        match self.probe(p) {
            Ok(i) => {
                let b = slot_base(i);
                let table_slot = self.table[b + SLOT_TABLE];
                if table_slot != NO_TABLE {
                    let idx = local_index(x, y, z);
                    let bt = table_slot as usize * BLOCKS_PER_CHUNK + chunk_block(idx) as usize;
                    let slot = self.block_tables[bt];
                    if slot != NO_OVERLAY {
                        let within = block_index(idx);
                        let base = slot as usize * OVERLAY_WORDS;
                        if self.overlays[base + overlay_bit_word(within)] & overlay_bit_mask(within)
                            != 0
                        {
                            let word = base + overlay_value_word(within);
                            return (self.overlays[word] >> overlay_value_shift(within)) as u16;
                        }
                    }
                }
                dag_get(&self.pool, self.table[b + SLOT_ROOT], x, y, z)
            }
            Err(_) => 0,
        }
    }

    pub fn contains(&self, p: ChunkPos) -> bool {
        self.probe(p).is_ok()
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn pool(&self) -> &[u32] {
        &self.pool
    }
    pub fn table(&self) -> &[u32] {
        &self.table
    }
    pub fn overlays(&self) -> &[u32] {
        &self.overlays
    }
    pub fn block_tables(&self) -> &[u32] {
        &self.block_tables
    }
    pub fn unique_nodes(&self) -> usize {
        self.dedup.len() + self.dedup_more.values().map(Vec::len).sum::<usize>()
    }
    pub fn chunk_count(&self) -> usize {
        self.live
    }
    /// The chunk's block-table slot, or `None` when it has no overlay at all.
    pub fn block_table_slot(&self, p: ChunkPos) -> Option<u32> {
        match self.probe(p) {
            Ok(i) => {
                let slot = self.table[slot_base(i) + SLOT_TABLE];
                (slot != NO_TABLE).then_some(slot)
            }
            Err(_) => None,
        }
    }
    /// The block-overlay slot for the 16^3 block containing `idx`, if edited.
    pub fn block_overlay_slot(&self, p: ChunkPos, idx: u32) -> Option<u32> {
        let table_slot = self.block_table_slot(p)?;
        let bt = table_slot as usize * BLOCKS_PER_CHUNK + chunk_block(idx) as usize;
        let slot = self.block_tables[bt];
        (slot != NO_OVERLAY).then_some(slot)
    }
    pub fn root_of(&self, p: ChunkPos) -> u32 {
        self.probe(p).map_or(NONE, |i| {
            let root = self.table[slot_base(i) + SLOT_ROOT];
            if root == EMPTY { NONE } else { root }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_index_at;

    /// The 64³ baseline and the pre-registered 128³ target, both checked from
    /// one build so an edge-dependent bug cannot hide behind `chunk128`.
    const EDGE_BITS: [u32; 2] = [6, 7];

    const fn blocks_per_chunk_at(chunk_bits: u32) -> usize {
        1usize << (3 * (chunk_bits - BLOCK_BITS))
    }

    #[test]
    fn block_index_matches_cpu_at_both_chunk_edges() {
        assert_eq!(BLOCKS_PER_CHUNK, blocks_per_chunk_at(CHUNK_BITS));

        for bits in EDGE_BITS {
            let dim = 1u32 << bits;
            let stride = bits - BLOCK_BITS;
            let block_mask = (1u32 << stride) - 1;
            let voxel_mask = BLOCK_DIM as u32 - 1;
            let blocks = blocks_per_chunk_at(bits) as u32;
            assert_eq!(blocks, 1u32 << (3 * stride));

            for i in 0..dim * dim * dim {
                let (x, y, z) = crate::local_xyz_at(bits, i);
                let block = chunk_block_at(bits, i);
                let within = block_index_at(bits, i);

                assert!(block < blocks, "block out of range at {bits} bits");
                assert!((within as usize) < BLOCK_VOLUME);

                let bx = block & block_mask;
                let by = (block >> stride) & block_mask;
                let bz = block >> (2 * stride);
                let wx = within & voxel_mask;
                let wy = (within >> BLOCK_BITS) & voxel_mask;
                let wz = within >> (2 * BLOCK_BITS);

                // The GPU's (block, within) pair is exactly the CPU local
                // index split into block coordinates and a block-local voxel.
                assert_eq!(
                    (bx, by, bz),
                    (x >> BLOCK_BITS, y >> BLOCK_BITS, z >> BLOCK_BITS)
                );
                assert_eq!(
                    (wx, wy, wz),
                    (x & voxel_mask, y & voxel_mask, z & voxel_mask)
                );
                assert_eq!(
                    local_index_at(
                        bits,
                        (bx << BLOCK_BITS) | wx,
                        (by << BLOCK_BITS) | wy,
                        (bz << BLOCK_BITS) | wz
                    ),
                    i
                );

                // The overlay slot holds one fixed 16³ block, so both word
                // ranges stay inside it at either edge.
                let base = block as usize * OVERLAY_WORDS;
                let value_word = base + overlay_value_word(within);
                let bit_word = base + overlay_bit_word(within);
                assert!((base..base + OVERLAY_WORDS).contains(&value_word));
                assert!((base..base + OVERLAY_WORDS).contains(&bit_word));
                assert!(value_word < base + OVERLAY_VALUES);
                assert!(bit_word >= base + OVERLAY_VALUES);
            }
        }
    }

    #[test]
    fn block_index_overlay_words_stay_fixed_per_block() {
        // 16³ = 4096 voxels: 2048 packed u16 words + 128 presence words.
        assert_eq!(BLOCK_VOLUME, 4096);
        assert_eq!(OVERLAY_VALUES, 2048);
        assert_eq!(OVERLAY_BITS, 128);
        assert_eq!(OVERLAY_WORDS, 2176);

        let mut value_slots = vec![false; BLOCK_VOLUME];
        let mut bit_slots = vec![false; BLOCK_VOLUME];
        for within in 0..BLOCK_VOLUME as u32 {
            let value =
                overlay_value_word(within) * 2 + (overlay_value_shift(within) / 16) as usize;
            assert!(value < BLOCK_VOLUME);
            assert!(!value_slots[value], "value slot reused at {within}");
            value_slots[value] = true;

            let bit_word = overlay_bit_word(within);
            assert!((OVERLAY_VALUES..OVERLAY_WORDS).contains(&bit_word));
            let bit = (bit_word - OVERLAY_VALUES) * 32
                + overlay_bit_mask(within).trailing_zeros() as usize;
            assert!(bit < BLOCK_VOLUME);
            assert!(!bit_slots[bit], "presence bit reused at {within}");
            bit_slots[bit] = true;
        }

        assert!(value_slots.into_iter().all(|used| used));
        assert!(bit_slots.into_iter().all(|used| used));
        assert_eq!(
            overlay_value_word(BLOCK_VOLUME as u32 - 1),
            OVERLAY_VALUES - 1
        );
        assert_eq!(overlay_bit_word(0), OVERLAY_VALUES);
        assert_eq!(overlay_bit_word(BLOCK_VOLUME as u32 - 1), OVERLAY_WORDS - 1);
    }

    #[test]
    fn block_index_uses_the_compiled_edge() {
        for i in 0..CHUNK_VOLUME as u32 {
            assert_eq!(chunk_block(i), chunk_block_at(CHUNK_BITS, i));
            assert_eq!(block_index(i), block_index_at(CHUNK_BITS, i));
        }
        let last = CHUNK_VOLUME as u32 - 1;
        assert_eq!(chunk_block(last) as usize, BLOCKS_PER_CHUNK - 1);
        assert_eq!(block_index(last) as usize, BLOCK_VOLUME - 1);
    }

    #[test]
    fn block_index_overlay_store_roundtrip() {
        let p = ChunkPos::new(-2, 3, 1);
        let mut g = GpuStore::new();
        let mut values = Vec::new();
        for z in 0..BLOCK_DIM as u32 {
            for y in 0..BLOCK_DIM as u32 {
                for x in 0..BLOCK_DIM as u32 {
                    let idx = crate::local_index(x, y, z);
                    let within = block_index(idx);
                    let value = within as u16 + 1;
                    g.set_overlay(p, idx, value);
                    values.push((x, y, z, within, value));
                }
            }
        }

        // All edits belong to block 0, so exactly one overlay slot is live.
        assert!(g.block_table_slot(p).is_some());
        assert_eq!(g.overlays().len(), OVERLAY_WORDS);
        for (x, y, z, within, value) in values {
            let idx = crate::local_index(x, y, z);
            assert_eq!(block_index(idx), within);
            assert_eq!(g.block_overlay_slot(p, idx), Some(0));
            assert_eq!(g.lookup(p, x, y, z), value);
        }

        g.clear_chunk_overlay(p);
        assert!(g.block_table_slot(p).is_none());
        assert_eq!(g.lookup(p, 0, 0, 0), 0);
    }

    /// The slot layout is a five-word contract independent of the chunk edge:
    /// positions use the shader's two's-complement bit pattern, and the initial
    /// capacity is 1024 slots whether a chunk is 64³ or 128³.
    #[test]
    fn chunk_table_slot_words_and_positions_are_edge_independent() {
        assert_eq!(SLOT_WORDS, 5);
        assert_eq!(
            (SLOT_X, SLOT_Y, SLOT_Z, SLOT_ROOT, SLOT_TABLE),
            (0, 1, 2, 3, 4)
        );
        assert_eq!(INITIAL_SLOTS, 1024);

        // Position words are exactly `bitcast<u32>(p.axis)`: every i32 survives,
        // including the extremes and both signs.
        for &(x, y, z) in &[
            (0, 0, 0),
            (-1, 1, -1),
            (i32::MIN, i32::MAX, i32::MIN),
            (i32::MAX, i32::MIN, i32::MAX),
        ] {
            let p = ChunkPos::new(x, y, z);
            let mut g = GpuStore::new();
            let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
            g.set_chunk(p, &solid);

            let i = g.probe(p).expect("an inserted chunk has a slot");
            let b = slot_base(i);
            assert_eq!(g.table[b + SLOT_X], slot_coord(x));
            assert_eq!(g.table[b + SLOT_Y], slot_coord(y));
            assert_eq!(g.table[b + SLOT_Z], slot_coord(z));
            assert_eq!(coord_from_slot(g.table[b + SLOT_X]), x);
            assert_eq!(coord_from_slot(g.table[b + SLOT_Y]), y);
            assert_eq!(coord_from_slot(g.table[b + SLOT_Z]), z);
            assert!(g.contains(p));
        }

        // The hash reads all three axes and distinguishes the sign bit.
        assert_ne!(
            slot_hash(ChunkPos::new(-1, 0, 0)),
            slot_hash(ChunkPos::new(1, 0, 0))
        );
        assert_ne!(
            slot_hash(ChunkPos::new(1, 0, 0)),
            slot_hash(ChunkPos::new(0, 1, 0))
        );
        assert_ne!(
            slot_hash(ChunkPos::new(0, 1, 0)),
            slot_hash(ChunkPos::new(0, 0, 1))
        );

        // The 128³ edge has 8x the voxels; the chunk-table slot count is fixed.
        let volume_64 = 1usize << (3 * 6);
        let volume_128 = 1usize << (3 * 7);
        assert_eq!(volume_128, volume_64 * 8);
        assert_eq!(INITIAL_SLOTS * SLOT_WORDS, 5120);
    }

    /// 1024 distinct chunk positions, scaled by the chunk edge. The bit split
    /// makes the mapping a bijection, so no chunk repeats at either edge.
    fn chunk_table_positions(chunk_bits: u32) -> Vec<ChunkPos> {
        let scale = 1i32 << (chunk_bits - 6);
        let mut positions = Vec::with_capacity(1024);
        for i in 0..1024i32 {
            let x = ((i & 15) - 8) * scale;
            let y = (((i >> 4) & 7) - 4) * scale;
            let z = ((i >> 7) - 4) * scale;
            positions.push(ChunkPos::new(x, y, z));
        }
        positions
    }

    /// M1-22 acceptance: 1024 chunks insert, lookup and remove at both chunk
    /// edges, and refilling after a full vacate reuses the same capacity
    /// instead of accumulating tombstones.
    #[test]
    fn chunk_table_1024_chunk_insert_lookup_remove_at_both_chunk_edges() {
        for bits in EDGE_BITS {
            let positions = chunk_table_positions(bits);
            assert_eq!(positions.len(), 1024);
            let unique: std::collections::HashSet<_> = positions.iter().copied().collect();
            assert_eq!(unique.len(), 1024, "edge {bits}: workload repeats a chunk");

            let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
            let mut g = GpuStore::new();
            assert_eq!(g.table_slots(), INITIAL_SLOTS, "edge {bits}");

            for p in &positions {
                g.set_chunk(*p, &solid);
            }
            assert_eq!(g.chunk_count(), 1024, "edge {bits}");
            assert_eq!(g.tombs, 0, "edge {bits}");
            assert_eq!(
                g.table_slots(),
                INITIAL_SLOTS * 2,
                "edge {bits}: 1024 chunks must size the table by count, not volume"
            );

            for p in &positions {
                assert!(g.contains(*p), "edge {bits} {p:?}");
                assert_eq!(g.lookup(*p, 1, 2, 3), 1, "edge {bits} {p:?}");
                assert_ne!(g.root_of(*p), NONE, "edge {bits} {p:?}");
            }

            // Vacate all 1024 slots, then refill three times. The first refill
            // rehashes in place because live is zero, dropping every tombstone;
            // capacity and tombstone count must return to the same values.
            for cycle in 0..3 {
                for p in &positions {
                    g.remove_chunk(*p);
                }
                assert_eq!(g.chunk_count(), 0, "edge {bits} cycle {cycle}");
                assert_eq!(g.tombs, 1024, "edge {bits} cycle {cycle}");
                for p in &positions {
                    assert!(!g.contains(*p), "edge {bits} cycle {cycle} {p:?}");
                }

                let tag = g.end_batch();
                g.reclaim(tag);

                for p in &positions {
                    g.set_chunk(*p, &solid);
                }
                assert_eq!(g.chunk_count(), 1024, "edge {bits} cycle {cycle}");
                assert_eq!(g.tombs, 0, "edge {bits} cycle {cycle}: tombstone leak");
                assert_eq!(
                    g.table_slots(),
                    INITIAL_SLOTS * 2,
                    "edge {bits} cycle {cycle}: tombstone-only growth"
                );
            }
        }
    }

    /// A single chunk inserted and removed repeatedly must keep reusing its one
    /// vacated slot. Otherwise every edit churn would grow the GPU table.
    #[test]
    fn chunk_table_churn_reuses_one_tombstone_without_growing() {
        let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
        let p = ChunkPos::new(3, -7, 11);
        let mut g = GpuStore::new();
        let capacity = g.table_slots();

        for _ in 0..10_000 {
            g.set_chunk(p, &solid);
            assert!(g.contains(p));
            g.remove_chunk(p);
            assert!(!g.contains(p));
        }

        assert_eq!(g.table_slots(), capacity);
        assert_eq!(g.tombs, 1, "the single vacated slot was reused");
        assert_eq!(g.chunk_count(), 0);
    }

    /// A hash collision must not make the colliding content private: the second
    /// insert has to reuse the first node instead of allocating again.
    #[test]
    fn intern_reuses_content_after_a_hash_collision() {
        let mut g = GpuStore::new();
        let words = [0x0000_0002u32, 0x1111_1111];
        let h = hash_words(&words);

        // Occupy the primary entry for this hash with different content.
        let occupant = g.alloc(&[0x0000_0002, 0x2222_2222]);
        g.dedup.insert(h, occupant);

        let (first, created) = g.intern(&words);
        assert!(created, "the colliding content is new");
        assert_ne!(first, occupant);

        let (second, created) = g.intern(&words);
        assert!(
            !created,
            "a colliding node must be re-interned, not allocated again"
        );
        assert_eq!(first, second);
        assert_eq!(g.unique_nodes(), 2, "one primary plus one colliding node");
    }

    /// Retiring a colliding node frees its slot; the next intern allocates but
    /// does not leave a stale entry behind.
    #[test]
    fn intern_after_retiring_a_colliding_node_is_consistent() {
        let mut g = GpuStore::new();
        let words = [0x0000_0002u32, 0x3333_3333];
        let h = hash_words(&words);
        let occupant = g.alloc(&[0x0000_0002, 0x4444_4444]);
        g.dedup.insert(h, occupant);

        let (offset, _) = g.intern(&words);
        g.rc[offset as usize] = 1;
        g.decref(offset);

        let (again, created) = g.intern(&words);
        assert!(created, "the node was freed, so this is a fresh allocation");
        assert_eq!(again, offset, "the allocator may reuse the freed slot");
        assert_eq!(g.unique_nodes(), 2);
    }
}
