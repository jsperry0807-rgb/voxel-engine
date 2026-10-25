//! Sparse voxel DAG over a 64^3 chunk (6 levels), hash-consed.
//!
//! Node layout (u32 words): `[header, child_0 .. child_{k-1}]`
//!   header = child_mask (bits 0..8) | level << 8     (level 1..=6, root = 6)
//!   only present children are stored, in ascending child-index order
//!   child index c = cx | cy<<1 | cz<<2
//!   level > 1 : child word = pool offset of a level-1 node
//!   level == 1: child word = voxel material id (non-zero; 0 = air = absent)
//! Level is part of the header so identical-looking nodes at different levels never dedup.
use crate::CHUNK_VOLUME;
use crate::hash::{FxMap, hash_words};

pub const NONE: u32 = u32::MAX;
pub const ROOT_LEVEL: u32 = 6;

#[inline]
pub fn header(mask: u32, level: u32) -> u32 {
    mask | (level << 8)
}
#[inline]
pub fn hdr_mask(h: u32) -> u32 {
    h & 0xff
}
#[inline]
pub fn hdr_level(h: u32) -> u32 {
    (h >> 8) & 0xf
}

#[derive(Clone, Debug, PartialEq)]
pub struct Svdag {
    pub pool: Vec<u32>,
    pub root: u32,
}

impl Svdag {
    pub fn empty() -> Self {
        Self {
            pool: Vec::new(),
            root: NONE,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.root == NONE
    }
    pub fn words(&self) -> usize {
        self.pool.len()
    }

    /// `d` is dense, index = x + y*64 + z*4096.
    pub fn from_dense(d: &[u16]) -> Self {
        assert_eq!(d.len(), CHUNK_VOLUME);
        let mut b = Builder::new();
        let root = b.build(d, ROOT_LEVEL, 0, 0, 0);
        Self {
            pool: b.pool,
            root: root.unwrap_or(NONE),
        }
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32, z: u32) -> u16 {
        dag_get(&self.pool, self.root, x, y, z)
    }

    pub fn expand(&self, out: &mut [u16]) {
        assert_eq!(out.len(), CHUNK_VOLUME);
        out.fill(0);
        if self.root != NONE {
            expand_rec(&self.pool, self.root, ROOT_LEVEL, 0, 0, 0, out);
        }
    }

    /// Rebuild with `edits` (local voxel index, material; 0 = air) applied, without expanding to
    /// a dense array. Untouched subtrees are copied (re-interned), only edited paths are rebuilt.
    pub fn apply_edits(&self, edits: impl IntoIterator<Item = (u32, u16)>) -> Self {
        let mut e: Vec<(u32, u16)> = edits.into_iter().map(|(i, v)| (morton(i), v)).collect();
        if e.is_empty() {
            return self.clone();
        }
        e.sort_unstable_by_key(|x| x.0);
        let mut b = Builder::new();
        let root = b.edit(
            &self.pool,
            (self.root != NONE).then_some(self.root),
            ROOT_LEVEL,
            &e,
        );
        Self {
            pool: b.pool,
            root: root.unwrap_or(NONE),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(8 + self.pool.len() * 4);
        v.extend_from_slice(&self.root.to_le_bytes());
        v.extend_from_slice(&(self.pool.len() as u32).to_le_bytes());
        for w in &self.pool {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < 8 {
            return None;
        }
        let root = u32::from_le_bytes(b[0..4].try_into().ok()?);
        let n = u32::from_le_bytes(b[4..8].try_into().ok()?) as usize;
        if b.len() != 8 + n * 4 || (root != NONE && root as usize >= n) {
            return None;
        }
        let pool = b[8..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        Some(Self { pool, root })
    }
}

/// Traversal over any pool holding this layout (chunk-local or the global GPU mirror).
#[inline]
pub fn dag_get(pool: &[u32], root: u32, x: u32, y: u32, z: u32) -> u16 {
    if root == NONE {
        return 0;
    }
    let mut n = root as usize;
    let mut level = ROOT_LEVEL;
    loop {
        let h = pool[n];
        let mask = hdr_mask(h);
        let b = level - 1;
        let c = ((x >> b) & 1) | (((y >> b) & 1) << 1) | (((z >> b) & 1) << 2);
        if ((mask >> c) & 1) == 0 {
            return 0;
        }
        let k = (mask & ((1 << c) - 1)).count_ones() as usize;
        let w = pool[n + 1 + k];
        if level == 1 {
            return w as u16;
        }
        n = w as usize;
        level -= 1;
    }
}

#[inline]
fn spread(v: u32) -> u32 {
    let mut r = 0;
    for b in 0..6 {
        r |= ((v >> b) & 1) << (3 * b);
    }
    r
}

/// Morton code of a local index; child index at level L is (m >> 3*(L-1)) & 7, matching node layout.
#[inline]
pub fn morton(idx: u32) -> u32 {
    let (x, y, z) = (idx & 63, (idx >> 6) & 63, idx >> 12);
    spread(x) | (spread(y) << 1) | (spread(z) << 2)
}

fn uniform_block(d: &[u16], ox: usize, oy: usize, oz: usize, size: usize) -> Option<u16> {
    let m = d[ox + oy * 64 + oz * 4096];
    for z in oz..oz + size {
        for y in oy..oy + size {
            let s = ox + y * 64 + z * 4096;
            if !d[s..s + size].iter().all(|&v| v == m) {
                return None;
            }
        }
    }
    Some(m)
}

struct Builder {
    pool: Vec<u32>,
    map: FxMap<u64, u32>,
    ucache: FxMap<(u32, u16), u32>,
    memo: FxMap<u32, u32>,
}

impl Builder {
    fn new() -> Self {
        Self {
            pool: Vec::new(),
            map: FxMap::default(),
            ucache: FxMap::default(),
            memo: FxMap::default(),
        }
    }

    fn intern(&mut self, key: &[u32]) -> u32 {
        let h = hash_words(key);
        let n = key.len();
        if let Some(&o) = self.map.get(&h) {
            let u = o as usize;
            if self.pool[u..u + n] == *key {
                return o;
            }
            // hash collision: append unshared (still correct, just not deduped)
            let o = self.pool.len() as u32;
            self.pool.extend_from_slice(key);
            return o;
        }
        let o = self.pool.len() as u32;
        self.pool.extend_from_slice(key);
        self.map.insert(h, o);
        o
    }

    /// Node whose 8 children are all material `m` (None for air).
    fn uniform(&mut self, level: u32, m: u16) -> Option<u32> {
        if m == 0 {
            return None;
        }
        if let Some(&o) = self.ucache.get(&(level, m)) {
            return Some(o);
        }
        let c = if level == 1 {
            m as u32
        } else {
            self.uniform(level - 1, m).unwrap()
        };
        let mut words = [c; 9];
        words[0] = header(0xff, level);
        let o = self.intern(&words);
        self.ucache.insert((level, m), o);
        Some(o)
    }

    fn build(&mut self, d: &[u16], level: u32, ox: usize, oy: usize, oz: usize) -> Option<u32> {
        if level >= 3
            && let Some(m) = uniform_block(d, ox, oy, oz, 1usize << level)
        {
            return self.uniform(level, m);
        }
        let half = 1usize << (level - 1);
        let mut words = [0u32; 9];
        let mut mask = 0u32;
        let mut n = 0usize;
        for c in 0..8usize {
            let cx = ox + (c & 1) * half;
            let cy = oy + ((c >> 1) & 1) * half;
            let cz = oz + ((c >> 2) & 1) * half;
            let v = if level == 1 {
                let m = d[cx + cy * 64 + cz * 4096];
                (m != 0).then_some(m as u32)
            } else {
                self.build(d, level - 1, cx, cy, cz)
            };
            if let Some(v) = v {
                mask |= 1 << c;
                n += 1;
                words[n] = v;
            }
        }
        if mask == 0 {
            return None;
        }
        words[0] = header(mask, level);
        Some(self.intern(&words[..=n]))
    }

    /// Copy an old subtree into the new pool (interned, memoized per old offset).
    fn copy(&mut self, old: &[u32], node: u32) -> u32 {
        if let Some(&g) = self.memo.get(&node) {
            return g;
        }
        let n = node as usize;
        let h = old[n];
        let cnt = hdr_mask(h).count_ones() as usize;
        let level = hdr_level(h);
        let mut words = [0u32; 9];
        words[0] = h;
        for k in 0..cnt {
            let w = old[n + 1 + k];
            words[1 + k] = if level == 1 { w } else { self.copy(old, w) };
        }
        let g = self.intern(&words[..=cnt]);
        self.memo.insert(node, g);
        g
    }

    /// `edits` are (morton, material) sorted by morton, all inside this node's block.
    fn edit(
        &mut self,
        old: &[u32],
        node: Option<u32>,
        level: u32,
        edits: &[(u32, u16)],
    ) -> Option<u32> {
        if edits.is_empty() {
            return node.map(|n| self.copy(old, n));
        }
        let shift = 3 * (level - 1);
        let (omask, obase) = match node {
            Some(n) => (hdr_mask(old[n as usize]), n as usize + 1),
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
                Some(old[obase + k])
            } else {
                None
            };
            let res = if level == 1 {
                match sub.last() {
                    Some(&(_, v)) => (v != 0).then_some(v as u32),
                    None => ow,
                }
            } else {
                self.edit(old, ow, level - 1, sub)
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
        Some(self.intern(&words[..=cnt]))
    }
}

fn expand_rec(
    pool: &[u32],
    node: u32,
    level: u32,
    ox: usize,
    oy: usize,
    oz: usize,
    out: &mut [u16],
) {
    let n = node as usize;
    let mask = hdr_mask(pool[n]);
    let half = 1usize << (level - 1);
    let mut k = 0;
    for c in 0..8usize {
        if (mask >> c) & 1 == 0 {
            continue;
        }
        let w = pool[n + 1 + k];
        k += 1;
        let cx = ox + (c & 1) * half;
        let cy = oy + ((c >> 1) & 1) * half;
        let cz = oz + ((c >> 2) & 1) * half;
        if level == 1 {
            out[cx + cy * 64 + cz * 4096] = w as u16;
        } else {
            expand_rec(pool, w, level - 1, cx, cy, cz, out);
        }
    }
}
