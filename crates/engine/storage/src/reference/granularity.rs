//! Culling-granularity instrumentation (M1-34).
//!
//! The pre-registered gate asks for the fraction of **traversed-but-invisible**
//! block volume at chunk boundaries (pre-registration §3; ADR-006). This module
//! answers that question by combining the two reference oracles:
//!
//! - [`cull::traverse`] emits the nodes the hierarchical frustum traversal keeps
//!   at a requested LOD cutoff (M1-32).
//! - [`occlusion::OcclusionFill`] supplies the reference-visible block bits for
//!   the camera (M1-33).
//!
//! A block is **reference-visible** when its per-block box intersects the
//! frustum and the occlusion fill marks it visible. That is the exact block-level
//! oracle, not the traversal's own answer, so the report is independent of the
//! traversal it measures.
//!
//! # What is counted
//!
//! `traversed` is the **block volume represented by the emitted nodes**, not the
//! number of nodes and not the number of plane tests. At LOD0 one node is one
//! block; at LOD1 a node covers 2³ blocks and at LOD2 4³ blocks (ADR-009). A
//! coarse node contributes its whole block volume even when only part of it
//! intersects the frustum or when it contains air. That over-inclusion, plus
//! blocks the occlusion fill rejects, is precisely the granularity cost.
//!
//! `visible` counts reference-visible blocks that lie inside the traversed
//! volume. `missed` counts reference-visible blocks that no emitted node covers;
//! a conservative culler must report zero, and a nonzero value is a correctness
//! failure rather than a granularity result.
//!
//! # Ratio
//!
//! ```text
//! invisible = traversed - visible
//! ratio     = invisible / traversed
//! ```
//!
//! The ratio is in `[0, 1]`; an empty traversal reports `0.0`. The gate's
//! pre-registered limit is **15%** ([`GRANULARITY_GATE_LIMIT`]).
//!
//! The report is split per chunk so a gate run can show which side of a chunk
//! boundary contributes the waste, and it is deterministic: chunk order and
//! hash-map iteration never change the counts.

use std::collections::{HashMap, HashSet};

use voxel_core::constants::{BLOCK_BITS, BLOCK_DIM, CHUNK_DIM};

use crate::ChunkPos;
use crate::reference::cull::{ChunkView, Frustum, VisibleNode, brute_force_blocks, traverse};
use crate::reference::occlusion::OcclusionFill;

/// Pre-registered maximum traversed-but-invisible fraction (pre-registration §3).
///
/// This is a reporting constant, not a pass/fail decision: the gate report
/// (M1-39) owns the verdict and its margin.
pub const GRANULARITY_GATE_LIMIT: f64 = 0.15;

/// Traversed and reference-visible block counts for one chunk.
///
/// A chunk with no emitted node still appears in the report with zero counts, so
/// a per-chunk table has a row for every input chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkGranularity {
    chunk: ChunkPos,
    traversed_blocks: u64,
    visible_blocks: u64,
    missed_blocks: u64,
}

impl ChunkGranularity {
    /// The chunk these counts belong to.
    pub const fn chunk(self) -> ChunkPos {
        self.chunk
    }

    /// Block volume covered by the traversal's emitted nodes.
    pub const fn traversed_blocks(self) -> u64 {
        self.traversed_blocks
    }

    /// Reference-visible blocks inside the traversed volume.
    pub const fn visible_blocks(self) -> u64 {
        self.visible_blocks
    }

    /// Traversed blocks the reference says are not visible.
    pub const fn invisible_blocks(self) -> u64 {
        self.traversed_blocks - self.visible_blocks
    }

    /// Reference-visible blocks the traversal failed to emit.
    ///
    /// Must be zero: the culler may over-include but never under-include.
    pub const fn missed_blocks(self) -> u64 {
        self.missed_blocks
    }

    /// Traversed-but-invisible fraction for this chunk, `0.0` when empty.
    pub fn ratio(self) -> f64 {
        ratio(self.invisible_blocks(), self.traversed_blocks)
    }
}

/// One granularity measurement across all supplied chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GranularityReport {
    traversed_blocks: u64,
    visible_blocks: u64,
    missed_blocks: u64,
    per_chunk: Vec<ChunkGranularity>,
}

impl GranularityReport {
    /// Block volume covered by the traversal's emitted nodes.
    pub const fn traversed_blocks(&self) -> u64 {
        self.traversed_blocks
    }

    /// Reference-visible blocks inside the traversed volume.
    pub const fn visible_blocks(&self) -> u64 {
        self.visible_blocks
    }

    /// Traversed blocks the reference says are not visible.
    pub const fn invisible_blocks(&self) -> u64 {
        self.traversed_blocks - self.visible_blocks
    }

    /// Reference-visible blocks the traversal failed to emit.
    ///
    /// Must be zero. [`Self::is_conservative`] is the boolean form.
    pub const fn missed_blocks(&self) -> u64 {
        self.missed_blocks
    }

    /// Traversed-but-invisible fraction across the whole measurement.
    ///
    /// This is the per-pose number the gate compares with
    /// [`GRANULARITY_GATE_LIMIT`]. An empty traversal reports `0.0`.
    pub fn ratio(&self) -> f64 {
        ratio(self.invisible_blocks(), self.traversed_blocks)
    }

    /// True when the traversal emitted every reference-visible block.
    pub const fn is_conservative(&self) -> bool {
        self.missed_blocks == 0
    }

    /// Per-chunk counts, sorted by chunk position.
    pub fn per_chunk(&self) -> &[ChunkGranularity] {
        &self.per_chunk
    }

    /// Per-chunk counts for one position, if it was in the input set.
    pub fn chunk(&self, chunk: ChunkPos) -> Option<ChunkGranularity> {
        self.per_chunk
            .iter()
            .copied()
            .find(|entry| entry.chunk == chunk)
    }
}

/// Measures traversed block volume against reference-visible volume.
///
/// `frustum` and `chunks` are the same inputs as [`cull::traverse`];
/// `lod_max_depth` is the core depth cutoff (`BLOCK_LEVEL` is LOD0).
/// `occlusion` must be built for the same camera pose and world content.
///
/// The reference-visible set is recomputed independently through
/// [`brute_force_blocks`] and the occlusion bits, so this function measures the
/// traversal rather than trusting it. Running it for every pose emits the
/// per-pose ratio the gate records.
///
/// # Panics
///
/// Panics if `lod_max_depth > BLOCK_LEVEL`, matching [`cull::traverse`].
pub fn measure(
    frustum: &Frustum,
    chunks: &[ChunkView<'_>],
    occlusion: &OcclusionFill,
    lod_max_depth: u32,
) -> GranularityReport {
    let traversed_nodes = traverse(frustum, chunks, lod_max_depth);

    let mut traversed = HashSet::new();
    for node in traversed_nodes {
        insert_node_blocks(node, &mut traversed);
    }

    let reference_visible: HashSet<VisibleNode> = brute_force_blocks(frustum, chunks)
        .into_iter()
        .filter(|block| occlusion.is_visible(global_block(*block)))
        .collect();

    let mut per_chunk: HashMap<(i32, i32, i32), ChunkGranularity> = chunks
        .iter()
        .map(|view| {
            let pos = view.pos;
            let counts = ChunkGranularity {
                chunk: pos,
                traversed_blocks: 0,
                visible_blocks: 0,
                missed_blocks: 0,
            };
            ((pos.x, pos.y, pos.z), counts)
        })
        .collect();

    let mut visible_blocks = 0u64;
    for block in &traversed {
        let counts = per_chunk
            .get_mut(&chunk_key(block.chunk()))
            .expect("a traversed block belongs to an input chunk");
        counts.traversed_blocks += 1;
        if reference_visible.contains(block) {
            counts.visible_blocks += 1;
            visible_blocks += 1;
        }
    }

    let mut missed_blocks = 0u64;
    for block in reference_visible.difference(&traversed) {
        let counts = per_chunk
            .get_mut(&chunk_key(block.chunk()))
            .expect("a reference block belongs to an input chunk");
        counts.missed_blocks += 1;
        missed_blocks += 1;
    }

    let traversed_blocks = traversed.len() as u64;
    let mut per_chunk: Vec<ChunkGranularity> = per_chunk.into_values().collect();
    per_chunk.sort_unstable_by_key(|entry| (entry.chunk.x, entry.chunk.y, entry.chunk.z));

    GranularityReport {
        traversed_blocks,
        visible_blocks,
        missed_blocks,
        per_chunk,
    }
}

/// Adds every block in `node`'s volume to `blocks`.
///
/// A node at the block level adds itself; a coarse LOD node adds all
/// `2^(level - BLOCK_BITS)` blocks per axis. The node origin is aligned to its
/// own extent, so every block origin is a valid [`VisibleNode`].
fn insert_node_blocks(node: VisibleNode, blocks: &mut HashSet<VisibleNode>) {
    debug_assert!(
        node.level() >= BLOCK_BITS,
        "the traversal cutoff cannot be below the block level"
    );
    let blocks_per_axis = 1usize << (node.level() - BLOCK_BITS);
    let block_dim = BLOCK_DIM as u32;
    let origin = node.origin();

    for bz in 0..blocks_per_axis {
        for by in 0..blocks_per_axis {
            for bx in 0..blocks_per_axis {
                let block_origin = [
                    origin[0] + bx as u32 * block_dim,
                    origin[1] + by as u32 * block_dim,
                    origin[2] + bz as u32 * block_dim,
                ];
                blocks.insert(VisibleNode::new(node.chunk(), BLOCK_BITS, block_origin));
            }
        }
    }
}

/// Converts a chunk-local block node to world block coordinates.
///
/// The flood fill speaks world block coordinates; [`VisibleNode`] at
/// [`BLOCK_BITS`] carries a chunk position and a chunk-local voxel origin.
fn global_block(block: VisibleNode) -> [i32; 3] {
    debug_assert_eq!(block.level(), BLOCK_BITS);
    let blocks_per_edge = (CHUNK_DIM / BLOCK_DIM) as i64;
    let origin = block.origin();
    let chunk = block.chunk();
    [
        global_axis(chunk.x, origin[0], blocks_per_edge),
        global_axis(chunk.y, origin[1], blocks_per_edge),
        global_axis(chunk.z, origin[2], blocks_per_edge),
    ]
}

fn global_axis(chunk: i32, local_voxel: u32, blocks_per_edge: i64) -> i32 {
    let local_block = i64::from(local_voxel / BLOCK_DIM as u32);
    let global = i64::from(chunk) * blocks_per_edge + local_block;
    i32::try_from(global).expect("a world block coordinate leaves the i32 range")
}

const fn chunk_key(chunk: ChunkPos) -> (i32, i32, i32) {
    (chunk.x, chunk.y, chunk.z)
}

fn ratio(invisible: u64, traversed: u64) -> f64 {
    if traversed == 0 {
        0.0
    } else {
        invisible as f64 / traversed as f64
    }
}

// The report compares block volumes, so the conversion from a node's voxel
// volume to blocks must be exact for every LOD the traversal can emit.
const _: () = {
    assert!(CHUNK_DIM.is_multiple_of(BLOCK_DIM));
    assert!(BLOCK_DIM.is_power_of_two());
};
