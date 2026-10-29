//! Public chunk DAG facade over the banded layout (M1-15).
//!
//! The public [`Svdag`] API predates the banded rewrite: `from_dense`, `get`,
//! `expand`, `apply_edits`, `to_bytes`, `from_bytes`, `words`, `empty` and
//! `is_empty` keep their old signatures and meaning. The authoritative content
//! now lives in a [`ChunkDag`] (8 B nodes, per-level bands, popcount child
//! addressing), so the CPU edit/read path exercises the new layout.
//!
//! The legacy `pool`/`root` fields remain a canonical v1 view of the same
//! content. They keep the GPU importer, the v1 region log and existing callers
//! working until the v2 upload and disk formats land (M4-09, M6-08/M6-09). A
//! canonical view is generated from the banded DAG, not maintained by a second
//! builder, so the two representations cannot drift in content.
//!
//! The v1 node layout is `[header, child_0 .. child_{k-1}]`:
//!
//! - `header = child_mask (bits 0..8) | level << 8` (level `1..=ROOT_LEVEL`)
//! - only present children are stored, in ascending child-index order
//! - child index `c = cx | cy<<1 | cz<<2`
//! - level > 1: a child word is a pool offset of a level-1 node
//! - level == 1: a child word is a voxel material id (non-zero; 0 = air)
//!
//! The level is part of the header, so identical-looking nodes at different
//! levels never dedup.

use crate::bands::Bands;
use crate::chunk::ChunkDag;
use crate::hash::{FxMap, hash_words};
use crate::node_word::NodeRef;
use crate::{CHUNK_VOLUME, local_index};

pub const NONE: u32 = u32::MAX;

/// Level of the chunk root in this pool's numbering, where a node at level `l`
/// covers `2^l` voxels and level 1 holds material ids.
///
/// This counts up from the leaf, the opposite of
/// [`voxel_core::constants::LEVEL_SIZE`] (which counts depth down from the
/// root). The two are complements: `ROOT_LEVEL - depth`.
pub const ROOT_LEVEL: u32 = crate::CHUNK_BITS;

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

/// The public chunk DAG.
///
/// Equality is over the canonical legacy view (`pool` and `root`), matching the
/// pre-M1-15 value semantics: two chunks with the same content compare equal
/// even when one was produced by a COW edit and the other by a dense rebuild.
#[derive(Clone, Debug)]
pub struct Svdag {
    /// Canonical v1 pool view. Kept for GPU/disk compatibility until M4/M6.
    pub pool: Vec<u32>,
    /// Root offset in [`Self::pool`], or [`NONE`] for an all-air chunk.
    pub root: u32,
    /// Authoritative banded representation used by `get`, `expand` and edits.
    chunk: ChunkDag,
}

impl PartialEq for Svdag {
    fn eq(&self, other: &Self) -> bool {
        self.pool == other.pool && self.root == other.root
    }
}

impl Svdag {
    /// Creates an empty chunk: no nodes, no root.
    pub fn empty() -> Self {
        Self {
            pool: Vec::new(),
            root: NONE,
            chunk: ChunkDag::empty(),
        }
    }

    /// True when the chunk contains no voxels.
    pub fn is_empty(&self) -> bool {
        self.root == NONE
    }

    /// Number of `u32` words in the canonical legacy view.
    ///
    /// This is the historical meaning of `words()` and stays stable for callers
    /// that size the v1 disk/GPU buffers. Use [`Self::chunk`] for the banded
    /// node and leaf counts.
    pub fn words(&self) -> usize {
        self.pool.len()
    }

    /// The authoritative banded chunk.
    pub fn chunk(&self) -> &ChunkDag {
        &self.chunk
    }

    /// Checks the banded representation's invariants (M1-14).
    pub fn check(&self) -> Result<(), crate::InvariantError> {
        self.chunk.check()
    }

    /// `d` is dense, index = x + y*64 + z*4096.
    pub fn from_dense(d: &[u16]) -> Self {
        Self::from_chunk(ChunkDag::from_dense(d))
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32, z: u32) -> u16 {
        self.chunk.get(x, y, z)
    }

    pub fn expand(&self, out: &mut [u16]) {
        self.chunk.expand(out);
    }

    /// Rebuild with `edits` (local voxel index, material; 0 = air) applied.
    ///
    /// The old chunk is not mutated. The returned chunk carries the same
    /// content in the banded layout. The band arenas are compacted only when
    /// their waste crosses the 25% threshold ([`ChunkDag::compact_if_needed`]):
    /// a full renumber on every edit was O(chunk) work per batch, and the
    /// canonical v1 pool is rebuilt from node content, not band offsets, so the
    /// rebuild-equality contract is unaffected.
    ///
    /// The legacy v1 `pool` is still carried alongside the banded chunk; it is
    /// the on-disk and GPU-import format until those paths move to the banded
    /// layout (M4/M6).
    pub fn apply_edits(&self, edits: impl IntoIterator<Item = (u32, u16)>) -> Self {
        let edits: Vec<(u32, u16)> = edits.into_iter().collect();
        if edits.is_empty() {
            return self.clone();
        }

        let mut chunk = self.chunk.clone();
        chunk.apply_edits(edits);
        chunk.compact_if_needed();
        Self::from_chunk(chunk)
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
        let pool: Vec<u32> = b[8..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();

        // Build the banded representation from the v1 content. A malformed
        // pool is rejected here rather than reaching `get`/`expand`.
        let mut dense = vec![0u16; CHUNK_VOLUME];
        if root != NONE {
            expand_legacy(&pool, root, ROOT_LEVEL, 0, 0, 0, &mut dense)?;
        }
        let chunk = ChunkDag::from_dense(&dense);
        Some(Self { pool, root, chunk })
    }

    fn from_chunk(chunk: ChunkDag) -> Self {
        let (pool, root) = LegacyPool::encode_chunk(&chunk);
        Self { pool, root, chunk }
    }
}

/// Traversal over any pool holding the v1 layout (chunk-local or the global GPU
/// mirror).
#[inline]
pub fn dag_get(pool: &[u32], root: u32, x: u32, y: u32, z: u32) -> u16 {
    if root == NONE {
        return 0;
    }
    let mut n = root as usize;
    let mut level = ROOT_LEVEL;
    loop {
        // A malformed or out-of-range reference reads as air instead of
        // panicking: this is the shader's behaviour too.
        let Some(&h) = pool.get(n) else {
            return 0;
        };
        let mask = hdr_mask(h);
        let b = level - 1;
        let c = ((x >> b) & 1) | (((y >> b) & 1) << 1) | (((z >> b) & 1) << 2);
        if ((mask >> c) & 1) == 0 {
            return 0;
        }
        let k = (mask & ((1 << c) - 1)).count_ones() as usize;
        let Some(&w) = pool.get(n + 1 + k) else {
            return 0;
        };
        if level == 1 {
            return w as u16;
        }
        n = w as usize;
        level -= 1;
    }
}

/// Re-export of the chunk-local Morton helper from the crate root (M1-20).
pub use crate::morton;

/// Expands a v1 pool into a dense chunk, rejecting malformed references.
fn expand_legacy(
    pool: &[u32],
    node: u32,
    level: u32,
    ox: usize,
    oy: usize,
    oz: usize,
    out: &mut [u16],
) -> Option<()> {
    let n = usize::try_from(node).ok()?;
    let h = *pool.get(n)?;
    if hdr_level(h) != level {
        return None;
    }
    let mask = hdr_mask(h);
    let half = 1usize << (level - 1);
    let mut k = 0usize;
    for c in 0..8usize {
        if (mask >> c) & 1 == 0 {
            continue;
        }
        let w = *pool.get(n + 1 + k)?;
        k += 1;
        let cx = ox + (c & 1) * half;
        let cy = oy + ((c >> 1) & 1) * half;
        let cz = oz + ((c >> 2) & 1) * half;
        if level == 1 {
            out[local_index(cx as u32, cy as u32, cz as u32) as usize] = w as u16;
        } else {
            expand_legacy(pool, w, level - 1, cx, cy, cz, out)?;
        }
    }
    Some(())
}

/// Builds the canonical v1 pool from a banded chunk.
///
/// The key is `(level, mask, ordered child words)` exactly like the historical
/// builder: two banded nodes with the same content intern to one offset, so the
/// legacy view is independent of COW history and duplicate banded runs.
struct LegacyPool {
    pool: Vec<u32>,
    map: FxMap<u64, u32>,
}

impl LegacyPool {
    fn encode_chunk(chunk: &ChunkDag) -> (Vec<u32>, u32) {
        let mut builder = Self {
            pool: Vec::new(),
            map: FxMap::default(),
        };
        let mut memo: FxMap<NodeRef, u32> = FxMap::default();
        let root = match chunk.root() {
            Some(root) => builder.encode_node(chunk.bands(), root, &mut memo),
            None => NONE,
        };
        (builder.pool, root)
    }

    fn encode_node(
        &mut self,
        bands: &Bands,
        node_ref: NodeRef,
        memo: &mut FxMap<NodeRef, u32>,
    ) -> u32 {
        if let Some(&offset) = memo.get(&node_ref) {
            return offset;
        }

        let node = bands
            .get(node_ref)
            .unwrap_or_else(|| panic!("a live banded reference names a node: {node_ref:?}"));
        let mut words = [0u32; 9];
        words[0] = header(u32::from(node.mask()), node.level());
        let mut count = 0usize;
        for c in 0..8u32 {
            if node.mask() & (1 << c) == 0 {
                continue;
            }
            let slot = node
                .child_slot(c)
                .expect("a present child has a compacted slot");
            let index = node.base() + u64::from(slot);
            let word = if node.level() == 1 {
                let material = bands
                    .leaf(index)
                    .expect("a live leaf reference is inside the leaf arena");
                u32::from(material)
            } else {
                self.encode_node(bands, NodeRef::new(node.level() - 1, index), memo)
            };
            count += 1;
            words[count] = word;
        }

        let offset = self.intern(&words[..=count]);
        memo.insert(node_ref, offset);
        offset
    }

    fn intern(&mut self, key: &[u32]) -> u32 {
        let hash = hash_words(key);
        let len = key.len();
        if let Some(&offset) = self.map.get(&hash) {
            let start = offset as usize;
            if start + len <= self.pool.len() && self.pool[start..start + len] == *key {
                return offset;
            }
            // A hash collision appends an unshared copy, exactly like the old
            // builder; correctness never depends on the hash being unique.
            let offset = self.pool.len() as u32;
            self.pool.extend_from_slice(key);
            return offset;
        }
        let offset = self.pool.len() as u32;
        self.pool.extend_from_slice(key);
        self.map.insert(hash, offset);
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CHUNK_DIM, local_index};

    fn blocky() -> Vec<u16> {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = if y < 20 {
                        1
                    } else if (x / 8 + z / 8) % 3 == 0 && y < 30 {
                        2
                    } else {
                        0
                    };
                }
            }
        }
        dense
    }

    /// M1-15: every public method keeps its pre-rewrite signature and meaning,
    /// and the CPU path now runs on the banded `ChunkDag`.
    #[test]
    fn svdag_api_stays_stable_over_the_banded_layout() {
        let dense = blocky();
        let dag = Svdag::from_dense(&dense);

        assert!(!dag.is_empty());
        assert!(dag.words() > 0);
        assert!(dag.pool.len() <= dag.words());
        dag.check().expect("the banded representation is valid");
        assert!(dag.chunk().node_words() > 0);

        for i in (0..CHUNK_VOLUME).step_by(37) {
            let (x, y, z) = (
                (i % CHUNK_DIM) as u32,
                ((i / CHUNK_DIM) % CHUNK_DIM) as u32,
                (i / (CHUNK_DIM * CHUNK_DIM)) as u32,
            );
            assert_eq!(dag.get(x, y, z), dense[i]);
        }

        let mut expanded = vec![u16::MAX; CHUNK_VOLUME];
        dag.expand(&mut expanded);
        assert_eq!(expanded, dense);

        let bytes = dag.to_bytes();
        let reloaded = Svdag::from_bytes(&bytes).expect("v1 bytes still load");
        assert_eq!(reloaded, dag);
        assert_eq!(reloaded.to_bytes(), bytes);
    }

    /// The immutable edit API still returns a new chunk and leaves the old one
    /// untouched, while its canonical v1 view matches a dense rebuild.
    #[test]
    fn svdag_apply_edits_is_canonical_and_non_mutating() {
        let dense = blocky();
        let base = Svdag::from_dense(&dense);
        let mut edited = dense.clone();
        let edits: Vec<(u32, u16)> = (0..64u32)
            .map(|i| {
                let index = (i * 7919) % CHUNK_VOLUME as u32;
                let material = (i % 5) as u16;
                edited[index as usize] = material;
                (index, material)
            })
            .collect();

        let fast = base.apply_edits(edits.iter().copied());
        let slow = Svdag::from_dense(&edited);

        assert_eq!(fast, slow, "same content must compare equal");
        assert_eq!(fast.pool, slow.pool, "the canonical v1 view is stable");
        assert_eq!(fast.words(), slow.words());
        fast.check().expect("edited banded chunk is valid");

        let mut before = vec![0u16; CHUNK_VOLUME];
        base.expand(&mut before);
        assert_eq!(before, dense, "apply_edits must not mutate the receiver");

        let mut after = vec![0u16; CHUNK_VOLUME];
        fast.expand(&mut after);
        assert_eq!(after, edited);
    }

    /// Empty chunks keep the old sentinel semantics and clear a dirty output.
    #[test]
    fn svdag_empty_api_is_unchanged() {
        let empty = Svdag::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.words(), 0);
        assert_eq!(empty.root, NONE);
        assert!(empty.pool.is_empty());
        assert_eq!(empty.get(0, 0, 0), 0);
        assert!(empty.chunk().is_empty());

        let mut out = vec![u16::MAX; CHUNK_VOLUME];
        empty.expand(&mut out);
        assert!(out.iter().all(|&material| material == 0));

        let bytes = empty.to_bytes();
        assert_eq!(Svdag::from_bytes(&bytes), Some(empty));
    }

    /// A corrupt v1 reference is rejected by `from_bytes`, not accepted and
    /// deferred to a panic in `get`/`expand`.
    #[test]
    fn svdag_from_bytes_rejects_a_bad_child_reference() {
        let dag = Svdag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        let mut bytes = dag.to_bytes();
        let root = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let node_offset = 8 + root * 4;
        let header = u32::from_le_bytes(bytes[node_offset..node_offset + 4].try_into().unwrap());
        assert!(hdr_mask(header) != 0);
        // Point the first child past the pool.
        bytes[node_offset + 4..node_offset + 8]
            .copy_from_slice(&(dag.pool.len() as u32).to_le_bytes());
        assert!(Svdag::from_bytes(&bytes).is_none());
    }
}
