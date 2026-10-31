//! Budgeted remesh scheduler: dirty blocks → mesh batches (M2-23).
//!
//! Edits arrive on the content timeline (M2-22's [`DirtyBlocks`]) and a scrub
//! must not turn a burst of them into one long frame. The scheduler owns a
//! backlog of dirty blocks and hands the caller at most a fixed number of blocks
//! per frame; each block is one bounded mesh task (one 18³ scratch, at most
//! `6 · 16²` faces), so a block budget is a work bound that does not depend on
//! wall-clock noise.
//!
//! # Ordering
//!
//! Batches are taken in ascending block order (`x` fastest, then `y`, then `z`),
//! and a block leaves the backlog when it is handed out. If a block is dirtied
//! again after it was meshed, it is simply queued again: the backlog is a set,
//! so a burst of edits to the same block costs one mesh, not one per edit.
//!
//! # Budget
//!
//! [`RemeshScheduler::next_batch_into`] never returns more than `budget_blocks`
//! blocks, and `0` makes no progress (the caller decides how many blocks a frame
//! can afford). A batch is **committed when it is handed out**: the blocks leave
//! the backlog at that moment. A caller that stops on measured time must
//! therefore stop between batches — size the budget for the time it can afford,
//! or re-enqueue the unmeshed tail with [`RemeshScheduler::enqueue_block`].
//!
//! ```ignore
//! let mut scheduler = RemeshScheduler::new();
//! scheduler.enqueue(&dirty);
//! let mut batch = Vec::new();
//! let blocks = scheduler.next_batch_into(8, &mut batch);
//! for block in &batch[..blocks] {
//!     mesh_block(*block);
//! }
//! ```

use crate::BlockPos;
use crate::dirty::DirtyBlocks;

/// A backlog of dirty blocks drained in bounded per-frame batches.
#[derive(Clone, Debug)]
pub struct RemeshScheduler {
    dirty: DirtyBlocks,
}

impl RemeshScheduler {
    /// An empty scheduler at the compiled chunk edge.
    #[must_use]
    pub fn new() -> Self {
        Self::for_edge(crate::dirty::BLOCKS_PER_CHUNK_EDGE)
    }

    /// An empty scheduler for a chunk with `blocks_per_edge` blocks per edge.
    ///
    /// [`Self::enqueue`] requires the same edge as the incoming bitset;
    /// [`Self::next_batch_into`] works at any edge.
    ///
    /// # Panics
    ///
    /// Panics if `blocks_per_edge` is zero.
    #[must_use]
    pub fn for_edge(blocks_per_edge: usize) -> Self {
        Self {
            dirty: DirtyBlocks::for_edge(blocks_per_edge),
        }
    }

    /// Number of blocks waiting for a mesh.
    pub fn backlog(&self) -> usize {
        self.dirty.count()
    }

    /// True when no block is waiting.
    pub fn is_empty(&self) -> bool {
        self.dirty.is_empty()
    }

    /// Adds every dirty block of `dirty` to the backlog.
    ///
    /// Returns how many blocks were newly queued; blocks already waiting are
    /// coalesced, so repeated edit batches to the same block cost one mesh.
    ///
    /// # Panics
    ///
    /// Panics if `dirty` covers a different chunk edge.
    pub fn enqueue(&mut self, dirty: &DirtyBlocks) -> usize {
        let before = self.backlog();
        self.dirty.union(dirty);
        self.backlog() - before
    }

    /// Adds one block to the backlog, returning true when it was not already
    /// waiting.
    ///
    /// # Panics
    ///
    /// Panics if the block is outside the chunk.
    pub fn enqueue_block(&mut self, block: BlockPos) -> bool {
        let queued = !self.dirty.is_dirty(block);
        self.dirty.mark_block(block);
        queued
    }

    /// Drops the whole backlog, e.g. when the chunk is evicted.
    pub fn clear(&mut self) {
        self.dirty.clear();
    }

    /// Takes at most `budget_blocks` blocks, clearing them from the backlog.
    ///
    /// `out` is cleared first and reused, so a steady-state caller allocates
    /// nothing per frame. Blocks are returned in ascending order and each block
    /// is handed out exactly once until it is dirtied again. A budget of zero
    /// returns an empty batch and leaves the backlog untouched.
    pub fn next_batch_into(&mut self, budget_blocks: usize, out: &mut Vec<BlockPos>) -> usize {
        self.dirty.take_first(budget_blocks, out)
    }

    /// Convenience wrapper over [`Self::next_batch_into`] that allocates.
    pub fn next_batch(&mut self, budget_blocks: usize) -> Vec<BlockPos> {
        let mut batch = Vec::with_capacity(budget_blocks.min(self.backlog()));
        self.next_batch_into(budget_blocks, &mut batch);
        batch
    }
}

impl Default for RemeshScheduler {
    fn default() -> Self {
        Self::new()
    }
}
