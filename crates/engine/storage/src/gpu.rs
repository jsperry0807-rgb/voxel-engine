//! Tier 3: GPU-resident SVDAG.
//!
//! * `pool`   : global u32 node pool, same layout as `dag.rs`, absolute offsets, shared by all chunks
//!   (cross-chunk dedup via content-hash table). Per-node refcounts live CPU-side only.
//! * `table`  : dynamic open-addressing chunk table, 4 u32 per slot `[x, y, z, root]`,
//!   root == 0xFFFFFFFF empty, 0xFFFFFFFE tombstone. Power-of-two capacity, linear probe,
//!   load <= 0.5 (tombstones included), grows/rehashes -> full re-upload.
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
//! // i = slot_hash(p) & (cap-1); loop { s = table[i]; if s.root==EMPTY {miss}
//! //   if s.root!=TOMB && s.xyz==p {hit}; i=(i+1)&(cap-1); }
//! ```
use crate::ChunkPos;
use crate::dag::{NONE, ROOT_LEVEL, Svdag, dag_get, hdr_level, hdr_mask, header, morton};
use crate::hash::{FxMap, hash_words};

pub const EMPTY: u32 = u32::MAX;
pub const TOMB: u32 = u32::MAX - 1;

/// Implemented by voxel-render over wgpu buffers. `resize_*` may reallocate and discard
/// contents; the store re-uploads everything right after any resize.
pub trait GpuSink {
    fn resize_nodes(&mut self, words: usize);
    fn write_nodes(&mut self, word_off: usize, data: &[u32]);
    fn resize_table(&mut self, words: usize);
    fn write_table(&mut self, word_off: usize, data: &[u32]);
}

/// In-memory sink for tests/headless.
#[derive(Default)]
pub struct MemSink {
    pub nodes: Vec<u32>,
    pub table: Vec<u32>,
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
    live: usize,
    tombs: usize,
    node_dirty: Vec<(usize, usize)>,
    table_dirty: Vec<(usize, usize)>,
    gpu_node_cap: usize,
    gpu_table_cap: usize,
    retire: Vec<(u64, u32)>,
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
            table: vec![EMPTY; 1024 * 4],
            live: 0,
            tombs: 0,
            node_dirty: Vec::new(),
            table_dirty: vec![(0, 1024 * 4)],
            gpu_node_cap: 0,
            gpu_table_cap: 0,
            retire: Vec::new(),
            epoch: 0,
            import_memo: FxMap::default(),
        }
    }

    // ---------- chunk table ----------

    fn probe(&self, p: ChunkPos) -> Result<usize, usize> {
        let mask = self.table.len() / 4 - 1;
        let mut i = slot_hash(p) as usize & mask;
        let mut tomb = None;
        loop {
            let s = &self.table[i * 4..i * 4 + 4];
            match s[3] {
                EMPTY => return Err(tomb.unwrap_or(i)),
                TOMB => {
                    tomb.get_or_insert(i);
                }
                _ if s[0] == p.x as u32 && s[1] == p.y as u32 && s[2] == p.z as u32 => {
                    return Ok(i);
                }
                _ => {}
            }
            i = (i + 1) & mask;
        }
    }

    fn rehash(&mut self) {
        let cap = self.table.len() / 4;
        let new_cap = if self.live * 4 > cap { cap * 2 } else { cap };
        let old = std::mem::replace(&mut self.table, vec![EMPTY; new_cap * 4]);
        self.live = 0;
        self.tombs = 0;
        for s in old.as_chunks::<4>().0 {
            if s[3] < TOMB {
                let p = ChunkPos::new(s[0] as i32, s[1] as i32, s[2] as i32);
                let i = self.probe(p).unwrap_err();
                self.table[i * 4..i * 4 + 4].copy_from_slice(s);
                self.live += 1;
            }
        }
        self.table_dirty.clear();
        self.table_dirty.push((0, self.table.len()));
    }

    fn table_set(&mut self, p: ChunkPos, root: u32) -> Option<u32> {
        if (self.live + self.tombs + 1) * 2 > self.table.len() / 4 {
            self.rehash();
        }
        match self.probe(p) {
            Ok(i) => {
                let old = self.table[i * 4 + 3];
                self.table[i * 4 + 3] = root;
                self.table_dirty.push((i * 4 + 3, 1));
                Some(old)
            }
            Err(i) => {
                if self.table[i * 4 + 3] == TOMB {
                    self.tombs -= 1;
                }
                self.table[i * 4..i * 4 + 4]
                    .copy_from_slice(&[p.x as u32, p.y as u32, p.z as u32, root]);
                self.live += 1;
                self.table_dirty.push((i * 4, 4));
                None
            }
        }
    }

    fn table_remove(&mut self, p: ChunkPos) -> Option<u32> {
        match self.probe(p) {
            Ok(i) => {
                let old = self.table[i * 4 + 3];
                self.table[i * 4 + 3] = TOMB;
                self.live -= 1;
                self.tombs += 1;
                self.table_dirty.push((i * 4 + 3, 1));
                Some(old)
            }
            Err(_) => None,
        }
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

    /// Point `p` at `dag` (copy-on-write). Empty dag removes the chunk.
    pub fn set_chunk(&mut self, p: ChunkPos, dag: &Svdag) {
        if dag.is_empty() {
            if let Some(old) = self.table_remove(p) {
                self.retire.push((self.epoch, old));
            }
            return;
        }
        let mut memo = std::mem::take(&mut self.import_memo);
        memo.clear();
        let g = self.import(&dag.pool, dag.root, &mut memo);
        self.import_memo = memo;
        self.rc[g as usize] += 1;
        if let Some(old) = self.table_set(p, g) {
            self.retire.push((self.epoch, old));
        }
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
        let old = self.probe(p).ok().map(|i| self.table[i * 4 + 3]);
        match self.gedit(old, ROOT_LEVEL, &e) {
            Some(g) => {
                self.rc[g as usize] += 1;
                if let Some(o) = self.table_set(p, g) {
                    self.retire.push((self.epoch, o));
                }
            }
            None => {
                if let Some(o) = self.table_remove(p) {
                    self.retire.push((self.epoch, o));
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

    /// Free nodes retired at epochs <= `completed`.
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
        for (s, l) in coalesce(std::mem::take(&mut self.node_dirty)) {
            sink.write_nodes(s, &self.pool[s..s + l]);
        }
        for (s, l) in coalesce(std::mem::take(&mut self.table_dirty)) {
            sink.write_table(s, &self.table[s..s + l]);
        }
    }

    /// CPU replica of the shader traversal (tests / CPU fallback renderer).
    pub fn lookup(&self, p: ChunkPos, x: u32, y: u32, z: u32) -> u16 {
        match self.probe(p) {
            Ok(i) => dag_get(&self.pool, self.table[i * 4 + 3], x, y, z),
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
    pub fn unique_nodes(&self) -> usize {
        self.dedup.len()
    }
    pub fn chunk_count(&self) -> usize {
        self.live
    }
    pub fn root_of(&self, p: ChunkPos) -> u32 {
        self.probe(p).map_or(NONE, |i| self.table[i * 4 + 3])
    }
}
