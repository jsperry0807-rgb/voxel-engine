//! Dense-to-banded chunk build (M1-07).
//!
//! [`ChunkDag::from_dense`] turns a `CHUNK_VOLUME`-entry dense buffer into the
//! level bands of [`Bands`], with the root in the top band and the leaf
//! materials in the leaf arena. It is the first half of the banded layout's
//! public surface: `get` (M1-08), `expand` (M1-09) and COW `apply_edits`
//! (M1-10) all read the structure this build emits.
//!
//! # Build order
//!
//! The build is breadth-first over the octree, root first:
//!
//! 1. **Plan, root to leaves.** Occupancy is reduced bottom-up into a mask per
//!    region, then the octree is walked in BFS from the root. Each level's
//!    present nodes are appended to a frontier in parent order and child
//!    order, so band `l` is numbered in BFS order. The walk finishes at the
//!    level-1 parents, whose eight voxels are the last data read from the
//!    dense buffer.
//! 2. **Materialize, leaves to root.** A parent word stores the base of its
//!    child run, so runs must be interned bottom-up: level-1 words first, then
//!    level 2, and so on. This changes neither the band membership nor the BFS
//!    numbering: a level's slots are still appended in the order the plan
//!    visited that level.
//!
//! Empty regions are absent, not zero-material nodes: a mask bit is clear and
//! no slot is appended. An all-air chunk therefore allocates no bands and has
//! no root. The leaf arena follows the internal bands in the chunk's layout,
//! and is filled in the BFS order of its level-1 parents.
//!
//! # Example
//!
//! ```
//! use voxel_storage::{CHUNK_BITS, CHUNK_VOLUME, ChunkDag};
//!
//! let dense = vec![3u16; CHUNK_VOLUME];
//! let dag = ChunkDag::from_dense(&dense);
//!
//! let root = dag.root().expect("a solid chunk has a root");
//! assert_eq!(root.level(), CHUNK_BITS); // pool numbering: root at CHUNK_BITS
//! assert!(!dag.is_empty());
//! ```

use voxel_core::constants::ROOT_LEVEL;

use crate::bands::Bands;
use crate::leaf::{AIR, LEAF_RUN_MATERIALS, fetch_leaf};
use crate::node_word::{Node, NodeRef};
use crate::run::{RunInterner, fetch_child};
use crate::{CHUNK_DIM, CHUNK_VOLUME, local_index};

/// One chunk in the banded SVDAG layout.
///
/// The bands are append-only and level-correct: every node in band `l` carries
/// `level == l`, and the root (when present) is the only node in band
/// `ROOT_LEVEL`. A chunk with no voxels has no bands and no root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkDag {
    bands: Bands,
    root: Option<NodeRef>,
}

impl ChunkDag {
    /// Creates an empty chunk: no bands, no root.
    pub fn empty() -> Self {
        Self {
            bands: Bands::new(),
            root: None,
        }
    }

    /// Builds one chunk from a dense buffer, index = `x + y*CHUNK_DIM + z*CHUNK_DIM^2`.
    ///
    /// Present regions are laid out breadth-first from the root; see the
    /// [module docs](self) for the two passes and what "BFS order" means for
    /// each band. An all-air buffer builds the empty chunk.
    ///
    /// # Panics
    ///
    /// Panics if `dense.len() != CHUNK_VOLUME`.
    pub fn from_dense(dense: &[u16]) -> Self {
        assert_eq!(
            dense.len(),
            CHUNK_VOLUME,
            "a dense chunk must hold CHUNK_VOLUME voxels"
        );

        let masks = level_masks(dense);
        if masks[ROOT_LEVEL as usize][0] == 0 {
            return Self::empty();
        }

        let order = bfs_order(&masks);
        let mut interner = RunInterner::new();

        // Level 1 parents are leaves' direct parents: their child base is a
        // leaf-arena slot, so they are the only level that does not intern a
        // run of internal nodes.
        let mut child_words = level_one_words(dense, &masks[1], &order[1], &mut interner);
        let mut root_word = None;

        // Each pass turns the previous level's words into runs in the band
        // below, then builds this level's words from those bases.
        for parent_level in 2..=ROOT_LEVEL {
            let parent_masks = &masks[parent_level as usize];
            let mut parent_words = vec![Node::new(0, parent_level, 0); level_nodes(parent_level)];

            for &index in &order[parent_level as usize] {
                let mask = parent_masks[index];
                let mut children = [Node::new(0, parent_level - 1, 0); 8];
                let mut count = 0usize;
                for c in 0..8u32 {
                    if mask & (1 << c) == 0 {
                        continue;
                    }
                    children[count] = child_words[child_index(parent_level, index, c)];
                    count += 1;
                }

                let base = interner.intern(parent_level - 1, mask, &children[..count]);
                let word = Node::new(mask, parent_level, base);
                if parent_level == ROOT_LEVEL {
                    root_word = Some(word);
                } else {
                    parent_words[index] = word;
                }
            }

            child_words = parent_words;
        }

        let root_word = root_word.expect("a non-empty chunk has a root node");
        let mut bands = interner.into_bands();
        // The root has no parent to intern its run, so it is the one direct
        // append. Band ROOT_LEVEL has no other nodes.
        let root = bands.append(ROOT_LEVEL, root_word);

        Self {
            bands,
            root: Some(root),
        }
    }

    /// True when the chunk has no voxels.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Reads the material at local voxel `(x, y, z)`.
    ///
    /// The walk descends from the root, taking the octant that contains the
    /// voxel at each level, and ends at the level-1 parent whose leaf run holds
    /// the material. An absent octant at any level is air, so an all-air chunk
    /// and a carved-out region both read [`AIR`] (`0`) without a special case.
    ///
    /// Coordinates are local to the chunk (`0..CHUNK_DIM`); the traversal reads
    /// only the low [`CHUNK_BITS`](crate::CHUNK_BITS) bits, so a value past the
    /// edge aliases the voxel at the same offset modulo the chunk edge.
    #[inline]
    pub fn get(&self, x: u32, y: u32, z: u32) -> u16 {
        let Some(root) = self.root else {
            return AIR;
        };
        let Some(mut node) = self.bands.get(root) else {
            return AIR;
        };

        loop {
            let level = node.level();
            let c = octant(x, y, z, level);
            if level == 1 {
                return fetch_leaf(&self.bands, node, c);
            }
            let Some(child) = fetch_child(&self.bands, node, c) else {
                return AIR;
            };
            node = child;
        }
    }

    /// Writes the chunk's voxels into a dense buffer, index
    /// `x + y*CHUNK_DIM + z*CHUNK_DIM^2`.
    ///
    /// `out` is cleared first, so an all-air chunk leaves it all [`AIR`] and an
    /// absent octant reads as air. The walk visits only present children, in
    /// ascending child order, so expanding a sparse chunk costs its stored
    /// nodes rather than the full [`CHUNK_VOLUME`].
    ///
    /// # Panics
    ///
    /// Panics if `out.len() != CHUNK_VOLUME`.
    pub fn expand(&self, out: &mut [u16]) {
        assert_eq!(
            out.len(),
            CHUNK_VOLUME,
            "a dense chunk must hold CHUNK_VOLUME voxels"
        );
        expand_from(&self.bands, self.root, out);
    }

    /// The chunk root, or `None` for an all-air chunk.
    pub fn root(&self) -> Option<NodeRef> {
        self.root
    }

    /// The chunk's level bands and leaf arena.
    pub fn bands(&self) -> &Bands {
        &self.bands
    }

    /// Mutable access for the COW editor (M1-10).
    ///
    /// The editor only appends, so every existing node index stays valid.
    pub(crate) fn bands_mut(&mut self) -> &mut Bands {
        &mut self.bands
    }

    /// Publishes the root produced by a COW edit (M1-10).
    pub(crate) fn set_root(&mut self, root: Option<NodeRef>) {
        self.root = root;
    }

    /// Number of 8 B internal node words across every band.
    pub fn node_words(&self) -> u64 {
        self.bands.total_len()
    }

    /// Number of `u16` material slots in the leaf arena.
    pub fn leaf_words(&self) -> u64 {
        self.bands.leaf_len()
    }
}

impl Default for ChunkDag {
    fn default() -> Self {
        Self::empty()
    }
}

/// Number of possible nodes at `level`, including absent (mask-zero) regions.
fn level_nodes(level: u32) -> usize {
    let n = CHUNK_DIM >> level;
    n * n * n
}

/// Voxel origin of node `index` at `level`, with nodes indexed `x + y*n + z*n^2`.
fn node_origin(level: u32, index: usize) -> (usize, usize, usize) {
    let n = CHUNK_DIM >> level;
    let size = 1usize << level;
    let x = index % n;
    let y = (index / n) % n;
    let z = index / (n * n);
    (x * size, y * size, z * size)
}

/// Index at `level - 1` of child `c` of node `index` at `level`.
fn child_index(level: u32, index: usize, c: u32) -> usize {
    let n = CHUNK_DIM >> level;
    let child_n = n * 2;
    let x = 2 * (index % n) + (c as usize & 1);
    let y = 2 * ((index / n) % n) + ((c as usize >> 1) & 1);
    let z = 2 * (index / (n * n)) + ((c as usize >> 2) & 1);
    x + y * child_n + z * child_n * child_n
}

/// Octant of local voxel `(x, y, z)` inside a node at `level`.
///
/// A node at `level` covers `2^level` voxels per edge, so the bit that picks
/// the child is `level - 1`; the child-index order matches [`child_index`].
#[inline]
fn octant(x: u32, y: u32, z: u32, level: u32) -> u32 {
    let bit = level - 1;
    ((x >> bit) & 1) | (((y >> bit) & 1) << 1) | (((z >> bit) & 1) << 2)
}

/// Expands `root` (or air) into `out`, clearing it first.
///
/// This is the shared implementation behind [`ChunkDag::expand`] and the COW
/// tests, which expand a retained old root against the bands a later edit
/// appended to.
pub(crate) fn expand_from(bands: &Bands, root: Option<NodeRef>, out: &mut [u16]) {
    debug_assert_eq!(out.len(), CHUNK_VOLUME);
    out.fill(AIR);
    if let Some(root) = root {
        expand_node(bands, root, 0, 0, 0, out);
    }
}

/// Writes the voxels covered by `node_ref` into `out`, at origin `(ox, oy, oz)`.
///
/// Only present children are visited, so absent octants keep the air the
/// caller wrote. The node's level and the reference's band are cross-checked in
/// debug builds: the two must agree for the address to be meaningful.
fn expand_node(bands: &Bands, node_ref: NodeRef, ox: usize, oy: usize, oz: usize, out: &mut [u16]) {
    let node = bands.get(node_ref).expect("a live reference names a node");
    let level = node.level();
    debug_assert_eq!(level, node_ref.level(), "a reference must name its band");
    let half = 1usize << (level - 1);

    for c in 0..8u32 {
        if node.mask() & (1 << c) == 0 {
            continue;
        }
        let x = ox + (c as usize & 1) * half;
        let y = oy + ((c as usize >> 1) & 1) * half;
        let z = oz + ((c as usize >> 2) & 1) * half;

        if level == 1 {
            out[local_index(x as u32, y as u32, z as u32) as usize] = fetch_leaf(bands, node, c);
        } else {
            let index = node.child_index(c).expect("a present child has a slot");
            expand_node(bands, NodeRef::new(level - 1, index), x, y, z, out);
        }
    }
}

/// Occupancy mask of every possible node at levels 1..=ROOT_LEVEL.
///
/// Level 1 is read straight from the voxels; every higher level is the
/// non-emptiness of its eight children, so one pass over each level suffices.
fn level_masks(dense: &[u16]) -> Vec<Vec<u8>> {
    let mut masks = vec![Vec::new(); ROOT_LEVEL as usize + 1];

    let mut leaves = vec![0u8; level_nodes(1)];
    for (index, mask) in leaves.iter_mut().enumerate() {
        let (ox, oy, oz) = node_origin(1, index);
        for c in 0..8u32 {
            let x = ox + (c as usize & 1);
            let y = oy + ((c as usize >> 1) & 1);
            let z = oz + ((c as usize >> 2) & 1);
            if dense[local_index(x as u32, y as u32, z as u32) as usize] != 0 {
                *mask |= 1 << c;
            }
        }
    }
    masks[1] = leaves;

    for level in 2..=ROOT_LEVEL {
        let previous = &masks[(level - 1) as usize];
        let mut current = vec![0u8; level_nodes(level)];
        for (index, mask) in current.iter_mut().enumerate() {
            for c in 0..8u32 {
                if previous[child_index(level, index, c)] != 0 {
                    *mask |= 1 << c;
                }
            }
        }
        masks[level as usize] = current;
    }

    masks
}

/// BFS order of the present nodes in each band, starting at the root.
fn bfs_order(masks: &[Vec<u8>]) -> Vec<Vec<usize>> {
    let mut order = vec![Vec::new(); masks.len()];
    order[ROOT_LEVEL as usize] = vec![0];

    for level in (2..=ROOT_LEVEL).rev() {
        let mut next = Vec::new();
        for &index in &order[level as usize] {
            let mask = masks[level as usize][index];
            for c in 0..8u32 {
                if mask & (1 << c) != 0 {
                    next.push(child_index(level, index, c));
                }
            }
        }
        order[(level - 1) as usize] = next;
    }

    order
}

/// Builds a word for every present level-1 parent, interning its leaf run.
fn level_one_words(
    dense: &[u16],
    masks: &[u8],
    order: &[usize],
    interner: &mut RunInterner,
) -> Vec<Node> {
    let mut words = vec![Node::new(0, 1, 0); masks.len()];
    for &index in order {
        let mask = masks[index];
        let (ox, oy, oz) = node_origin(1, index);
        let mut materials = [0u16; LEAF_RUN_MATERIALS];
        let mut count = 0usize;
        for c in 0..8u32 {
            if mask & (1 << c) == 0 {
                continue;
            }
            let x = ox + (c as usize & 1);
            let y = oy + ((c as usize >> 1) & 1);
            let z = oz + ((c as usize >> 2) & 1);
            materials[count] = dense[local_index(x as u32, y as u32, z as u32) as usize];
            count += 1;
        }
        let base = interner.intern_leaf(&materials[..count]);
        words[index] = Node::new(mask, 1, base);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// The five M1-07 fixtures: empty, uniform, layered, alternating and noisy.
    fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
        let air = vec![0u16; CHUNK_VOLUME];
        let solid = vec![3u16; CHUNK_VOLUME];

        let mut slab = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if y < 17 {
                        slab[local_index(x as u32, y as u32, z as u32) as usize] =
                            1 + (y % 3) as u16;
                    }
                }
            }
        }

        let mut checker = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    checker[local_index(x as u32, y as u32, z as u32) as usize] =
                        if (x + y + z) % 2 == 0 { 1 } else { 2 };
                }
            }
        }

        let mut noise = vec![0u16; CHUNK_VOLUME];
        let mut state = 0x5eed_1e55_c0de_1234u64;
        for voxel in &mut noise {
            let sample = rng(&mut state);
            if sample % 100 < 30 {
                *voxel = 1 + ((sample >> 8) % 7) as u16;
            }
        }

        vec![
            ("air", air),
            ("solid", solid),
            ("slab", slab),
            ("checker", checker),
            ("noise", noise),
        ]
    }

    /// Every region gets all eight voxels and a leaf run that no other region
    /// shares, so the BFS-order test sees every band slot exactly once.
    fn unique_dense() -> Vec<u16> {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        let n = CHUNK_DIM / 2;
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let region = (x / 2) + (y / 2) * n + (z / 2) * n * n;
                    // A unique (lo, hi) pair in the first two materials makes
                    // every eight-material run unique, at either chunk edge.
                    let lo = (region % (u16::MAX as usize - 1)) as u16 + 1;
                    let hi = (region / (u16::MAX as usize - 1)) as u16 + 1;
                    let material = match (x & 1) | ((y & 1) << 1) | ((z & 1) << 2) {
                        0 => lo,
                        1 => hi,
                        c => 1 + ((region + c) % 7) as u16,
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
        dense
    }

    /// M1-07 acceptance: on all five fixtures every band holds only nodes of
    /// its own level, the root is the sole node in the top band, level 0 stays
    /// unused, and expanding reproduces the source dense buffer exactly.
    #[test]
    fn bfs_build_fixtures_are_level_correct_and_rebuild_dense() {
        for (name, dense) in fixtures() {
            let dag = ChunkDag::from_dense(&dense);

            if dense.iter().all(|&voxel| voxel == 0) {
                assert!(dag.is_empty(), "{name}");
                assert_eq!(dag.root(), None, "{name}");
                assert_eq!(dag.node_words(), 0, "{name}");
                assert_eq!(dag.leaf_words(), 0, "{name}");
                continue;
            }

            let root = dag.root().unwrap_or_else(|| panic!("{name}: missing root"));
            assert_eq!(root.level(), ROOT_LEVEL, "{name}");
            assert_eq!(root.index(), 0, "{name}: the root band holds only the root");
            assert_eq!(dag.bands().len(ROOT_LEVEL), 1, "{name}");
            assert_eq!(dag.bands().len(0), 0, "{name}: level 0 is never built");

            for level in 1..=ROOT_LEVEL {
                for (index, node) in dag.bands().band(level).iter().enumerate() {
                    assert_eq!(node.level(), level, "{name}: band {level}, slot {index}");
                    let node_ref = NodeRef::new(level, index as u64);
                    assert_eq!(dag.bands().get(node_ref), Some(*node), "{name}");
                }
                for (node_ref, node) in dag.bands().iter_level(level) {
                    assert_eq!(node_ref.level(), level, "{name}");
                    assert_eq!(node.level(), level, "{name}");
                }
            }

            let mut rebuilt = vec![0u16; CHUNK_VOLUME];
            dag.expand(&mut rebuilt);
            assert_eq!(rebuilt, dense, "{name}: dense rebuild differs");
        }
    }

    /// Bands are numbered breadth-first: walking the built DAG level by level
    /// yields slot 0, 1, 2, ... in every band, and leaf runs follow the same
    /// order. The fixture has no repeated run, so no slot is deduplicated away.
    #[test]
    fn bfs_build_bands_are_numbered_in_breadth_first_order() {
        let dense = unique_dense();
        let dag = ChunkDag::from_dense(&dense);

        for level in 1..ROOT_LEVEL {
            assert_eq!(
                dag.bands().len(level),
                level_nodes(level) as u64,
                "level {level} lost a slot to unexpected dedup"
            );
        }
        assert_eq!(dag.bands().len(ROOT_LEVEL), 1);
        assert_eq!(dag.leaf_words(), CHUNK_VOLUME as u64);

        let mut frontier = vec![dag.root().expect("unique fixture is non-empty")];
        assert_eq!(frontier[0].level(), ROOT_LEVEL);

        for level in (2..=ROOT_LEVEL).rev() {
            assert_eq!(frontier.len(), level_nodes(level));

            let mut next = Vec::with_capacity(level_nodes(level - 1));
            for &parent_ref in &frontier {
                let parent = dag
                    .bands()
                    .get(parent_ref)
                    .expect("every frontier reference is live");
                assert_eq!(parent.mask(), 0xFF, "unique fixture fills every child");

                for c in 0..8u32 {
                    let index = parent.child_index(c).expect("a full mask has every child");
                    let child_ref = NodeRef::new(level - 1, index);
                    assert!(dag.bands().get(child_ref).is_some());
                    next.push(child_ref);
                }
            }

            let indices: Vec<u64> = next.iter().map(|node| node.index()).collect();
            let expected: Vec<u64> = (0..next.len() as u64).collect();
            assert_eq!(indices, expected, "band {} is not in BFS order", level - 1);
            frontier = next;
        }

        assert_eq!(frontier.len(), level_nodes(1));
        let leaf_bases: Vec<u64> = frontier
            .iter()
            .map(|node| dag.bands().get(*node).unwrap().base())
            .collect();
        let expected_bases: Vec<u64> = (0..leaf_bases.len() as u64)
            .map(|run| run * LEAF_RUN_MATERIALS as u64)
            .collect();
        assert_eq!(
            leaf_bases, expected_bases,
            "leaf runs must follow the BFS order of their level-1 parents"
        );
    }

    /// M1-08 acceptance: `get` equals the dense buffer on every coordinate of
    /// three fixtures, including the empty chunk's absent root.
    #[test]
    fn get_matches_dense_on_every_coordinate_of_three_fixtures() {
        for (name, dense) in fixtures() {
            if !matches!(name, "air" | "slab" | "noise") {
                continue;
            }
            let dag = ChunkDag::from_dense(&dense);
            for z in 0..CHUNK_DIM as u32 {
                for y in 0..CHUNK_DIM as u32 {
                    for x in 0..CHUNK_DIM as u32 {
                        let expected = dense[local_index(x, y, z) as usize];
                        assert_eq!(dag.get(x, y, z), expected, "{name} at ({x}, {y}, {z})");
                    }
                }
            }
        }
    }

    /// M1-08 acceptance: one million random coordinates on the noise fixture
    /// agree with the dense buffer. The seed is fixed, so a failure reproduces.
    #[test]
    fn get_matches_dense_on_random_noise_coordinates() {
        let (_, dense) = fixtures()
            .into_iter()
            .find(|(name, _)| *name == "noise")
            .expect("the noise fixture is part of the set");
        let dag = ChunkDag::from_dense(&dense);

        let mut state = 0x0bad_cafe_d00d_1234u64;
        for _ in 0..1_000_000 {
            let x = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            let y = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            let z = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            let expected = dense[local_index(x, y, z) as usize];
            assert_eq!(dag.get(x, y, z), expected, "noise at ({x}, {y}, {z})");
        }
    }

    /// M1-09 acceptance: expanding each of the five fixtures reproduces the
    /// source dense chunk byte for byte. The destination starts dirty, so
    /// absent octants must be cleared rather than left untouched.
    #[test]
    fn expand_matches_dense_on_five_fixtures() {
        for (name, dense) in fixtures() {
            let dag = ChunkDag::from_dense(&dense);
            let mut rebuilt = vec![u16::MAX; CHUNK_VOLUME];
            dag.expand(&mut rebuilt);
            assert_eq!(rebuilt, dense, "{name}: expanded buffer differs");
        }
    }

    #[test]
    #[should_panic(expected = "CHUNK_VOLUME")]
    fn expand_rejects_a_dense_length_mismatch() {
        ChunkDag::empty().expand(&mut [0; CHUNK_VOLUME - 1]);
    }

    #[test]
    #[should_panic(expected = "CHUNK_VOLUME")]
    fn bfs_build_rejects_a_dense_length_mismatch() {
        let _ = ChunkDag::from_dense(&[0; CHUNK_VOLUME - 1]);
    }
}
