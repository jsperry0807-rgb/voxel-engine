//! Layout cost report (M1-18).
//!
//! Measures the banded layout against the superseded v1 `u32` pool on the same
//! dense fixtures:
//!
//! - internal bytes, including the padding that aligns full runs to 64 B;
//! - bytes per reachable internal node and the padding ratio;
//! - internal and leaf run counts, and how many runs are shared;
//! - dedup ratio for whole-run sharing versus the old per-node encoding;
//! - COW bytes per edited path for one edit and a 100-edit batch.
//!
//! The old encoding's unique nodes and expanded positions come from its public
//! pool (`header = mask | level << 8`, then the present child words). A v1
//! level-1 child is a material ID, not a node, so the walk stops there just
//! like the new layout stops at the leaf arena.

use std::collections::{HashMap, HashSet};

use voxel_storage::dag::{NONE, Svdag, hdr_level, hdr_mask};
use voxel_storage::{CHUNK_VOLUME, ChunkDag};

use crate::patterns;
use crate::report::{CowRecord, LayoutRecord};

type FixtureBuilder = fn() -> Vec<u16>;

/// Fixtures measured by the report. Names match the memory chunks so a reader
/// can compare the layout table with `chunk/<fixture>`.
const FIXTURES: &[(&str, FixtureBuilder)] = &[
    ("solid", || patterns::solid(1)),
    ("terrain", || patterns::terrain(0, 0)),
    ("noise30%", || patterns::noise(0x1234, 300)),
    ("sparse4k", || patterns::sparse(0x5678, 4_096)),
];

/// Builds the per-fixture layout cost records.
pub fn records() -> Vec<LayoutRecord> {
    FIXTURES
        .iter()
        .map(|(name, build)| measure(name, &build()))
        .collect()
}

fn measure(fixture: &str, dense: &[u16]) -> LayoutRecord {
    let old = Svdag::from_dense(dense);
    let chunk = old.chunk();
    let check = chunk
        .check_report()
        .expect("a freshly built chunk passes the invariant checker");

    let physical_nodes = chunk.node_words();
    let padding_slots = physical_nodes
        .checked_sub(check.nodes)
        .expect("a fresh build stores live nodes plus padding only");
    let padding_bytes = padding_slots * 8;
    let internal_bytes = physical_nodes * 8;
    let (old_nodes, expanded_nodes) = legacy_stats(&old);
    let old_bytes = old.to_bytes().len() as u64;
    let new_bytes = chunk.to_bytes().len() as u64;

    LayoutRecord {
        fixture: fixture.to_owned(),
        internal_bytes,
        nodes: check.nodes,
        padding_bytes,
        bytes_per_node: ratio(internal_bytes as f64, check.nodes as f64),
        padding_ratio: ratio(padding_bytes as f64, internal_bytes as f64),
        internal_runs: check.internal_runs,
        leaf_runs: check.leaf_runs,
        shared_runs: check.shared_runs,
        expanded_nodes,
        dedup_new: ratio(expanded_nodes as f64, check.nodes as f64),
        dedup_old: ratio(expanded_nodes as f64, old_nodes as f64),
        old_nodes,
        old_bytes,
        new_bytes,
        size_ratio: ratio(old_bytes as f64, new_bytes as f64),
        cow: [1usize, 100]
            .into_iter()
            .map(|paths| cow_batch(chunk, paths))
            .collect(),
    }
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

/// Measures one COW batch on a clone of the fresh chunk, so every batch starts
/// from the same band numbering and no retired runs are included.
fn cow_batch(base: &ChunkDag, paths: usize) -> CowRecord {
    let mut edited = base.clone();
    let indices = unique_indices(paths, 0xC0FF_EE00 ^ paths as u64);
    let stats = edited.apply_edits(
        indices
            .iter()
            .enumerate()
            .map(|(i, &index)| (index, 4 + (i % 3) as u16)),
    );

    CowRecord {
        paths: stats.paths,
        runs: stats.runs,
        leaf_runs: stats.leaf_runs,
        node_bytes: stats.node_bytes(),
        leaf_bytes: stats.leaf_bytes,
        total_bytes: stats.bytes,
        node_bytes_per_path: stats.node_bytes_per_path(),
        bytes_per_path: stats.bytes_per_path(),
    }
}

fn unique_indices(count: usize, seed: u64) -> Vec<u32> {
    let mut state = seed | 1;
    let mut seen = HashSet::with_capacity(count);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let index = (rng(&mut state) % CHUNK_VOLUME as u64) as u32;
        if seen.insert(index) {
            out.push(index);
        }
    }
    out
}

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Returns `(unique internal nodes, expanded internal positions)` for the v1
/// pool. Expanded positions count one node per sibling slot in the unshared
/// tree, so it is the same logical tree the banded layout stores.
fn legacy_stats(dag: &Svdag) -> (u64, u64) {
    if dag.root == NONE {
        return (0, 0);
    }
    let mut seen = HashSet::new();
    let mut positions = HashMap::new();
    let expanded = legacy_positions(&dag.pool, dag.root, &mut seen, &mut positions);
    (seen.len() as u64, expanded)
}

fn legacy_positions(
    pool: &[u32],
    node: u32,
    seen: &mut HashSet<u32>,
    positions: &mut HashMap<u32, u64>,
) -> u64 {
    if let Some(&total) = positions.get(&node) {
        return total;
    }
    seen.insert(node);

    let header = pool[node as usize];
    let mask = hdr_mask(header);
    let level = hdr_level(header);
    let mut total = 1u64;
    if level > 1 {
        let mut slot = 1usize;
        for child in 0..8u32 {
            if mask & (1 << child) == 0 {
                continue;
            }
            total += legacy_positions(pool, pool[node as usize + slot], seen, positions);
            slot += 1;
        }
    }

    positions.insert(node, total);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_report_counts_padding_and_dedup_consistently() {
        let records = records();
        assert_eq!(records.len(), FIXTURES.len());
        assert_eq!(records[0].fixture, "solid");
        assert_eq!(records[1].fixture, "terrain");

        for record in &records {
            assert!(record.nodes > 0, "{} has a root", record.fixture);
            assert_eq!(
                record.internal_bytes,
                record.nodes * 8 + record.padding_bytes,
                "{}: every internal byte is a node or padding",
                record.fixture
            );
            assert!(
                record.padding_bytes <= record.internal_runs * 7 * 8,
                "{}: padding is bounded by seven slots per run",
                record.fixture
            );
            assert!(record.dedup_new >= 1.0, "{}", record.fixture);
            assert!(record.dedup_old >= 1.0, "{}", record.fixture);
            assert!(record.expanded_nodes >= record.nodes, "{}", record.fixture);
            assert!(record.old_nodes >= 1, "{}", record.fixture);
            assert!(record.old_bytes > 0, "{}", record.fixture);
            assert!(record.new_bytes > 0, "{}", record.fixture);

            assert_eq!(record.cow.len(), 2, "{}", record.fixture);
            assert_eq!(record.cow[0].paths, 1);
            assert_eq!(record.cow[1].paths, 100);
            for cow in &record.cow {
                assert!(
                    cow.runs >= voxel_storage::dag::ROOT_LEVEL as u64,
                    "{}",
                    record.fixture
                );
                assert!(cow.leaf_runs >= 1, "{}", record.fixture);
                assert_eq!(
                    cow.total_bytes,
                    cow.node_bytes + cow.leaf_bytes,
                    "{}",
                    record.fixture
                );
                assert!(cow.node_bytes_per_path > 0.0, "{}", record.fixture);
                assert!(cow.bytes_per_path >= cow.node_bytes_per_path);
            }
        }
    }

    #[test]
    fn expanded_positions_match_a_fully_unshared_octree() {
        // A uniform chunk has no sharing below the root: 1 + 8 + ... + 8^5.
        let dense = patterns::solid(1);
        let old = Svdag::from_dense(&dense);
        let (_, expanded) = legacy_stats(&old);
        let expected: u64 = (0..voxel_storage::dag::ROOT_LEVEL)
            .map(|level| 8u64.pow(level))
            .sum();
        assert_eq!(expanded, expected);
    }
}
