//! Append-only per-level node bands and the leaf arena (M1-02).
//!
//! A chunk's internal nodes live in [`BAND_COUNT`] append-only bands, one per
//! level `0..=ROOT_LEVEL`. Each band owns a contiguous arena of 8 B nodes, so a
//! node index is stable for the lifetime of the arena: appending only grows the
//! tail and never renumbers or relocates an existing node. A [`NodeRef`]
//! identifies one slot as `(level, index)`, and every access checks the level
//! against the chunk's bands before touching memory.
//!
//! Every band's slot zero is **64 B aligned** (see [`BAND_ALIGN_BYTES`]), so a
//! full sibling run of eight nodes at a slot index that is a multiple of eight
//! occupies exactly one cache line. [`crate::run::RunAllocator`] relies on
//! this when it hands out aligned runs.
//!
//! Leaves are not internal nodes. They live in a separate arena of `u16`
//! material IDs (air is `0`); M1-06 groups those slots into eight-material leaf
//! runs. Keeping leaves out of the node bands means an internal band never
//! mixes 8 B words with 2 B payloads.
//!
//! [`Bands::level_start`] returns `levelStart[ROOT_LEVEL + 1]`: the offset of
//! each band in the flattened node pool obtained by concatenating bands in
//! level order. The host keeps bands as separate vectors; the flattened offset
//! table is the form serialization and the GPU upload consume, and it is
//! snapshotted once per chunk version.
//!
//! # Example
//!
//! ```
//! use voxel_storage::bands::Bands;
//! use voxel_storage::node_word::Node;
//!
//! let mut bands = Bands::new();
//! let root = bands.append(6, Node::new(0b0000_0011, 6, 0));
//! let child = bands.append(5, Node::new(0b0000_0001, 5, 0));
//!
//! assert_eq!((root.level(), root.index()), (6, 0));
//! assert_eq!((child.level(), child.index()), (5, 0));
//! assert_eq!(bands.len(6), 1);
//! assert_eq!(bands.range(6), 0..1);
//! assert_eq!(bands.get(child), Some(Node::new(0b0000_0001, 5, 0)));
//!
//! let leaf = bands.append_leaf(7);
//! assert_eq!(leaf, 0);
//! assert_eq!(bands.leaves(), &[7]);
//! ```

use core::fmt;
use core::ops::Range;

use voxel_core::constants::ROOT_LEVEL;

use crate::node_word::{Node, NodeRef};

/// Byte alignment of slot zero in every band, and of one full sibling run.
///
/// Eight 8 B nodes are exactly one cache line. Aligning slot zero makes every
/// full run — a run at a slot index that is a multiple of eight — start on a
/// line, so one fetch covers the whole sibling group (ADR-011).
pub const BAND_ALIGN_BYTES: usize = 64;

/// Size of one node slot in bytes.
const NODE_SLOT_BYTES: usize = core::mem::size_of::<Node>();

/// Worst-case number of leading slots needed to reach a 64 B boundary.
const MAX_SLACK_SLOTS: usize = BAND_ALIGN_BYTES / NODE_SLOT_BYTES;

/// Placeholder word for slots that are never read: mask 0, level 0, base 0.
const PADDING_NODE: Node = Node::decode(0);

/// Append-only storage for one level band whose slot zero is 64 B aligned.
///
/// A `Vec<Node>` only guarantees 8 B alignment, which is not enough for a full
/// run to occupy exactly one cache line. This wrapper over-allocates by the
/// worst-case slack and keeps up to seven leading slots so that the first
/// published node starts on a 64 B boundary. Growth may move the backing
/// allocation, so it slides the live slots back onto a boundary; published
/// slot indices are unaffected.
struct AlignedBand {
    // Invariant: `slack + len() == cells.len()`, and
    // `(cells.as_ptr() as usize + slack * NODE_SLOT_BYTES) % BAND_ALIGN_BYTES == 0`
    // once the band has storage.
    cells: Vec<Node>,
    slack: usize,
}

impl AlignedBand {
    fn new() -> Self {
        Self {
            cells: Vec::new(),
            slack: 0,
        }
    }

    fn with_capacity(nodes: usize) -> Self {
        let mut band = Self {
            cells: Vec::with_capacity(nodes + MAX_SLACK_SLOTS),
            slack: 0,
        };
        band.realign();
        band
    }

    fn len(&self) -> usize {
        self.cells.len() - self.slack
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn as_slice(&self) -> &[Node] {
        &self.cells[self.slack..]
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = Node> + '_ {
        self.as_slice().iter().copied()
    }

    fn push(&mut self, node: Node) {
        // Reserve the new slot plus the worst-case slack in one go. `reserve`
        // may move the buffer, so alignment is re-established afterwards.
        if self.cells.len() + MAX_SLACK_SLOTS + 1 > self.cells.capacity() {
            let extra = self.len().max(4) + 2 * MAX_SLACK_SLOTS;
            self.cells.reserve(extra);
        }
        self.realign();
        self.cells.push(node);
    }

    /// Slides the live slots so that slot zero is 64 B aligned.
    fn realign(&mut self) {
        loop {
            let target = Self::slack_slots(self.cells.as_ptr());
            let len = self.len();
            let needed = target + len;
            if needed <= self.cells.capacity() {
                if self.cells.len() < needed {
                    self.cells.resize(needed, PADDING_NODE);
                }
                if target != self.slack {
                    if len != 0 {
                        self.cells.copy_within(self.slack..self.slack + len, target);
                    }
                    self.cells.truncate(needed);
                    self.slack = target;
                }
                debug_assert_eq!(
                    (self.cells.as_ptr() as usize + self.slack * NODE_SLOT_BYTES)
                        % BAND_ALIGN_BYTES,
                    0,
                    "band slot zero must be cache-line aligned"
                );
                return;
            }
            // The move would not fit; grow and recompute against the new
            // allocation instead of trusting the old pointer.
            self.cells.reserve(needed - self.cells.len());
        }
    }

    fn slack_slots(ptr: *const Node) -> usize {
        let misalign = (ptr as usize) % BAND_ALIGN_BYTES;
        if misalign == 0 {
            0
        } else {
            (BAND_ALIGN_BYTES - misalign) / NODE_SLOT_BYTES
        }
    }
}

impl Clone for AlignedBand {
    fn clone(&self) -> Self {
        // A clone gets its own allocation, so it must re-establish alignment
        // rather than copy the source's slack offset.
        let mut band = Self::with_capacity(self.len());
        for &node in self.as_slice() {
            band.push(node);
        }
        band
    }
}

impl fmt::Debug for AlignedBand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl PartialEq for AlignedBand {
    fn eq(&self, other: &Self) -> bool {
        // Slack is an allocation detail, not band contents.
        self.as_slice() == other.as_slice()
    }
}

impl Eq for AlignedBand {}

/// Number of internal level bands: one for each level `0..=ROOT_LEVEL`.
pub const BAND_COUNT: usize = ROOT_LEVEL as usize + 1;

/// Resolves a level to its band slot, panicking outside the chunk's bands.
///
/// Every public entry point that names a level goes through this helper, so a
/// level from a decoded node word or a caller cannot index another level's
/// memory silently.
#[inline]
fn band_index(level: u32) -> usize {
    assert!(
        level <= ROOT_LEVEL,
        "band level {level} is outside 0..={ROOT_LEVEL}"
    );
    level as usize
}

/// The per-level append-only node arenas plus the leaf material arena.
///
/// The arena is append-only by construction: there is no removal, truncation or
/// mutable slice access. Reallocation of a band's backing vector changes where
/// its bytes live in the process, but never a [`NodeRef`] index, which is why
/// indices may be published before later appends.
///
/// The meaning of a level follows the caller; this type only enforces that
/// level `l` is stored in band `l`. The pool numbering used by [`NodeRef`]
/// (root at [`ROOT_LEVEL`], descending children) and the core numbering (root
/// at level `0`) are both expressible; a node word's `level` field must agree
/// with the band it is appended to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bands {
    levels: [AlignedBand; BAND_COUNT],
    leaves: Vec<u16>,
}

impl Bands {
    /// Creates an empty set of bands and an empty leaf arena.
    pub fn new() -> Self {
        Self {
            levels: core::array::from_fn(|_| AlignedBand::new()),
            leaves: Vec::new(),
        }
    }

    /// Creates empty bands with room for `nodes_per_band` nodes per level and
    /// `leaves` leaf material slots.
    ///
    /// Capacity is a hint, not a limit: appends grow the owning band normally.
    pub fn with_capacity(nodes_per_band: usize, leaves: usize) -> Self {
        Self {
            levels: core::array::from_fn(|_| AlignedBand::with_capacity(nodes_per_band)),
            leaves: Vec::with_capacity(leaves),
        }
    }

    /// Appends `node` to `level`'s band and returns its stable reference.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`, the slot index would exceed the `u48`
    /// node address space, or `node.level()` does not name `level`. The node
    /// level check keeps a band from carrying a word that claims to belong to
    /// another level; the invariant checker (M1-14) relies on it.
    pub fn append(&mut self, level: u32, node: Node) -> NodeRef {
        let band = band_index(level);
        assert_eq!(
            node.level(),
            level,
            "node level does not match its band level {level}"
        );

        let index = self.levels[band].len() as u64;
        // Construct before pushing, so an index outside the u48 space leaves
        // the arena untouched.
        let node_ref = NodeRef::new(level, index);
        self.levels[band].push(node);
        node_ref
    }

    /// Returns the contiguous slot range occupied by `level`'s band.
    ///
    /// Band indices are local to the band and start at zero, so this is
    /// `0..len(level)`; the start never moves when another level grows.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    pub fn range(&self, level: u32) -> Range<u64> {
        let band = band_index(level);
        0..self.levels[band].len() as u64
    }

    /// Number of nodes in `level`'s band.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    pub fn len(&self, level: u32) -> u64 {
        self.levels[band_index(level)].len() as u64
    }

    /// Iterates `level`'s band in append order, yielding each node with its
    /// stable reference.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    pub fn iter_level(&self, level: u32) -> impl ExactSizeIterator<Item = (NodeRef, Node)> + '_ {
        let band = band_index(level);
        self.levels[band]
            .iter()
            .enumerate()
            .map(move |(index, node)| (NodeRef::new(band as u32, index as u64), node))
    }

    /// Returns `level`'s band as one contiguous slice.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    pub fn band(&self, level: u32) -> &[Node] {
        self.levels[band_index(level)].as_slice()
    }

    /// Reads the node named by `node_ref`.
    ///
    /// Returns `None` when the reference is in range for its level but its
    /// index is past the band's tail. A reference can only name an in-bounds
    /// level because [`NodeRef`] validates it at construction; the level is
    /// still checked here on every access.
    pub fn get(&self, node_ref: NodeRef) -> Option<Node> {
        let band = band_index(node_ref.level());
        let index = usize::try_from(node_ref.index()).ok()?;
        self.levels[band].as_slice().get(index).copied()
    }

    /// Returns `levelStart[ROOT_LEVEL + 1]` for the current band lengths.
    ///
    /// Entry `l` is the first slot of level `l` in the flattened pool formed by
    /// concatenating bands `0, 1, ..., ROOT_LEVEL`. Because the arenas are
    /// separate, this table is recomputed from the lengths; a chunk uploads a
    /// snapshot of it once per version (M4-09).
    pub fn level_start(&self) -> [u64; BAND_COUNT] {
        let mut starts = [0u64; BAND_COUNT];
        let mut next = 0u64;
        for (start, band) in starts.iter_mut().zip(&self.levels) {
            *start = next;
            next += band.len() as u64;
        }
        starts
    }

    /// Total number of internal nodes across all bands.
    pub fn total_len(&self) -> u64 {
        self.levels.iter().map(|band| band.len() as u64).sum()
    }

    /// True when every band and the leaf arena are empty.
    pub fn is_empty(&self) -> bool {
        self.levels.iter().all(AlignedBand::is_empty) && self.leaves.is_empty()
    }

    /// Appends one `u16` material ID to the leaf arena and returns its slot.
    ///
    /// Slot zero is the first material, not a run index; M1-06 groups these
    /// slots into eight-material leaf runs. Material `0` means air.
    pub fn append_leaf(&mut self, material: u16) -> u64 {
        let index = self.leaves.len() as u64;
        self.leaves.push(material);
        index
    }

    /// Contiguous slot range occupied by the leaf arena.
    pub fn leaf_range(&self) -> Range<u64> {
        0..self.leaves.len() as u64
    }

    /// Number of `u16` material slots in the leaf arena.
    pub fn leaf_len(&self) -> u64 {
        self.leaves.len() as u64
    }

    /// The leaf arena as one contiguous slice.
    pub fn leaves(&self) -> &[u16] {
        &self.leaves
    }

    /// Reads one leaf material slot, or `None` past the tail.
    pub fn leaf(&self, index: u64) -> Option<u16> {
        let index = usize::try_from(index).ok()?;
        self.leaves.get(index).copied()
    }
}

impl Default for Bands {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(level: u32, tag: u64) -> Node {
        // The tag lives in the base field; mask is non-zero so the word is not
        // mistaken for an empty node by readers of the test failures.
        Node::new(1, level, tag)
    }

    #[test]
    fn bands_start_empty_with_one_range_per_level() {
        let bands = Bands::new();

        assert!(bands.is_empty());
        assert_eq!(bands.total_len(), 0);
        assert_eq!(bands.leaf_len(), 0);
        assert_eq!(bands.leaf_range(), 0..0);
        assert!(bands.leaves().is_empty());

        for level in 0..=ROOT_LEVEL {
            assert_eq!(bands.len(level), 0);
            assert_eq!(bands.range(level), 0..0);
            assert!(bands.band(level).is_empty());
            assert_eq!(bands.iter_level(level).count(), 0);
        }

        assert_eq!(bands.level_start(), [0; BAND_COUNT]);
    }

    #[test]
    fn bands_append_keeps_levels_and_indices_separate() {
        let mut bands = Bands::new();

        let level_2_first = bands.append(2, node(2, 20));
        let level_5_first = bands.append(5, node(5, 50));
        let level_2_second = bands.append(2, node(2, 21));

        assert_eq!((level_2_first.level(), level_2_first.index()), (2, 0));
        assert_eq!((level_5_first.level(), level_5_first.index()), (5, 0));
        assert_eq!((level_2_second.level(), level_2_second.index()), (2, 1));

        // The same index in different bands names different nodes.
        assert_eq!(bands.get(level_2_first), Some(node(2, 20)));
        assert_eq!(bands.get(level_5_first), Some(node(5, 50)));

        assert_eq!(bands.len(2), 2);
        assert_eq!(bands.len(5), 1);
        assert_eq!(bands.band(2), &[node(2, 20), node(2, 21)]);
        assert_eq!(bands.band(5), &[node(5, 50)]);

        let collected: Vec<_> = bands.iter_level(2).collect();
        assert_eq!(
            collected,
            vec![(level_2_first, node(2, 20)), (level_2_second, node(2, 21))]
        );
    }

    /// The primary M1-02 property: after sustained appends every band is still
    /// one contiguous range, old references still resolve, and `levelStart`
    /// exactly tiles the flattened pool.
    #[test]
    fn bands_ranges_stay_contiguous_after_10k_appends() {
        const APPENDS: u64 = 10_000;

        let mut bands = Bands::new();
        let mut first_refs = [None; BAND_COUNT];

        for i in 0..APPENDS {
            let level = (i % BAND_COUNT as u64) as u32;
            let tag = i + 1;
            let node_ref = bands.append(level, node(level, tag));

            // Remember the first reference in each band; it must keep resolving
            // to the same node after many reallocations of that band.
            first_refs[level as usize].get_or_insert(node_ref);
            assert_eq!(node_ref.index(), bands.len(level) - 1);
        }

        for (level, first) in (0..=ROOT_LEVEL).zip(first_refs) {
            let first = first.expect("every level was appended to");
            // The first append to each level is at i == level, so its tag is
            // level + 1.
            assert_eq!(bands.get(first), Some(node(level, level as u64 + 1)));
        }

        // Every band occupies exactly one contiguous range ending at len.
        for level in 0..=ROOT_LEVEL {
            let range = bands.range(level);
            let len = bands.len(level);
            assert_eq!(range, 0..len);

            let band = bands.band(level);
            assert_eq!(band.len() as u64, len);
            let iterated: Vec<_> = bands.iter_level(level).collect();
            assert_eq!(iterated.len() as u64, len);
            for (index, (node_ref, stored)) in iterated.into_iter().enumerate() {
                assert_eq!(node_ref, NodeRef::new(level, index as u64));
                assert_eq!(stored, band[index]);
            }
        }

        // levelStart is the cumulative length; the final entry plus its length
        // covers every node exactly once.
        let starts = bands.level_start();
        let mut expected_start = 0;
        for level in 0..=ROOT_LEVEL {
            assert_eq!(starts[level as usize], expected_start);
            expected_start += bands.len(level);
        }
        assert_eq!(expected_start, bands.total_len());
        assert_eq!(expected_start, APPENDS);

        let flattened: Vec<_> = (0..=ROOT_LEVEL)
            .flat_map(|level| bands.band(level).iter().copied())
            .collect();
        assert_eq!(flattened.len() as u64, APPENDS);
        for level in 0..=ROOT_LEVEL {
            let start = starts[level as usize] as usize;
            let end = start + bands.len(level) as usize;
            assert_eq!(&flattened[start..end], bands.band(level));
        }
    }

    #[test]
    fn bands_level_start_tracks_band_growth() {
        let mut bands = Bands::new();
        assert_eq!(bands.level_start(), [0; BAND_COUNT]);

        for _ in 0..3 {
            bands.append(0, node(0, 1));
        }
        assert_eq!(bands.level_start()[0], 0);
        assert_eq!(bands.level_start()[1], 3);

        bands.append(1, node(1, 2));
        bands.append(1, node(1, 3));
        assert_eq!(bands.level_start()[1], 3);
        assert_eq!(bands.level_start()[2], 5);
        assert_eq!(bands.total_len(), 5);

        // Growing an earlier band shifts the virtual starts of later bands, not
        // the band-local ranges or any stored reference.
        bands.append(0, node(0, 4));
        assert_eq!(bands.level_start()[1], 4);
        assert_eq!(bands.range(1), 0..2);
        assert_eq!(bands.band(1), &[node(1, 2), node(1, 3)]);
    }

    #[test]
    fn bands_leaf_arena_appends_materials_and_keeps_air() {
        let mut bands = Bands::new();

        assert_eq!(bands.append_leaf(0), 0); // air
        assert_eq!(bands.append_leaf(7), 1);
        assert_eq!(bands.append_leaf(300), 2);
        assert_eq!(bands.append_leaf(0), 3);

        assert_eq!(bands.leaf_len(), 4);
        assert_eq!(bands.leaf_range(), 0..4);
        assert_eq!(bands.leaves(), &[0, 7, 300, 0]);
        assert_eq!(bands.leaf(0), Some(0));
        assert_eq!(bands.leaf(2), Some(300));
        assert_eq!(bands.leaf(4), None);
        assert!(!bands.is_empty());
    }

    #[test]
    fn bands_get_returns_none_past_a_band_tail() {
        let mut bands = Bands::new();
        let first = bands.append(3, node(3, 9));

        assert_eq!(bands.get(first), Some(node(3, 9)));
        assert_eq!(bands.get(NodeRef::new(3, 1)), None);
        assert_eq!(bands.get(NodeRef::new(4, 0)), None);
    }

    /// Slot zero of every band starts on a cache line, before and after the
    /// backing allocation moves. M1-03's full-run alignment depends on it.
    #[test]
    fn bands_slot_zero_is_cache_line_aligned() {
        for nodes in [0usize, 1, 7, 8, 9, 64, 1000] {
            let mut bands = Bands::with_capacity(nodes, 0);
            for level in 0..=ROOT_LEVEL {
                assert_eq!(
                    bands.band(level).as_ptr() as usize % BAND_ALIGN_BYTES,
                    0,
                    "{nodes} reserved nodes, level {level}"
                );
            }

            for i in 0..nodes {
                let level = (i % BAND_COUNT) as u32;
                bands.append(level, node(level, i as u64 + 1));
            }

            for level in 0..=ROOT_LEVEL {
                assert_eq!(
                    bands.band(level).as_ptr() as usize % BAND_ALIGN_BYTES,
                    0,
                    "{nodes} appended nodes, level {level}"
                );
            }
        }
    }

    #[test]
    fn bands_clone_reestablish_alignment_and_compare_contents() {
        let mut bands = Bands::new();
        for i in 0..200 {
            let level = (i % BAND_COUNT) as u32;
            bands.append(level, node(level, i as u64 + 1));
        }

        let clone = bands.clone();
        // A clone has its own allocation and therefore its own slack; the
        // comparison must be over live slots, not the allocation detail.
        assert_eq!(clone, bands);
        for level in 0..=ROOT_LEVEL {
            assert_eq!(clone.band(level).as_ptr() as usize % BAND_ALIGN_BYTES, 0);
            assert_eq!(clone.band(level), bands.band(level));
        }
    }

    #[test]
    fn bands_accepts_the_deepest_level() {
        let mut bands = Bands::new();
        let deepest = bands.append(ROOT_LEVEL, node(ROOT_LEVEL, 42));

        assert_eq!(deepest.level(), ROOT_LEVEL);
        assert_eq!(bands.len(ROOT_LEVEL), 1);
        assert_eq!(bands.band(ROOT_LEVEL), &[node(ROOT_LEVEL, 42)]);
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn bands_append_checks_the_level_bound() {
        let mut bands = Bands::new();
        let _ = bands.append(ROOT_LEVEL + 1, Node::new(1, 0, 0));
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn bands_range_checks_the_level_bound() {
        let bands = Bands::new();
        let _ = bands.range(ROOT_LEVEL + 1);
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn bands_len_checks_the_level_bound() {
        let bands = Bands::new();
        let _ = bands.len(ROOT_LEVEL + 1);
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn bands_iter_level_checks_the_level_bound() {
        let bands = Bands::new();
        let _ = bands.iter_level(ROOT_LEVEL + 1);
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn bands_band_checks_the_level_bound() {
        let bands = Bands::new();
        let _ = bands.band(ROOT_LEVEL + 1);
    }

    #[test]
    #[should_panic(expected = "does not match its band")]
    fn bands_append_rejects_a_word_from_another_level() {
        let mut bands = Bands::new();
        let _ = bands.append(2, node(1, 0));
    }
}
