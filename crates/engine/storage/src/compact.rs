//! Offline band compaction (M1-12).
//!
//! Bands are append-only during normal operation, so dead runs and alignment
//! padding accumulate after edits. Compaction reclaims that space by building a
//! fresh copy of **one** band at a time:
//!
//! 1. Walk the live DAG from the root and collect the runs that are still
//!    referenced.
//! 2. Copy those runs into a fresh band, in ascending old-base order, using the
//!    same [`RunAllocator`](crate::run::RunAllocator) rules as the build and edit
//!    paths. A full run (`mask == 0xFF`) is still cache-line aligned; partial
//!    runs are still packed.
//! 3. Remap every live parent's `base` to the run's new slot.
//! 4. For the leaf arena, copy the live leaf runs and rebuild the level-1
//!    parents' leaf bases.
//!
//! Renumbering invalidates every [`NodeRef`] into the old band, so compaction
//! may run only when the chunk is quiescent. [`ChunkDag::retain_root`] and
//! [`ChunkDag::retain_frame`] register a live root or in-flight frame; the
//! compaction entry points panic while either counter is non-zero. The guard is
//! deliberately a runtime check: M4 wires the two A/B roots and the frame epoch
//! list to the same counters (M4-12), and a torn band must never be published.
//!
//! # Trigger
//!
//! [`ChunkDag::compaction_candidate`] returns the first arena whose dead or
//! padding bytes exceed 25%. [`ChunkDag::compact_if_needed`] compacts exactly
//! that one arena. [`ChunkDag::compact`] is the explicit full offline pass and
//! renumbers every band and the leaf arena regardless of the threshold.
//!
//! # Example
//!
//! ```
//! use voxel_storage::{ChunkDag, CHUNK_VOLUME, local_index};
//!
//! let mut dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
//! for i in 0..8u32 {
//!     dag.apply_edits([(local_index(i, 0, 0), 4)]);
//! }
//!
//! let before = dag.get(0, 0, 0);
//! let stats = dag.compact();
//! assert!(stats.reclaimed_bytes() > 0);
//! assert_eq!(dag.get(0, 0, 0), before);
//! ```

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

use voxel_core::constants::ROOT_LEVEL;

use crate::bands::BAND_COUNT;
use crate::chunk::{AccessCounters, ChunkDag};
use crate::leaf::LEAF_MATERIAL_BYTES;
use crate::node_word::{Node, NodeRef};
use crate::run::{NODE_BYTES, RunAllocator};

/// One arena that compaction can renumber.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompactBand {
    /// The `u16` material arena shared by level-1 parents.
    Leaves,
    /// Internal level band `level`.
    Level(u32),
}

/// Space reclaimed by one or more band compactions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompactionStats {
    /// Number of arenas rebuilt (internal bands plus the leaf arena).
    pub bands: u32,
    /// Live runs copied into the new arenas.
    pub runs: u64,
    /// Slot bytes in the old arenas before compaction.
    pub bytes_before: u64,
    /// Slot bytes in the new arenas after compaction.
    pub bytes_after: u64,
}

impl CompactionStats {
    /// Bytes removed from the chunk's arenas.
    pub fn reclaimed_bytes(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }

    fn merge(&mut self, other: Self) {
        self.bands += other.bands;
        self.runs += other.runs;
        self.bytes_before += other.bytes_before;
        self.bytes_after += other.bytes_after;
    }
}

/// Dead-slot accounting for one arena.
///
/// A dead slot is any slot not part of a live run: retired run bytes and
/// alignment padding both count. The 25% compaction trigger compares
/// [`Self::dead_bytes`] against [`Self::bytes`], not slot counts, so internal
/// bands and the narrower leaf arena can be compared consistently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BandWaste {
    /// Total slots in the arena.
    pub slots: u64,
    /// Slots belonging to live runs.
    pub live_slots: u64,
    /// Slots that are retired or padding.
    pub dead_slots: u64,
    /// Total slot bytes in the arena.
    pub bytes: u64,
    /// Bytes held by retired slots and padding.
    pub dead_bytes: u64,
}

impl BandWaste {
    fn new(slots: u64, live_slots: u64, slot_bytes: u64) -> Self {
        let live_slots = live_slots.min(slots);
        let dead_slots = slots - live_slots;
        Self {
            slots,
            live_slots,
            dead_slots,
            bytes: slots * slot_bytes,
            dead_bytes: dead_slots * slot_bytes,
        }
    }

    /// True when dead or padding bytes are strictly more than 25% of the arena.
    pub fn exceeds_quarter(&self) -> bool {
        self.dead_bytes > 0 && self.dead_bytes * 4 > self.bytes
    }
}

/// Keeps the chunk's current root live, blocking compaction.
///
/// The guard registers itself in [`ChunkDag::retain_root`] and unregisters on
/// drop. While any guard is alive, every compaction entry point panics rather
/// than renumbering a band that the retained root can still read.
#[derive(Debug)]
pub struct RootGuard {
    access: Arc<AccessCounters>,
    root: Option<NodeRef>,
}

impl RootGuard {
    /// The root this guard keeps live.
    pub const fn root(&self) -> Option<NodeRef> {
        self.root
    }
}

impl Drop for RootGuard {
    fn drop(&mut self) {
        self.access.release_root();
    }
}

/// Marks one frame as still reading the chunk, blocking compaction.
///
/// Drop it only after the frame's GPU work has completed. The guard is a
/// runtime epoch placeholder; M4-12 attaches it to the real frame fence.
#[derive(Debug)]
pub struct FrameGuard {
    access: Arc<AccessCounters>,
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        self.access.release_frame();
    }
}

/// Live references reachable from the current root.
struct Reachable {
    /// Per level, a bit per band slot. `false` means the slot is retired.
    nodes: [Vec<bool>; BAND_COUNT],
}

impl ChunkDag {
    /// Registers the current root as live and returns a guard.
    ///
    /// Compaction panics until the guard is dropped.
    pub fn retain_root(&self) -> RootGuard {
        self.access().retain_root();
        RootGuard {
            access: Arc::clone(self.access()),
            root: self.root(),
        }
    }

    /// Marks one in-flight frame as reading this chunk.
    ///
    /// Compaction panics until the guard is dropped.
    pub fn retain_frame(&self) -> FrameGuard {
        self.access().retain_frame();
        FrameGuard {
            access: Arc::clone(self.access()),
        }
    }

    /// Number of live-root guards currently held.
    pub fn live_root_count(&self) -> u32 {
        self.access().live_roots()
    }

    /// Number of in-flight-frame guards currently held.
    pub fn in_flight_frame_count(&self) -> u32 {
        self.access().in_flight_frames()
    }

    /// Dead-slot accounting for one internal band.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    pub fn band_waste(&self, level: u32) -> BandWaste {
        let reachable = self.reachable();
        self.band_waste_with(level, &reachable)
    }

    /// Dead-slot accounting for the leaf arena.
    pub fn leaf_waste(&self) -> BandWaste {
        let reachable = self.reachable();
        self.leaf_waste_with(&reachable)
    }

    /// The first arena whose dead or padding bytes exceed 25%.
    ///
    /// The leaf arena is considered first, then internal bands in ascending
    /// level order. `None` means every arena is below the compaction threshold.
    pub fn compaction_candidate(&self) -> Option<CompactBand> {
        let reachable = self.reachable();
        if self.leaf_waste_with(&reachable).exceeds_quarter() {
            return Some(CompactBand::Leaves);
        }
        for level in 1..=ROOT_LEVEL {
            if self.band_waste_with(level, &reachable).exceeds_quarter() {
                return Some(CompactBand::Level(level));
            }
        }
        None
    }

    /// Compacts one arena if it exceeds the 25% dead-or-padding threshold.
    ///
    /// This is the production entry point; call it only while the chunk is
    /// quiescent.
    pub fn compact_if_needed(&mut self) -> Option<CompactionStats> {
        match self.compaction_candidate()? {
            CompactBand::Leaves => Some(self.compact_leaves()),
            CompactBand::Level(level) => Some(self.compact_band(level)),
        }
    }

    /// Renumbers `level`'s band and remaps every live parent's base into it.
    ///
    /// The root band is handled specially: it keeps only the current root at
    /// slot zero. Band zero is never referenced and is simply cleared.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`, if a live root or in-flight frame still
    /// references the chunk, or if the live DAG is inconsistent (a parent points
    /// outside the runs collected from the band).
    pub fn compact_band(&mut self, level: u32) -> CompactionStats {
        self.assert_quiescent();
        self.compact_band_inner(level)
    }

    /// Rebuilds the leaf arena and remaps every live level-1 parent's leaf base.
    ///
    /// # Panics
    ///
    /// Panics if a live root or in-flight frame still references the chunk, or
    /// if the live DAG is inconsistent.
    pub fn compact_leaves(&mut self) -> CompactionStats {
        self.assert_quiescent();
        self.compact_leaves_inner()
    }

    /// Full offline compaction: rebuilds the leaf arena and every internal band.
    ///
    /// This ignores the 25% trigger because the caller explicitly asked for a
    /// quiescent renumber. Assets and save paths use it to publish dense bands;
    /// the per-frame path uses [`Self::compact_if_needed`].
    ///
    /// # Panics
    ///
    /// Panics if a live root or in-flight frame still references the chunk.
    pub fn compact(&mut self) -> CompactionStats {
        self.assert_quiescent();
        let mut stats = self.compact_leaves_inner();
        for level in 0..=ROOT_LEVEL {
            stats.merge(self.compact_band_inner(level));
        }
        stats
    }

    fn assert_quiescent(&self) {
        let roots = self.access().live_roots();
        assert_eq!(
            roots, 0,
            "cannot compact while {roots} live root(s) reference the chunk"
        );
        let frames = self.access().in_flight_frames();
        assert_eq!(
            frames, 0,
            "cannot compact while {frames} in-flight frame(s) reference the chunk"
        );
    }

    fn compact_band_inner(&mut self, level: u32) -> CompactionStats {
        let old_slots = self.bands().len(level);
        let reachable = self.reachable();

        if level == ROOT_LEVEL {
            let new_root = self
                .root()
                .map(|root| self.bands().get(root).expect("a live root names a node"));
            let new_slots = u64::from(new_root.is_some());
            self.bands_mut().replace_band(level, new_root);
            self.set_root(new_root.map(|_| NodeRef::new(ROOT_LEVEL, 0)));
            return CompactionStats {
                bands: 1,
                runs: new_slots,
                bytes_before: old_slots * NODE_BYTES,
                bytes_after: new_slots * NODE_BYTES,
            };
        }

        if level == 0 {
            // Level 0 is never built: level-1 parents point at the leaf arena.
            self.bands_mut().replace_band(level, core::iter::empty());
            return CompactionStats {
                bands: 1,
                runs: 0,
                bytes_before: old_slots * NODE_BYTES,
                bytes_after: 0,
            };
        }

        let runs = self.live_internal_runs(level, &reachable);
        let old_band = self.bands().band(level).to_vec();
        let mut allocator = RunAllocator::new();
        let mut new_nodes = Vec::with_capacity(old_band.len());
        let mut remap = BTreeMap::new();

        for (&old_base, &mask) in &runs {
            let count = u64::from(mask.count_ones());
            let allocation = allocator.allocate(mask);
            let new_base = allocation.base();
            while (new_nodes.len() as u64) < new_base {
                // Full-run alignment padding, level-correct and never referenced.
                new_nodes.push(Node::new(0, level, 0));
            }

            let start = usize::try_from(old_base).expect("run base does not fit usize");
            let end = start
                .checked_add(count as usize)
                .expect("run end overflows usize");
            let children = old_band.get(start..end).unwrap_or_else(|| {
                panic!("live run at level {level} slot {old_base} is past the band tail")
            });
            for &child in children {
                assert_eq!(
                    child.level(),
                    level,
                    "a run in band {level} contains a level {} node",
                    child.level()
                );
                new_nodes.push(child);
            }
            remap.insert(old_base, new_base);
        }

        debug_assert_eq!(new_nodes.len() as u64, allocator.next_slot());
        let new_slots = new_nodes.len() as u64;
        self.bands_mut().replace_band(level, new_nodes);
        self.remap_parent_bases(level, &remap, &reachable);

        CompactionStats {
            bands: 1,
            runs: runs.len() as u64,
            bytes_before: old_slots * NODE_BYTES,
            bytes_after: new_slots * NODE_BYTES,
        }
    }

    fn compact_leaves_inner(&mut self) -> CompactionStats {
        let old_slots = self.bands().leaf_len();
        let reachable = self.reachable();
        let runs = self.live_leaf_runs(&reachable);
        let old_leaves = self.bands().leaves().to_vec();
        let mut new_leaves = Vec::with_capacity(old_leaves.len());
        let mut remap = BTreeMap::new();

        for (&old_base, &count) in &runs {
            let start = usize::try_from(old_base).expect("leaf base does not fit usize");
            let end = start
                .checked_add(count as usize)
                .expect("leaf run end overflows usize");
            let materials = old_leaves.get(start..end).unwrap_or_else(|| {
                panic!("live leaf run at slot {old_base} is past the arena tail")
            });
            let new_base = new_leaves.len() as u64;
            new_leaves.extend_from_slice(materials);
            remap.insert(old_base, new_base);
        }

        let new_slots = new_leaves.len() as u64;
        self.bands_mut().replace_leaves(new_leaves);
        self.remap_leaf_parents(&remap, &reachable);

        CompactionStats {
            bands: 1,
            runs: runs.len() as u64,
            bytes_before: old_slots * LEAF_MATERIAL_BYTES,
            bytes_after: new_slots * LEAF_MATERIAL_BYTES,
        }
    }

    /// Walks the DAG from the current root and marks every reachable node slot.
    fn reachable(&self) -> Reachable {
        let mut nodes: [Vec<bool>; BAND_COUNT] = core::array::from_fn(|_| Vec::new());
        let Some(root) = self.root() else {
            return Reachable { nodes };
        };

        let mut stack = vec![root];
        while let Some(node_ref) = stack.pop() {
            let level = node_ref.level() as usize;
            let index = usize::try_from(node_ref.index()).expect("node index does not fit usize");
            let marks = &mut nodes[level];
            if marks.len() <= index {
                marks.resize(index + 1, false);
            }
            if marks[index] {
                continue;
            }
            marks[index] = true;

            let node = self
                .bands()
                .get(node_ref)
                .unwrap_or_else(|| panic!("live root reaches missing node {node_ref:?}"));
            if level <= 1 {
                continue; // level-1 children are leaf materials, not band nodes
            }
            for c in 0..8u32 {
                if node.mask() & (1 << c) == 0 {
                    continue;
                }
                let index = node
                    .child_index(c)
                    .expect("a present child has an in-range slot");
                stack.push(NodeRef::new(level as u32 - 1, index));
            }
        }

        Reachable { nodes }
    }

    fn band_waste_with(&self, level: u32, reachable: &Reachable) -> BandWaste {
        let slots = self.bands().len(level);
        let live_slots = if level == ROOT_LEVEL {
            u64::from(self.root().is_some())
        } else if level == 0 {
            0
        } else {
            self.live_internal_runs(level, reachable)
                .values()
                .map(|&mask| u64::from(mask.count_ones()))
                .sum()
        };
        BandWaste::new(slots, live_slots, NODE_BYTES)
    }

    fn leaf_waste_with(&self, reachable: &Reachable) -> BandWaste {
        let slots = self.bands().leaf_len();
        let live_slots = self
            .live_leaf_runs(reachable)
            .values()
            .copied()
            .sum::<u64>();
        BandWaste::new(slots, live_slots, LEAF_MATERIAL_BYTES)
    }

    /// Unique internal runs in `level`'s band, keyed by old base.
    ///
    /// Every live node in band `level + 1` contributes its run. Two parents may
    /// share a run only when the mask and ordered children match, so a shared
    /// base must report one mask.
    fn live_internal_runs(&self, level: u32, reachable: &Reachable) -> BTreeMap<u64, u8> {
        let mut runs = BTreeMap::new();
        if level == 0 || level >= ROOT_LEVEL {
            return runs;
        }
        let parent_level = level + 1;
        for (index, &live) in reachable.nodes[parent_level as usize].iter().enumerate() {
            if !live {
                continue;
            }
            let parent_ref = NodeRef::new(parent_level, index as u64);
            let parent = self
                .bands()
                .get(parent_ref)
                .expect("a live parent names a node");
            if parent.mask() == 0 {
                continue;
            }
            match runs.entry(parent.base()) {
                Entry::Occupied(entry) => assert_eq!(
                    *entry.get(),
                    parent.mask(),
                    "shared run at level {level} base {} has conflicting masks",
                    parent.base()
                ),
                Entry::Vacant(entry) => {
                    entry.insert(parent.mask());
                }
            }
        }
        runs
    }

    /// Unique leaf runs, keyed by old base, with their material counts.
    ///
    /// The leaf key excludes the mask, so two parents may share a base with
    /// different masks. They must still agree on the number of stored materials.
    fn live_leaf_runs(&self, reachable: &Reachable) -> BTreeMap<u64, u64> {
        let mut runs = BTreeMap::new();
        for (index, &live) in reachable.nodes[1].iter().enumerate() {
            if !live {
                continue;
            }
            let parent_ref = NodeRef::new(1, index as u64);
            let parent = self
                .bands()
                .get(parent_ref)
                .expect("a live level-1 parent names a node");
            let count = u64::from(parent.mask().count_ones());
            if count == 0 {
                continue;
            }
            match runs.entry(parent.base()) {
                Entry::Occupied(entry) => assert_eq!(
                    *entry.get(),
                    count,
                    "shared leaf run at base {} has conflicting lengths",
                    parent.base()
                ),
                Entry::Vacant(entry) => {
                    entry.insert(count);
                }
            }
        }
        runs
    }

    fn remap_parent_bases(
        &mut self,
        child_level: u32,
        remap: &BTreeMap<u64, u64>,
        reachable: &Reachable,
    ) {
        let parent_level = child_level + 1;
        for (index, &live) in reachable.nodes[parent_level as usize].iter().enumerate() {
            if !live {
                continue;
            }
            let parent_ref = NodeRef::new(parent_level, index as u64);
            let parent = self
                .bands()
                .get(parent_ref)
                .expect("a live parent names a node");
            if parent.mask() == 0 {
                continue;
            }
            let new_base = *remap.get(&parent.base()).unwrap_or_else(|| {
                panic!(
                    "live parent {parent_ref:?} points outside the compacted level \
                     {child_level} run set"
                )
            });
            if new_base != parent.base() {
                let remapped = Node::new(parent.mask(), parent_level, new_base);
                self.bands_mut().set_node(parent_ref, remapped);
            }
        }
    }

    fn remap_leaf_parents(&mut self, remap: &BTreeMap<u64, u64>, reachable: &Reachable) {
        for (index, &live) in reachable.nodes[1].iter().enumerate() {
            if !live {
                continue;
            }
            let parent_ref = NodeRef::new(1, index as u64);
            let parent = self
                .bands()
                .get(parent_ref)
                .expect("a live level-1 parent names a node");
            if parent.mask() == 0 {
                continue;
            }
            let new_base = *remap.get(&parent.base()).unwrap_or_else(|| {
                panic!(
                    "live level-1 parent {parent_ref:?} points outside the compacted \
                     leaf run set"
                )
            });
            if new_base != parent.base() {
                let remapped = Node::new(parent.mask(), 1, new_base);
                self.bands_mut().set_node(parent_ref, remapped);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CHUNK_DIM, CHUNK_VOLUME, local_index};

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// A blocky base with a solid lower half and sparse upper structures.
    fn blocky_base() -> Vec<u16> {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let material = if y < 20 {
                        1
                    } else if (x / 8 + z / 8) % 3 == 0 && y < 30 {
                        2
                    } else {
                        0
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
        dense
    }

    /// Applies `count` single-voxel edits, updating the dense reference too.
    fn edit_many(dag: &mut ChunkDag, dense: &mut [u16], state: &mut u64, count: usize) {
        for _ in 0..count {
            let index = (rng(state) % CHUNK_VOLUME as u64) as u32;
            let material = (rng(state) % 5) as u16;
            dense[index as usize] = material;
            dag.apply_edits([(index, material)]);
        }
    }

    /// Walks every live node and checks child addressing, leaf bounds and the
    /// full-run alignment invariant after a renumber.
    fn verify_live_dag(dag: &ChunkDag) {
        let Some(root) = dag.root() else {
            assert_eq!(dag.node_words(), 0);
            assert_eq!(dag.leaf_words(), 0);
            return;
        };

        let mut stack = vec![root];
        let mut visited = 0u64;
        while let Some(node_ref) = stack.pop() {
            let node = dag
                .bands()
                .get(node_ref)
                .unwrap_or_else(|| panic!("live reference {node_ref:?} is dead"));
            assert_eq!(node.level(), node_ref.level(), "{node_ref:?}");

            if node.level() == 1 {
                for c in 0..8u32 {
                    if node.mask() & (1 << c) == 0 {
                        continue;
                    }
                    let leaf = node
                        .child_index(c)
                        .expect("a present leaf child has a slot");
                    assert!(
                        dag.bands().leaf(leaf).is_some(),
                        "leaf slot {leaf} is past the arena tail"
                    );
                }
            } else {
                if node.mask() == 0xFF {
                    assert_eq!(
                        node.base() % 8,
                        0,
                        "full run at level {} is not cache-line aligned",
                        node.level()
                    );
                }
                for c in 0..8u32 {
                    if node.mask() & (1 << c) == 0 {
                        continue;
                    }
                    let index = node.child_index(c).expect("a present child has a slot");
                    let child_ref = NodeRef::new(node.level() - 1, index);
                    assert!(
                        dag.bands().get(child_ref).is_some(),
                        "child {child_ref:?} is missing"
                    );
                    stack.push(child_ref);
                }
            }
            visited += 1;
        }
        assert!(visited > 0, "a non-empty chunk has at least the root");
    }

    /// M1-12 acceptance: a 100k-coordinate sweep reads exactly the same
    /// materials before and after compaction, and the reclaimed bands still
    /// expand to the dense reference.
    #[test]
    fn compaction_get_sweep_matches_before_on_100k_coords() {
        let mut dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0x5eed_1e55_c0de_1234u64;
        edit_many(&mut dag, &mut dense, &mut state, 256);

        let words_before = dag.node_words();
        let leaves_before = dag.leaf_words();

        let mut probes = Vec::with_capacity(100_000);
        for _ in 0..100_000 {
            let x = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            let y = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            let z = (rng(&mut state) % CHUNK_DIM as u64) as u32;
            probes.push((x, y, z, dag.get(x, y, z)));
        }

        let stats = dag.compact();
        assert!(stats.reclaimed_bytes() > 0, "{stats:?}");
        assert_eq!(stats.bands, ROOT_LEVEL + 2);
        assert!(dag.node_words() < words_before, "{stats:?}");
        assert!(dag.leaf_words() < leaves_before, "{stats:?}");
        verify_live_dag(&dag);

        for &(x, y, z, expected) in &probes {
            assert_eq!(dag.get(x, y, z), expected, "voxel ({x}, {y}, {z})");
        }

        let mut rebuilt = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(
            rebuilt, dense,
            "compacted chunk differs from the dense reference"
        );
    }

    /// A single-band compaction renumbers only that band and remaps every live
    /// parent base into the new numbering; the live DAG still resolves.
    #[test]
    fn compaction_band_renumbers_and_remaps_parent_bases() {
        let mut dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0x0bad_cafe_d00d_1234u64;
        edit_many(&mut dag, &mut dense, &mut state, 256);

        let level = 1;
        let old_len = dag.bands().len(level);
        let old_words = dag.node_words();

        let stats = dag.compact_band(level);

        assert_eq!(stats.bands, 1);
        assert!(stats.runs > 0);
        assert_eq!(stats.bytes_before, old_len * NODE_BYTES);
        assert!(dag.bands().len(level) < old_len, "{stats:?}");
        assert!(dag.node_words() < old_words, "{stats:?}");
        assert!(stats.reclaimed_bytes() > 0, "{stats:?}");
        verify_live_dag(&dag);

        for test in 0..10_000 {
            let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
            let (x, y, z) = crate::local_xyz(index);
            assert_eq!(
                dag.get(x, y, z),
                dense[index as usize],
                "probe {test} voxel {index}"
            );
        }
    }

    /// Leaf compaction rebuilds the level-1 parents' leaf bases; reads still
    /// equal the dense reference after the arena shrinks.
    #[test]
    fn compaction_leaves_rebuilds_level_one_refs() {
        let mut dense = vec![0u16; CHUNK_VOLUME];
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0x1234_5678_9abc_def0u64;
        edit_many(&mut dag, &mut dense, &mut state, 512);

        let old_leaves = dag.leaf_words();
        let stats = dag.compact_leaves();

        assert_eq!(stats.bands, 1);
        assert!(stats.runs > 0);
        assert_eq!(stats.bytes_before, old_leaves * LEAF_MATERIAL_BYTES);
        assert!(dag.leaf_words() < old_leaves, "{stats:?}");
        verify_live_dag(&dag);

        let mut rebuilt = vec![u16::MAX; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(rebuilt, dense);
    }

    /// The trigger ignores a fresh uniform build and fires once edits leave
    /// retired runs behind.
    #[test]
    fn compaction_trigger_ignores_clean_build_and_fires_after_dead_runs() {
        let mut dag = ChunkDag::from_dense(&vec![3u16; CHUNK_VOLUME]);
        assert_eq!(dag.compaction_candidate(), None);
        assert!(dag.compact_if_needed().is_none());

        for i in 0..8u32 {
            dag.apply_edits([(local_index(i, 0, 0), 4)]);
        }
        let candidate = dag
            .compaction_candidate()
            .expect("edits leave retired runs behind");
        assert!(matches!(
            candidate,
            CompactBand::Leaves | CompactBand::Level(_)
        ));

        let stats = dag.compact_if_needed().expect("a candidate was reported");
        assert_eq!(stats.bands, 1);
        assert!(stats.reclaimed_bytes() > 0, "{stats:?}");
        verify_live_dag(&dag);
        for i in 0..8u32 {
            assert_eq!(dag.get(i, 0, 0), 4, "voxel {i}");
        }
        assert_eq!(dag.get(8, 0, 0), 3);
    }

    /// Interleaved edits and one-band compactions keep agreeing with the dense
    /// reference, whichever band is renumbered next.
    #[test]
    fn compaction_repeated_cycles_match_dense() {
        let mut dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0xdead_beef_1234_5678u64;

        for cycle in 0..100 {
            let count = 1 + (rng(&mut state) % 8) as usize;
            edit_many(&mut dag, &mut dense, &mut state, count);

            match cycle % 5 {
                0 => {
                    dag.compact();
                }
                1 => {
                    dag.compact_leaves();
                }
                2 => {
                    let level = 1 + (rng(&mut state) % ROOT_LEVEL as u64) as u32;
                    dag.compact_band(level);
                }
                3 => {
                    let _ = dag.compact_if_needed();
                }
                _ => {}
            }

            verify_live_dag(&dag);
            for probe in 0..64 {
                let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
                let (x, y, z) = crate::local_xyz(index);
                assert_eq!(
                    dag.get(x, y, z),
                    dense[index as usize],
                    "cycle {cycle} probe {probe} voxel {index}"
                );
            }
        }

        let mut rebuilt = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(rebuilt, dense);
    }

    /// An all-air chunk still owns retired bands after carving; full compaction
    /// clears every arena.
    #[test]
    fn compaction_empty_chunk_clears_every_arena() {
        let index = local_index(1, 2, 3);
        let mut dag = ChunkDag::empty();
        dag.apply_edits([(index, 5)]);
        dag.apply_edits([(index, 0)]);
        assert!(dag.is_empty());
        assert!(
            !dag.bands().is_empty(),
            "carving leaves the old bands behind"
        );

        let stats = dag.compact();
        assert!(stats.reclaimed_bytes() > 0, "{stats:?}");
        assert!(dag.bands().is_empty());
        assert_eq!(dag.root(), None);
        verify_live_dag(&dag);
    }

    /// M1-12 acceptance: a retained root blocks compaction instead of publishing
    /// a torn band.
    #[test]
    #[should_panic(expected = "live root")]
    fn compaction_panics_with_a_live_root() {
        let mut dag = ChunkDag::from_dense(&vec![1u16; CHUNK_VOLUME]);
        let root = dag.retain_root();
        assert_eq!(root.root(), dag.root());
        assert_eq!(dag.live_root_count(), 1);
        dag.compact();
    }

    /// An in-flight frame blocks compaction for the same reason.
    #[test]
    #[should_panic(expected = "in-flight frame")]
    fn compaction_panics_with_an_in_flight_frame() {
        let mut dag = ChunkDag::from_dense(&vec![1u16; CHUNK_VOLUME]);
        let frame = dag.retain_frame();
        assert_eq!(dag.in_flight_frame_count(), 1);
        drop(frame);
        let _frame = dag.retain_frame();
        dag.compact_band(ROOT_LEVEL);
    }

    /// Releasing the guards re-enables compaction.
    #[test]
    fn compaction_guard_releases_on_drop() {
        let mut dag = ChunkDag::from_dense(&vec![1u16; CHUNK_VOLUME]);
        {
            let _root = dag.retain_root();
            let _frame = dag.retain_frame();
            assert_eq!(dag.live_root_count(), 1);
            assert_eq!(dag.in_flight_frame_count(), 1);
        }
        assert_eq!(dag.live_root_count(), 0);
        assert_eq!(dag.in_flight_frame_count(), 0);
        dag.compact();
    }
}
