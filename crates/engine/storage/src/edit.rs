//! Copy-on-write edits on the banded chunk (M1-10).
//!
//! [`ChunkDag::apply_edits`] rebuilds only the runs that contain an edited
//! voxel. The bands are append-only, so every rebuilt run is appended at the
//! tail of its level band; the previous version's runs stay byte-for-byte
//! unchanged and its root keeps resolving to the old content. No dense buffer
//! is allocated and no untouched subtree is walked: an untouched child word is
//! copied from the old run into the new one.
//!
//! # What is written
//!
//! One edited voxel touches one node per level. For each touched node the edit
//! appends exactly one sibling run in the child band, from the level-1 parent up
//! to the root; the level-1 parent also appends one leaf run with its new
//! material payload. A full run is padded to the next 64 B cache line exactly
//! as [`RunAllocator`] does. Empty results are not stored: a mask-0 parent
//! writes no run, and carving the last voxel leaves the chunk with no root.
//!
//! [`EditStats`] reports the distinct edited paths, the runs and node words
//! appended, the alignment padding and the leaf payload bytes. `bytes` is the
//! total written (internal nodes plus padding plus leaf materials), so the
//! per-path cost is [`EditStats::bytes_per_path`].
//!
//! # Example
//!
//! ```
//! use voxel_storage::{ChunkDag, local_index};
//!
//! let mut dag = ChunkDag::empty();
//! let stats = dag.apply_edits([(local_index(1, 2, 3), 5)]);
//!
//! assert_eq!(stats.paths, 1);
//! assert_eq!(stats.leaf_runs, 1);
//! assert_eq!(dag.get(1, 2, 3), 5);
//! assert!(stats.bytes_per_path() > 0.0);
//! ```
//!
//! [`RunAllocator`]: crate::run::RunAllocator

use crate::chunk::ChunkDag;
use crate::leaf::{AIR, LEAF_MATERIAL_BYTES, LEAF_RUN_MATERIALS, fetch_leaf};
use crate::node_word::{Node, NodeRef};
use crate::run::{FULL_MASK, FULL_RUN_NODES, NODE_BYTES};
use crate::{CHUNK_VOLUME, morton};
use voxel_core::constants::ROOT_LEVEL;

/// Counts the runs and bytes a COW edit appended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditStats {
    /// Distinct voxel indices in the batch; repeated indices collapse.
    pub paths: u64,
    /// Internal sibling runs appended, one per touched node level.
    pub runs: u64,
    /// Internal node words appended across those runs.
    pub nodes: u64,
    /// Alignment padding appended before full runs.
    pub padding_bytes: u64,
    /// Leaf runs appended for touched level-1 parents.
    pub leaf_runs: u64,
    /// Leaf material bytes appended.
    pub leaf_bytes: u64,
    /// Total bytes appended: internal nodes, padding and leaf materials.
    pub bytes: u64,
}

impl EditStats {
    /// Bytes of internal nodes, including alignment padding.
    pub const fn node_bytes(&self) -> u64 {
        self.nodes * NODE_BYTES + self.padding_bytes
    }

    /// Total bytes appended per distinct edited path, or `0.0` for an empty batch.
    pub fn bytes_per_path(&self) -> f64 {
        if self.paths == 0 {
            0.0
        } else {
            self.bytes as f64 / self.paths as f64
        }
    }

    /// Internal node bytes per distinct edited path, or `0.0` for an empty batch.
    pub fn node_bytes_per_path(&self) -> f64 {
        if self.paths == 0 {
            0.0
        } else {
            self.node_bytes() as f64 / self.paths as f64
        }
    }
}

impl ChunkDag {
    /// Applies `(local voxel index, material)` edits in place, appending the
    /// rebuilt runs to the existing bands.
    ///
    /// Material `0` is air. Later edits to the same index win. An empty batch
    /// does nothing and returns zeroed [`EditStats`]. The old root and every
    /// run it reaches stay valid: this method never overwrites a published
    /// band slot, so a caller that retained an earlier root can still expand
    /// the earlier content.
    ///
    /// # Panics
    ///
    /// Panics if an index is not below [`CHUNK_VOLUME`].
    pub fn apply_edits(&mut self, edits: impl IntoIterator<Item = (u32, u16)>) -> EditStats {
        let mut edits: Vec<(u32, u16)> = edits
            .into_iter()
            .map(|(index, material)| {
                assert!((index as usize) < CHUNK_VOLUME, "voxel index out of range");
                (morton(index), material)
            })
            .collect();
        if edits.is_empty() {
            return EditStats::default();
        }

        // Stable order makes duplicate indices deterministic: the last edit in
        // the input wins at the leaf, and runs stay grouped by child octant.
        edits.sort_by_key(|&(key, _)| key);

        let mut stats = EditStats {
            paths: distinct_paths(&edits),
            ..EditStats::default()
        };

        let root_word = self.edit_node(self.root(), ROOT_LEVEL, &edits, &mut stats);
        let root = match root_word {
            Some(word) => {
                let root = self.bands_mut().append(ROOT_LEVEL, word);
                stats.runs += 1;
                stats.nodes += 1;
                stats.bytes += NODE_BYTES;
                Some(root)
            }
            None => None,
        };
        self.set_root(root);
        stats
    }

    /// Rebuilds `level`'s node and appends the runs below it.
    ///
    /// `edits` are `(morton, material)` sorted by morton and all inside this
    /// node's block. The returned word is the new node at `level`; its caller
    /// appends it to the run in band `level`. The root has no caller, so
    /// [`Self::apply_edits`] appends it directly.
    fn edit_node(
        &mut self,
        old: Option<NodeRef>,
        level: u32,
        edits: &[(u32, u16)],
        stats: &mut EditStats,
    ) -> Option<Node> {
        if edits.is_empty() {
            return old.and_then(|node_ref| self.bands().get(node_ref));
        }

        let old_word = old.and_then(|node_ref| self.bands().get(node_ref));
        let shift = 3 * (level - 1);

        if level == 1 {
            return self.edit_leaf(old_word, edits, shift, stats);
        }

        let mut children = [Node::new(0, level - 1, 0); 8];
        let mut mask = 0u8;
        let mut count = 0usize;
        let mut rest = edits;
        for c in 0..8u32 {
            let split = rest.partition_point(|&(key, _)| ((key >> shift) & 7) == c);
            let (sub, tail) = rest.split_at(split);
            rest = tail;

            let old_child = old_word.and_then(|node| {
                if node.mask() & (1 << c) == 0 {
                    return None;
                }
                let index = node.child_index(c)?;
                Some(NodeRef::new(level - 1, index))
            });
            let child = if sub.is_empty() {
                // Untouched subtree: reuse the old word without walking it.
                old_child.and_then(|node_ref| self.bands().get(node_ref))
            } else {
                self.edit_node(old_child, level - 1, sub, stats)
            };

            if let Some(word) = child {
                mask |= 1 << c;
                children[count] = word;
                count += 1;
            }
        }

        if mask == 0 {
            return None;
        }
        let base = self.append_run(level - 1, mask, &children[..count], stats);
        Some(Node::new(mask, level, base))
    }

    /// Rebuilds one level-1 parent's leaf run.
    ///
    /// The parent's old materials are copied for children the batch does not
    /// touch; air is dropped from the new run and clears its mask bit.
    fn edit_leaf(
        &mut self,
        old: Option<Node>,
        edits: &[(u32, u16)],
        shift: u32,
        stats: &mut EditStats,
    ) -> Option<Node> {
        let mut materials = [AIR; LEAF_RUN_MATERIALS];
        let mut mask = 0u8;
        let mut count = 0usize;
        let mut rest = edits;
        for c in 0..8u32 {
            let split = rest.partition_point(|&(key, _)| ((key >> shift) & 7) == c);
            let (sub, tail) = rest.split_at(split);
            rest = tail;

            let material = match sub.last() {
                Some(&(_, material)) => material,
                None => old
                    .map(|node| fetch_leaf(self.bands(), node, c))
                    .unwrap_or(AIR),
            };
            if material != AIR {
                mask |= 1 << c;
                materials[count] = material;
                count += 1;
            }
        }

        if mask == 0 {
            return None;
        }

        let base = self.bands().leaf_len();
        for &material in &materials[..count] {
            self.bands_mut().append_leaf(material);
        }
        let leaf_bytes = count as u64 * LEAF_MATERIAL_BYTES;
        stats.leaf_runs += 1;
        stats.leaf_bytes += leaf_bytes;
        stats.bytes += leaf_bytes;

        Some(Node::new(mask, 1, base))
    }

    /// Appends one sibling run to `level`'s band and records what it cost.
    ///
    /// Full runs are aligned to an eight-slot boundary, exactly like
    /// [`crate::run::RunAllocator::allocate_into`]; the padding words are
    /// never referenced. Returns the run's first slot.
    fn append_run(
        &mut self,
        level: u32,
        mask: u8,
        children: &[Node],
        stats: &mut EditStats,
    ) -> u64 {
        assert_eq!(
            children.len() as u64,
            u64::from(mask.count_ones()),
            "run needs popcount(mask) child nodes"
        );

        let mut padding_bytes = 0u64;
        if mask == FULL_MASK {
            let len = self.bands().len(level);
            let aligned = len.next_multiple_of(FULL_RUN_NODES);
            padding_bytes = (aligned - len) * NODE_BYTES;
            for _ in len..aligned {
                // Padding slots are level-correct dead words (mask 0).
                self.bands_mut().append(level, Node::new(0, level, 0));
            }
        }

        let base = self.bands().len(level);
        for &child in children {
            self.bands_mut().append(level, child);
        }

        let node_bytes = children.len() as u64 * NODE_BYTES;
        stats.runs += 1;
        stats.nodes += children.len() as u64;
        stats.padding_bytes += padding_bytes;
        stats.bytes += node_bytes + padding_bytes;
        base
    }
}

/// Number of distinct morton keys in a sorted edit batch.
fn distinct_paths(edits: &[(u32, u16)]) -> u64 {
    let mut paths = 0u64;
    let mut previous = None;
    for &(key, _) in edits {
        if previous != Some(key) {
            paths += 1;
            previous = Some(key);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::expand_from;
    use crate::leaf::LEAF_RUN_BYTES;
    use crate::run::MAX_RUN_PADDING_BYTES;
    use crate::{CHUNK_DIM, local_index, local_xyz};

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
                        AIR
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
        dense
    }

    fn band_lengths(dag: &ChunkDag) -> Vec<u64> {
        (0..=ROOT_LEVEL)
            .map(|level| dag.bands().len(level))
            .collect()
    }

    /// The eight voxel indices of the level-1 parent at the origin.
    fn origin_leaf_indices() -> Vec<u32> {
        let mut indices = Vec::with_capacity(8);
        for z in 0..2u32 {
            for y in 0..2u32 {
                for x in 0..2u32 {
                    indices.push(local_index(x, y, z));
                }
            }
        }
        indices
    }

    /// M1-10 acceptance: a single edit appends exactly one internal run per
    /// level, plus one leaf run, and the old bands and old root are untouched.
    #[test]
    fn cow_paths_single_edit_appends_one_run_per_level() {
        let dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);

        let old_bands = dag.bands().clone();
        let old_root = dag.root().expect("the blocky base is non-empty");
        let old_lengths = band_lengths(&dag);
        let index = local_index(5, 10, 5);
        let mut expected = dense.clone();
        expected[index as usize] = 7;

        let stats = dag.apply_edits([(index, 7)]);

        assert_eq!(stats.paths, 1);
        assert_eq!(stats.runs, ROOT_LEVEL as u64);
        assert_eq!(stats.leaf_runs, 1);
        assert!(stats.nodes >= stats.runs);
        assert!(stats.padding_bytes <= stats.runs * MAX_RUN_PADDING_BYTES);
        assert_eq!(
            stats.node_bytes(),
            stats.nodes * NODE_BYTES + stats.padding_bytes
        );
        assert_eq!(stats.bytes, stats.node_bytes() + stats.leaf_bytes);
        assert_eq!(stats.leaf_bytes, LEAF_RUN_BYTES);
        assert_eq!(stats.bytes_per_path(), stats.bytes as f64);

        let mut rebuilt = vec![u16::MAX; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(
            rebuilt, expected,
            "the edited chunk differs from the dense apply"
        );

        // Append-only: every old slot is still the same bytes, and the old
        // root still expands to the old content against the new bands.
        for level in 0..=ROOT_LEVEL {
            let old = old_bands.band(level);
            let new = dag.bands().band(level);
            assert_eq!(&new[..old.len()], old, "old level {level} bytes changed");
            assert!(
                new.len() as u64 >= old_lengths[level as usize],
                "level {level} shrank"
            );
        }
        assert_eq!(
            &dag.bands().leaves()[..old_bands.leaves().len()],
            old_bands.leaves(),
            "old leaf bytes changed"
        );

        let mut old_rebuilt = vec![u16::MAX; CHUNK_VOLUME];
        expand_from(dag.bands(), Some(old_root), &mut old_rebuilt);
        assert_eq!(
            old_rebuilt, dense,
            "the old root no longer reads the old chunk"
        );
    }

    /// Several edits inside one level-1 parent still write one leaf run and
    /// one run per touched level: sibling edits share the rebuilt path.
    #[test]
    fn cow_paths_a_batch_shares_one_run_per_touched_level() {
        let dense = vec![1u16; CHUNK_VOLUME];
        let mut dag = ChunkDag::from_dense(&dense);

        let edits: Vec<(u32, u16)> = origin_leaf_indices()
            .into_iter()
            .map(|index| (index, 2))
            .collect();
        let stats = dag.apply_edits(edits);

        assert_eq!(stats.paths, 8);
        assert_eq!(stats.leaf_runs, 1);
        assert_eq!(stats.runs, ROOT_LEVEL as u64);
        assert_eq!(stats.leaf_bytes, 8 * LEAF_MATERIAL_BYTES);

        for z in 0..2u32 {
            for y in 0..2u32 {
                for x in 0..2u32 {
                    assert_eq!(dag.get(x, y, z), 2, "voxel ({x}, {y}, {z})");
                }
            }
        }
        let mut rebuilt = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        for index in origin_leaf_indices() {
            assert_eq!(rebuilt[index as usize], 2);
        }
        assert_eq!(rebuilt[local_index(10, 10, 10) as usize], 1);
    }

    /// The M1-10 acceptance property: 10,000 random batches agree with a dense
    /// reference, including carving to air and reverting.
    #[test]
    fn cow_paths_10k_random_batches_match_a_dense_apply() {
        const BATCHES: usize = 10_000;

        let mut dense = blocky_base();
        let mut dag = ChunkDag::from_dense(&dense);
        let mut state = 0x0c0a_5eed_1234_5678u64;

        for batch in 0..BATCHES {
            let count = (rng(&mut state) % 9) as usize;
            let mut edits = Vec::with_capacity(count);
            for _ in 0..count {
                let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
                let material = (rng(&mut state) % 5) as u16;
                edits.push((index, material));
            }

            // The dense reference applies the same batch in input order.
            for &(index, material) in &edits {
                dense[index as usize] = material;
            }

            let stats = dag.apply_edits(edits.iter().copied());

            // Distinct paths and final values, with later duplicates winning.
            let mut final_values: Vec<(u32, u16)> = Vec::new();
            for &(index, material) in &edits {
                match final_values.iter_mut().find(|(seen, _)| *seen == index) {
                    Some(entry) => entry.1 = material,
                    None => final_values.push((index, material)),
                }
            }
            assert_eq!(stats.paths, final_values.len() as u64, "batch {batch}");
            assert_eq!(
                stats.bytes,
                stats.node_bytes() + stats.leaf_bytes,
                "batch {batch}"
            );
            assert!(
                stats.runs <= stats.paths * ROOT_LEVEL as u64,
                "batch {batch}"
            );
            assert!(stats.leaf_runs <= stats.paths, "batch {batch}");

            for &(index, material) in &final_values {
                let (x, y, z) = local_xyz(index);
                assert_eq!(dag.get(x, y, z), material, "batch {batch} voxel {index}");
            }

            // Untouched regions must still read correctly too.
            for _ in 0..64 {
                let probe = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
                let (x, y, z) = local_xyz(probe);
                assert_eq!(
                    dag.get(x, y, z),
                    dense[probe as usize],
                    "batch {batch} probe {probe}"
                );
            }

            if batch % 500 == 0 || batch + 1 == BATCHES {
                let mut rebuilt = vec![u16::MAX; CHUNK_VOLUME];
                dag.expand(&mut rebuilt);
                assert_eq!(rebuilt, dense, "batch {batch}: dense apply differs");
            }
        }
    }

    /// An empty batch is a true no-op: no bytes, no root change, no band growth.
    #[test]
    fn cow_paths_empty_batch_is_a_noop() {
        let mut dag = ChunkDag::from_dense(&blocky_base());
        let root = dag.root();
        let lengths = band_lengths(&dag);
        let leaves = dag.bands().leaf_len();

        let stats = dag.apply_edits(std::iter::empty());

        assert_eq!(stats, EditStats::default());
        assert_eq!(dag.root(), root);
        assert_eq!(band_lengths(&dag), lengths);
        assert_eq!(dag.bands().leaf_len(), leaves);
    }

    /// Editing an empty chunk builds the same content as a dense build, and a
    /// later edit to air collapses it back to no root.
    #[test]
    fn cow_paths_empty_chunk_builds_and_collapses() {
        let index = local_index(1, 2, 3);
        let mut dag = ChunkDag::empty();

        let stats = dag.apply_edits([(index, 5)]);
        assert_eq!(stats.paths, 1);
        assert_eq!(stats.runs, ROOT_LEVEL as u64);
        assert_eq!(stats.leaf_runs, 1);
        assert_eq!(dag.get(1, 2, 3), 5);

        let mut expected = vec![0u16; CHUNK_VOLUME];
        expected[index as usize] = 5;
        let mut rebuilt = vec![0u16; CHUNK_VOLUME];
        dag.expand(&mut rebuilt);
        assert_eq!(rebuilt, expected);

        let stats = dag.apply_edits([(index, 0)]);
        assert!(dag.is_empty());
        assert_eq!(dag.root(), None);
        assert_eq!(stats.runs, 0);
        assert_eq!(stats.leaf_runs, 0);
        assert_eq!(stats.bytes, 0);
        assert_eq!(stats.paths, 1);
        let mut empty = vec![u16::MAX; CHUNK_VOLUME];
        dag.expand(&mut empty);
        assert!(empty.iter().all(|&material| material == AIR));
    }

    /// Duplicate indices collapse to one path and the last value wins.
    #[test]
    fn cow_paths_duplicate_indices_keep_the_last_value() {
        let index = local_index(4, 5, 6);
        let mut dag = ChunkDag::empty();

        let stats = dag.apply_edits([(index, 1), (index, 2), (index, 3)]);

        assert_eq!(stats.paths, 1);
        assert_eq!(stats.runs, ROOT_LEVEL as u64);
        assert_eq!(dag.get(4, 5, 6), 3);
    }

    #[test]
    #[should_panic(expected = "voxel index out of range")]
    fn cow_paths_reject_an_out_of_range_index() {
        ChunkDag::empty().apply_edits([(CHUNK_VOLUME as u32, 1)]);
    }
}
