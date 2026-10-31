//! M2-23: budgeted remesh scheduler.
//!
//! A backlog of dirty blocks (M2-22) is drained in batches with a hard per-frame
//! block budget: no batch may exceed it, a block is handed out once until it is
//! dirtied again, and the backlog must drain in ascending order with no repeats.
//! Blocks are the budget unit because one block is one bounded mesh task.

use std::collections::HashSet;

use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM};
use voxel_mesh::dirty::DirtyBlocks;
use voxel_mesh::{BlockPos, RemeshScheduler};
use voxel_storage::dag::Svdag;
use voxel_storage::{AIR, CHUNK_VOLUME, local_index};

fn blocks_per_edge() -> usize {
    CHUNK_DIM / BLOCK_DIM
}

fn block(x: usize, y: usize, z: usize) -> BlockPos {
    BlockPos::new(x as u32, y as u32, z as u32)
}

fn linear(pos: BlockPos, edge: usize) -> usize {
    pos.x as usize + pos.y as usize * edge + pos.z as usize * edge * edge
}

fn all_blocks() -> DirtyBlocks {
    let edge = blocks_per_edge();
    let mut dirty = DirtyBlocks::new();
    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                dirty.mark_block(block(x, y, z));
            }
        }
    }
    dirty
}

#[test]
fn scheduler_drains_the_backlog_within_the_budget() {
    let edge = blocks_per_edge();
    let total = edge * edge * edge;
    let budget = 7;

    let mut scheduler = RemeshScheduler::new();
    assert_eq!(scheduler.enqueue(&all_blocks()), total);
    assert_eq!(scheduler.backlog(), total);
    assert!(!scheduler.is_empty());

    let mut seen = HashSet::new();
    let mut last = None;
    let mut frames = 0;
    let mut batch = Vec::new();

    while !scheduler.is_empty() {
        let count = scheduler.next_batch_into(budget, &mut batch);
        assert!(count > 0, "a non-empty backlog must make progress");
        assert!(count <= budget, "frame {frames} exceeded the budget");
        assert_eq!(batch.len(), count);

        for pos in &batch {
            assert!(seen.insert(*pos), "block {pos:?} was handed out twice");
            let index = linear(*pos, edge);
            if let Some(last) = last {
                assert!(index > last, "batches are not in ascending order");
            }
            last = Some(index);
        }
        frames += 1;
        assert!(frames <= total, "the scheduler did not converge");
    }

    assert_eq!(seen.len(), total);
    assert_eq!(frames, total.div_ceil(budget));
    assert_eq!(scheduler.backlog(), 0);

    // An empty scheduler clears the caller's buffer and returns zero.
    assert_eq!(scheduler.next_batch_into(budget, &mut batch), 0);
    assert!(batch.is_empty());
}

#[test]
fn scheduler_never_exceeds_the_frame_budget() {
    for budget in [1usize, 5, 64] {
        let mut scheduler = RemeshScheduler::new();
        scheduler.enqueue(&all_blocks());
        let total = scheduler.backlog();
        let mut drained = 0;

        while !scheduler.is_empty() {
            let batch = scheduler.next_batch(budget);
            assert!(!batch.is_empty(), "budget {budget}: no progress");
            assert!(
                batch.len() <= budget,
                "budget {budget}: batch of {}",
                batch.len()
            );
            drained += batch.len();
        }
        assert_eq!(drained, total, "budget {budget}: blocks lost");
    }

    // A zero budget is a no-op, not a stall with hidden progress.
    let mut scheduler = RemeshScheduler::new();
    scheduler.enqueue_block(block(0, 0, 0));
    assert!(scheduler.next_batch(0).is_empty());
    assert_eq!(scheduler.backlog(), 1);
}

#[test]
fn scheduler_coalesces_repeated_edits() {
    let mut dirty = DirtyBlocks::new();
    // One voxel inside block (0, 0, 0).
    dirty.mark_voxel(8, 8, 8);

    let mut scheduler = RemeshScheduler::new();
    assert_eq!(scheduler.enqueue(&dirty), 1);
    assert_eq!(scheduler.enqueue(&dirty), 0, "the same block is coalesced");
    assert!(!scheduler.enqueue_block(block(0, 0, 0)));
    assert_eq!(scheduler.backlog(), 1);

    assert_eq!(scheduler.next_batch(10), vec![block(0, 0, 0)]);
    assert!(scheduler.is_empty());
}

#[test]
fn scheduler_requeues_a_block_dirtied_after_it_was_meshed() {
    let mut scheduler = RemeshScheduler::new();
    let pos = block(1, 1, 1);

    assert!(scheduler.enqueue_block(pos));
    assert_eq!(scheduler.next_batch(1), vec![pos]);
    assert!(scheduler.is_empty());

    // A later edit dirties it again; it must come back.
    assert!(scheduler.enqueue_block(pos));
    assert_eq!(scheduler.backlog(), 1);
    assert_eq!(scheduler.next_batch(1), vec![pos]);
}

#[test]
fn scheduler_merges_batches_between_frames() {
    let mut first = DirtyBlocks::new();
    first.mark_block(block(0, 0, 0));
    let mut second = DirtyBlocks::new();
    second.mark_block(block(0, 0, 0));
    second.mark_block(block(1, 0, 0));

    let mut scheduler = RemeshScheduler::new();
    assert_eq!(scheduler.enqueue(&first), 1);
    assert_eq!(scheduler.enqueue(&second), 1, "only (1, 0, 0) is new");
    assert_eq!(scheduler.backlog(), 2);
    assert_eq!(
        scheduler.next_batch(10),
        vec![block(0, 0, 0), block(1, 0, 0)]
    );
}

#[test]
fn scheduler_orders_batches_deterministically() {
    let mut dirty = DirtyBlocks::new();
    for pos in [
        block(3, 0, 0),
        block(0, 0, 0),
        block(1, 2, 1),
        block(2, 1, 3),
    ] {
        dirty.mark_block(pos);
    }

    let mut first = RemeshScheduler::new();
    let mut second = RemeshScheduler::new();
    first.enqueue(&dirty);
    second.enqueue(&dirty);

    let mut a = Vec::new();
    let mut b = Vec::new();
    while !first.is_empty() {
        first.next_batch_into(2, &mut a);
        second.next_batch_into(2, &mut b);
        assert_eq!(a, b, "two schedulers must produce the same batches");
    }
}

#[test]
fn scheduler_drains_a_cow_diff() {
    let mut dense = vec![AIR; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for x in 0..CHUNK_DIM as u32 {
            for y in 0..BLOCK_DIM as u32 {
                dense[local_index(x, y, z) as usize] = 1;
            }
        }
    }
    let dag = Svdag::from_dense(&dense);

    let edits = [
        (local_index(8, 24, 8), 7),   // added, block (0, 1, 0)
        (local_index(8, 15, 8), AIR), // removed, blocks (0, 0, 0) and (0, 1, 0)
        (local_index(24, 15, 8), 9),  // changed, block (1, 0, 0)
    ];
    let dirty = DirtyBlocks::from_edits(&dag, edits);
    let expected: Vec<BlockPos> = dirty.iter().collect();

    let mut scheduler = RemeshScheduler::new();
    assert_eq!(scheduler.enqueue(&dirty), dirty.count());

    let mut batch = Vec::new();
    let count = scheduler.next_batch_into(usize::MAX, &mut batch);
    assert_eq!(count, expected.len());
    assert_eq!(batch, expected);
    assert!(scheduler.is_empty());
}

#[test]
fn scheduler_clears_the_backlog() {
    let mut scheduler = RemeshScheduler::new();
    scheduler.enqueue(&all_blocks());
    assert!(!scheduler.is_empty());

    scheduler.clear();
    assert!(scheduler.is_empty());
    assert_eq!(scheduler.backlog(), 0);
    assert!(scheduler.next_batch(8).is_empty());
}

#[test]
#[should_panic(expected = "same chunk edge")]
fn scheduler_enqueue_rejects_another_edge() {
    let mut scheduler = RemeshScheduler::for_edge(4);
    let dirty = DirtyBlocks::for_edge(8);
    scheduler.enqueue(&dirty);
}

#[test]
fn scheduler_drains_a_multi_word_backlog() {
    // 8 blocks per edge = 512 bits = 8 words, even in a 64³ build. This is the
    // cross-word branch the compiled-edge tests only reach with chunk128.
    let edge = 8;
    let mut dirty = DirtyBlocks::for_edge(edge);
    for z in 0..edge {
        for y in 0..edge {
            for x in 0..edge {
                dirty.mark_block(block(x, y, z));
            }
        }
    }

    let mut scheduler = RemeshScheduler::for_edge(edge);
    assert_eq!(scheduler.enqueue(&dirty), 512);

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut frames = 0;
    while !scheduler.is_empty() {
        let count = scheduler.next_batch_into(7, &mut out);
        assert!(count > 0 && count <= 7);
        for pos in &out {
            assert!(seen.insert(*pos), "block {pos:?} repeated across words");
            assert!((pos.x as usize) < edge && (pos.y as usize) < edge);
            assert!((pos.z as usize) < edge);
        }
        frames += 1;
    }

    assert_eq!(seen.len(), 512);
    assert_eq!(frames, 512usize.div_ceil(7));
}

fn drain(scheduler: &mut RemeshScheduler) -> Vec<Vec<BlockPos>> {
    let mut batches = Vec::new();
    while !scheduler.is_empty() {
        batches.push(scheduler.next_batch(2));
    }
    batches
}

#[test]
fn scheduler_batches_do_not_depend_on_enqueue_order() {
    let n = blocks_per_edge();
    let blocks = [
        block(0, 0, 0),
        block(1, 2, 3 % n),
        block(n - 1, 0, 0),
        block(2, 1, 1),
    ];

    let mut forward = RemeshScheduler::new();
    for pos in blocks {
        forward.enqueue_block(pos);
    }

    let mut reverse = RemeshScheduler::new();
    for pos in blocks.iter().rev() {
        reverse.enqueue_block(*pos);
    }

    // The same set arriving as two merged batches must also converge.
    let mut merged = RemeshScheduler::new();
    let mut first = DirtyBlocks::new();
    first.mark_block(blocks[0]);
    first.mark_block(blocks[1]);
    let mut second = DirtyBlocks::new();
    second.mark_block(blocks[2]);
    second.mark_block(blocks[3]);
    merged.enqueue(&first);
    merged.enqueue(&second);

    let expected = drain(&mut forward);
    assert_eq!(drain(&mut reverse), expected);
    assert_eq!(drain(&mut merged), expected);
}

#[test]
fn scheduler_reuses_the_output_buffer() {
    let mut scheduler = RemeshScheduler::new();
    scheduler.enqueue(&all_blocks());

    let mut out = Vec::new();
    scheduler.next_batch_into(4, &mut out);
    let capacity = out.capacity();
    assert_eq!(out.len(), 4);

    // The next call clears first; the buffer is reused rather than grown.
    scheduler.next_batch_into(4, &mut out);
    assert_eq!(out.len(), 4);
    assert_eq!(out.capacity(), capacity);

    let mut empty = RemeshScheduler::new();
    out.push(block(0, 0, 0));
    assert_eq!(empty.next_batch_into(4, &mut out), 0);
    assert!(out.is_empty());
}
