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
//!   has 64 words, one per block, each pointing at an 8 KiB overlay slot (2 048 words of
//!   packed `u16` material ids followed by 128 words of presence bits). The bit, not the
//!   value, means "edited": a carved voxel stores material 0 and must shadow the DAG with
//!   air. Block tables and overlay slots are allocated lazily, uploaded as coalesced dirty
//!   ranges, and retired by epoch like nodes.
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
use crate::{local_index, local_xyz};

/// 16^3-voxel blocks per chunk, one block table word each.
pub use crate::BLOCKS_PER_CHUNK;

pub const EMPTY: u32 = u32::MAX;
pub const TOMB: u32 = u32::MAX - 1;
/// No overlay attached to a block-table entry.
pub const NO_OVERLAY: u32 = u32::MAX;
/// No block table attached to a chunk-table slot.
pub const NO_TABLE: u32 = u32::MAX;
/// Words per chunk-table slot: `[x, y, z, root, block_table]`.
pub const SLOT_WORDS: usize = 5;
/// Voxels in one block.
const BLOCK_VOLUME: usize = BLOCK_DIM * BLOCK_DIM * BLOCK_DIM;
/// Packed `u16` overlay values for one block, two voxels per word.
pub const OVERLAY_VALUES: usize = BLOCK_VOLUME / 2;
/// Overlay presence bits for one block, one per voxel.
pub const OVERLAY_BITS: usize = BLOCK_VOLUME / 32;
/// Words per block-overlay slot: values first, then the presence bitmap.
pub const OVERLAY_WORDS: usize = OVERLAY_VALUES + OVERLAY_BITS;

/// Block index (0..`BLOCKS_PER_CHUNK`) inside a chunk for a chunk-local voxel index.
#[inline]
pub fn chunk_block(idx: u32) -> u32 {
    let (x, y, z) = local_xyz(idx);
    let stride = CHUNK_BITS - BLOCK_BITS;
    (x >> BLOCK_BITS) | ((y >> BLOCK_BITS) << stride) | ((z >> BLOCK_BITS) << (2 * stride))
}

/// Voxel index (0..`BLOCK_VOLUME`) inside its block for a chunk-local voxel index.
#[inline]
pub fn block_index(idx: u32) -> u32 {
    let (x, y, z) = local_xyz(idx);
    let mask = BLOCK_DIM as u32 - 1;
    (x & mask) | ((y & mask) << BLOCK_BITS) | ((z & mask) << (2 * BLOCK_BITS))
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

pub fn slot_hash(p: ChunkPos) -> u32 {
    let mut h = (p.x as u32).wrapping_mul(73856093)
        ^ (p.y as u32).wrapping_mul(19349663)
        ^ (p.z as u32).wrapping_mul(83492791);
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
            table: vec![EMPTY; 1024 * SLOT_WORDS],
            overlays: Vec::new(),
            overlay_counts: Vec::new(),
            overlay_free: Vec::new(),
            block_tables: Vec::new(),
            table_counts: Vec::new(),
            table_free: Vec::new(),
            live: 0,
            tombs: 0,
            node_dirty: Vec::new(),
            table_dirty: vec![(0, 1024 * SLOT_WORDS)],
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

    fn probe(&self, p: ChunkPos) -> Result<usize, usize> {
        let mask = self.table.len() / SLOT_WORDS - 1;
        let mut i = slot_hash(p) as usize & mask;
        let mut tomb = None;
        loop {
            let b = i * SLOT_WORDS;
            let root = self.table[b + 3];
            let blocks = self.table[b + 4];
            if root == TOMB {
                tomb.get_or_insert(i);
            } else if root == EMPTY && blocks == NO_TABLE {
                return Err(tomb.unwrap_or(i));
            } else if self.table[b] == p.x as u32
                && self.table[b + 1] == p.y as u32
                && self.table[b + 2] == p.z as u32
            {
                return Ok(i);
            }
            i = (i + 1) & mask;
        }
    }

    fn rehash(&mut self) {
        let cap = self.table.len() / SLOT_WORDS;
        let new_cap = if self.live * 4 > cap { cap * 2 } else { cap };
        let old = std::mem::replace(&mut self.table, vec![EMPTY; new_cap * SLOT_WORDS]);
        self.live = 0;
        self.tombs = 0;
        for s in old.as_chunks::<SLOT_WORDS>().0 {
            let root = s[3];
            let occupied = root != TOMB && (root != EMPTY || s[4] != NO_TABLE);
            if occupied {
                let p = ChunkPos::new(s[0] as i32, s[1] as i32, s[2] as i32);
                let i = self.probe(p).unwrap_err();
                let b = i * SLOT_WORDS;
                self.table[b..b + SLOT_WORDS].copy_from_slice(s);
                self.live += 1;
            }
        }
        self.table_dirty.clear();
        self.table_dirty.push((0, self.table.len()));
    }

    /// Room check plus probe, so callers can tell `Ok(existing)` from `Err(insert_at)`.
    fn ensure_slot(&mut self, p: ChunkPos) -> Result<usize, usize> {
        if (self.live + self.tombs + 1) * 2 > self.table.len() / SLOT_WORDS {
            self.rehash();
        }
        self.probe(p)
    }

    fn insert_slot_at(&mut self, i: usize, p: ChunkPos, root: u32, blocks: u32) {
        let b = i * SLOT_WORDS;
        if self.table[b + 3] == TOMB {
            self.tombs -= 1;
        }
        self.table[b..b + SLOT_WORDS]
            .copy_from_slice(&[p.x as u32, p.y as u32, p.z as u32, root, blocks]);
        self.live += 1;
        self.table_dirty.push((b, SLOT_WORDS));
    }

    fn tombstone_at(&mut self, i: usize) {
        let b = i * SLOT_WORDS;
        self.table[b + 3] = TOMB;
        self.table[b + 4] = NO_TABLE;
        self.live -= 1;
        self.tombs += 1;
        self.table_dirty.push((b + 3, 2));
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
                let b = i * SLOT_WORDS;
                let old = self.table[b + 3];
                self.table[b + 3] = root;
                self.table_dirty.push((b + 3, 1));
                if root == EMPTY && self.table[b + 4] == NO_TABLE {
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
                let b = i * SLOT_WORDS;
                if self.table[b + 4] == NO_TABLE {
                    let slot = self.alloc_table();
                    self.table[b + 4] = slot;
                    self.table_dirty.push((b + 4, 1));
                }
                (i, self.table[b + 4])
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
        let word = base + (idx >> 1) as usize;
        let shift = (idx & 1) * 16;
        let old = self.overlays[word];
        let new = (old & !(0xffff << shift)) | ((v as u32) << shift);
        if new != old {
            self.overlays[word] = new;
            self.overlay_dirty.push((word, 1));
        }

        let bit_word = base + OVERLAY_VALUES + (idx >> 5) as usize;
        let bit = 1u32 << (idx & 31);
        if self.overlays[bit_word] & bit == 0 {
            self.overlays[bit_word] |= bit;
            self.overlay_dirty.push((bit_word, 1));
            self.overlay_counts[slot as usize] += 1;
        }
    }

    /// Returns true when that was the last presence bit in the block slot.
    fn overlay_clear(&mut self, slot: u32, idx: u32) -> bool {
        let base = slot as usize * OVERLAY_WORDS;
        let bit_word = base + OVERLAY_VALUES + (idx >> 5) as usize;
        let bit = 1u32 << (idx & 31);
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
        let b = i * SLOT_WORDS;
        if self.table[b + 3] == EMPTY {
            self.tombstone_at(i);
        } else {
            self.table[b + 4] = NO_TABLE;
            self.table_dirty.push((b + 4, 1));
        }
    }

    fn drop_chunk_overlay(&mut self, i: usize) {
        let b = i * SLOT_WORDS;
        let table_slot = self.table[b + 4];
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
            return (self.alloc(words), true); // hash collision: keep private, uninterned
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
        self.rc[o] -= 1;
        if self.rc[o] == 0 {
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
        let table_slot = self.table[i * SLOT_WORDS + 4];
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
        let b = i * SLOT_WORDS;
        self.retire_root(self.table[b + 3]);
        let table_slot = self.table[b + 4];
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
            .map(|i| self.table[i * SLOT_WORDS + 3])
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
                let b = i * SLOT_WORDS;
                let table_slot = self.table[b + 4];
                if table_slot != NO_TABLE {
                    let idx = local_index(x, y, z);
                    let bt = table_slot as usize * BLOCKS_PER_CHUNK + chunk_block(idx) as usize;
                    let slot = self.block_tables[bt];
                    if slot != NO_OVERLAY {
                        let within = block_index(idx);
                        let bit_word =
                            slot as usize * OVERLAY_WORDS + OVERLAY_VALUES + (within >> 5) as usize;
                        if self.overlays[bit_word] & (1u32 << (within & 31)) != 0 {
                            let word = slot as usize * OVERLAY_WORDS + (within >> 1) as usize;
                            return (self.overlays[word] >> ((within & 1) * 16)) as u16;
                        }
                    }
                }
                dag_get(&self.pool, self.table[b + 3], x, y, z)
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
        self.dedup.len()
    }
    pub fn chunk_count(&self) -> usize {
        self.live
    }
    /// The chunk's block-table slot, or `None` when it has no overlay at all.
    pub fn block_table_slot(&self, p: ChunkPos) -> Option<u32> {
        match self.probe(p) {
            Ok(i) => {
                let slot = self.table[i * SLOT_WORDS + 4];
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
            let root = self.table[i * SLOT_WORDS + 3];
            if root == EMPTY { NONE } else { root }
        })
    }
}
