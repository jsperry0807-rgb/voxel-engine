//! M2-29: worker scratch budget and the no-densify guarantee.
//!
//! The production driver walks a chunk one 16³ block at a time and reuses a
//! single 18³ [`BlockScratch`]; the greedy core keeps its merge mask on the
//! stack. This test installs a counting global allocator and arms it only on
//! the calling thread, so every heap allocation made by the walk is attributed
//! to the worker:
//!
//! - filling every block allocates nothing — the scratch lives on the stack;
//! - the greedy core allocates nothing when the caller reuses its quad buffer;
//! - the full build allocates exactly the two mesh-output buffers of each
//!   non-empty block and nothing else, so it can never densify the chunk.
//!
//! The budgeted working set is the scratch: `18³ × 2 B = 11,664 B` plus the
//! stack mask, under [`SCRATCH_BUDGET_BYTES`]. The quad list and the emitted
//! vertices/indices are per-block outputs (bounded by `3 × 16³` quads), not
//! scratch; the full-build test pins that they are the *only* heap traffic.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::mem::size_of;

use voxel_core::constants::{CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::{
    BLOCK_EDGE, BlockPos, BlockScratch, SCRATCH_BUDGET_BYTES, SCRATCH_BYTES, SCRATCH_EDGE,
    SCRATCH_VOXELS, for_each_block, greedy_block, mesh_block,
};
use voxel_storage::Apron;

/// Worst-case quads in one 16³ block: a checkerboard exposes three faces per
/// voxel, and every face can be its own rectangle.
const MAX_BLOCK_QUADS: usize = BLOCK_EDGE * BLOCK_EDGE * BLOCK_EDGE * 3;

// Compile-time budget: the scratch struct is exactly the advertised bytes, and
// a dense chunk copy is far larger than the per-worker budget at both edges.
const _: () = assert!(size_of::<BlockScratch>() == SCRATCH_BYTES);
const _: () = assert!(SCRATCH_VOXELS == SCRATCH_EDGE * SCRATCH_EDGE * SCRATCH_EDGE);
const _: () = assert!(SCRATCH_BYTES <= SCRATCH_BUDGET_BYTES);
const _: () = assert!(CHUNK_VOLUME * size_of::<u16>() > SCRATCH_BUDGET_BYTES);

/// Per-thread allocation counters. `armed` is false outside a measurement, so
/// unrelated harness allocations are ignored.
#[derive(Clone, Copy, Default)]
struct Counters {
    armed: bool,
    allocs: u64,
    allocated: u64,
    live: u64,
    peak: u64,
    max_single: u64,
}

thread_local! {
    static COUNTERS: Cell<Counters> = const {
        Cell::new(Counters {
            armed: false,
            allocs: 0,
            allocated: 0,
            live: 0,
            peak: 0,
            max_single: 0,
        })
    };
}

/// What one armed window allocated on the measuring thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stats {
    allocs: u64,
    allocated: u64,
    peak: u64,
    max_single: u64,
}

fn record_alloc(size: usize) {
    let _ = COUNTERS.try_with(|cell| {
        let mut counters = cell.get();
        if !counters.armed {
            return;
        }
        let size = size as u64;
        counters.allocs += 1;
        counters.allocated += size;
        counters.live += size;
        counters.peak = counters.peak.max(counters.live);
        counters.max_single = counters.max_single.max(size);
        cell.set(counters);
    });
}

fn record_free(size: usize) {
    let _ = COUNTERS.try_with(|cell| {
        let mut counters = cell.get();
        if !counters.armed {
            return;
        }
        counters.live = counters.live.saturating_sub(size as u64);
        cell.set(counters);
    });
}

/// A forwarding allocator that records the armed window's heap traffic.
///
/// It adds one thread-local load/store per allocation, which is fine for a
/// budget test and far too coarse for a latency benchmark. The thread-local is
/// `const`-initialised and holds a `Copy` value with no `Drop`, so the access
/// inside `alloc` is a plain TLS read: it cannot allocate, register a
/// destructor or recurse. `alloc_zeroed` is inherited, so it routes through
/// [`Self::alloc`] and is counted too.
struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

// SAFETY: every method forwards to `System` with the same layout, and the
// counters never allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarding to the system allocator with the same layout.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System.alloc`/`System.realloc` with `layout`.
        unsafe { System.dealloc(ptr, layout) };
        record_free(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarding to the system allocator with the same arguments.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            record_free(layout.size());
            record_alloc(new_size);
        }
        new_ptr
    }
}

/// Runs `work` with the counters armed on this thread and reports its traffic.
///
/// Accounting is per thread, which matches a per-worker budget. The driver is
/// synchronous today; if it ever moves onto a worker pool, this test must also
/// watch the pool threads or the workers would escape unnoticed.
fn measure<T>(work: impl FnOnce() -> T) -> (T, Stats) {
    COUNTERS.with(|cell| {
        cell.set(Counters {
            armed: true,
            ..Counters::default()
        })
    });
    let out = work();
    let counters = COUNTERS.with(|cell| {
        let counters = cell.get();
        cell.set(Counters::default());
        counters
    });
    let stats = Stats {
        allocs: counters.allocs,
        allocated: counters.allocated,
        peak: counters.peak,
        max_single: counters.max_single,
    };
    (out, stats)
}

/// A deterministic per-voxel material mix with many exposed faces and material
/// seams. It is not the theoretical worst case (that is a checkerboard), but it
/// exercises the full walk with a dense, varied volume.
fn noise_chunk() -> Vec<u16> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..CHUNK_VOLUME)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 7) as u16
        })
        .collect()
}

#[test]
fn scratch_budget_is_under_64_kib() {
    // The budget itself is a compile-time assertion at the top of the file, so
    // this reported test can never drift from the constant it documents.
    println!(
        "M2-29 budget: BlockScratch {} B / {} B (dense chunk {} B)",
        SCRATCH_BYTES,
        SCRATCH_BUDGET_BYTES,
        CHUNK_VOLUME * size_of::<u16>()
    );
    assert_eq!(size_of::<BlockScratch>(), SCRATCH_BYTES);
}

#[test]
fn scratch_budget_block_walk_stays_under_budget() {
    let chunk = noise_chunk();
    let apron = Apron::new();
    let (checksum, stats) = measure(|| {
        let mut checksum = 0u64;
        for_each_block(&chunk, &apron, |_, scratch| {
            checksum = checksum.wrapping_add(u64::from(
                scratch.get(0, 0, 0)
                    ^ scratch.get(
                        SCRATCH_EDGE as u32 - 1,
                        SCRATCH_EDGE as u32 - 1,
                        SCRATCH_EDGE as u32 - 1,
                    ),
            ));
        });
        checksum
    });
    black_box(checksum);
    assert_budget(&stats, "the block walk");
    // Stronger invariant: the driver reuses one stack scratch, so there is
    // nothing to account for at all. A future heap scratch under the budget
    // would still pass `assert_budget`; this catches per-block churn.
    assert_eq!(
        stats.allocs, 0,
        "the block walk allocated {} time(s); the driver must reuse one scratch",
        stats.allocs
    );
    println!(
        "M2-29 scratch walk: {} B static, {} allocs / {} B, peak {} B, largest {} B",
        SCRATCH_BYTES, stats.allocs, stats.allocated, stats.peak, stats.max_single
    );
}

#[test]
fn scratch_budget_block_fill_stays_under_budget() {
    let chunk = noise_chunk();
    let apron = Apron::new();
    let edge = (CHUNK_DIM / BLOCK_EDGE) as u32;
    let mut scratch = BlockScratch::new();
    let (last, stats) = measure(|| {
        let mut last = 0u16;
        for bz in 0..edge {
            for by in 0..edge {
                for bx in 0..edge {
                    scratch.fill(&chunk, &apron, BlockPos::new(bx, by, bz));
                    last ^= scratch.get(1, 1, 1);
                }
            }
        }
        last
    });
    black_box(last);
    assert_budget(&stats, "filling every block");
    assert_eq!(
        stats.allocs, 0,
        "filling every block allocated {} time(s); the scratch must be reused",
        stats.allocs
    );
}

#[test]
fn scratch_budget_greedy_stays_under_budget() {
    let chunk = noise_chunk();
    let apron = Apron::new();
    // Reserved before the measurement: this is the one caller-owned buffer the
    // greedy core writes into. It must not grow during the walk.
    let mut quads = Vec::with_capacity(MAX_BLOCK_QUADS);
    let (quads_seen, stats) = measure(|| {
        let mut quads_seen = 0usize;
        let mut peak_quads = 0usize;
        for_each_block(&chunk, &apron, |_, scratch| {
            greedy_block(scratch, &mut quads);
            quads_seen += quads.len();
            peak_quads = peak_quads.max(quads.len());
        });
        (quads_seen, peak_quads)
    });
    let (quads_seen, peak_quads) = quads_seen;
    black_box(quads_seen);
    assert_budget(&stats, "the greedy core");
    assert_eq!(
        stats.allocs, 0,
        "the greedy core allocated {} time(s) despite a reused quad buffer",
        stats.allocs
    );
    assert!(
        peak_quads <= MAX_BLOCK_QUADS,
        "one block emitted {peak_quads} quads, above the {MAX_BLOCK_QUADS} bound"
    );
}

/// The 64 KiB per-worker budget, applied to everything the window allocated.
fn assert_budget(stats: &Stats, what: &str) {
    assert!(
        stats.max_single <= SCRATCH_BUDGET_BYTES as u64,
        "{what} allocated {} B in one block, over the {SCRATCH_BUDGET_BYTES} B budget",
        stats.max_single
    );
    assert!(
        stats.peak <= SCRATCH_BUDGET_BYTES as u64,
        "{what} held {} B live, over the {SCRATCH_BUDGET_BYTES} B budget",
        stats.peak
    );
    // A dense chunk copy is several times the budget at either edge, so a
    // densify could never hide under the per-allocation check above.
    let dense_chunk = (CHUNK_VOLUME * size_of::<u16>()) as u64;
    assert!(dense_chunk > SCRATCH_BUDGET_BYTES as u64);
}

#[test]
fn scratch_budget_full_build_never_densifies_the_chunk() {
    let chunk = noise_chunk();
    let apron = Apron::new();
    let mut quads = Vec::with_capacity(MAX_BLOCK_QUADS);

    // Pre-pass: the shape of the per-block output. A build is exactly two
    // allocations per non-empty block (`mesh_block` reserves its vertex and
    // index buffers), so any extra allocation — in particular a chunk-wide
    // densify — changes this count regardless of its size.
    let mut max_block_quads = 0usize;
    let mut total_quads = 0usize;
    let mut nonempty_blocks = 0u64;
    for_each_block(&chunk, &apron, |_, scratch| {
        greedy_block(scratch, &mut quads);
        max_block_quads = max_block_quads.max(quads.len());
        total_quads += quads.len();
        nonempty_blocks += u64::from(!quads.is_empty());
    });
    assert!(max_block_quads > 0);

    let (meshed, stats) = measure(|| {
        let mut meshed = 0usize;
        for_each_block(&chunk, &apron, |block, scratch| {
            greedy_block(scratch, &mut quads);
            let mesh = mesh_block(&quads, block);
            meshed += mesh.quad_count();
            black_box(&mesh);
        });
        meshed
    });
    assert_eq!(
        meshed, total_quads,
        "the measured build must repeat the pre-pass"
    );

    // Vertex bytes plus index bytes for one quad: 4 × 16 B + 6 × 2 B.
    let bytes_per_quad = 4 * size_of::<voxel_mesh::Vertex>() + 6 * size_of::<u16>();
    assert_eq!(
        stats.allocs,
        2 * nonempty_blocks,
        "the full build made {} allocations; only the two mesh buffers of each of \
         {nonempty_blocks} non-empty blocks are allowed (a chunk-wide buffer would show here)",
        stats.allocs
    );
    assert_eq!(
        stats.allocated,
        (total_quads * bytes_per_quad) as u64,
        "the full build allocated only its per-block mesh outputs"
    );
    assert_eq!(
        stats.peak,
        (max_block_quads * bytes_per_quad) as u64,
        "live output must be one block's mesh at a time"
    );
    assert!(
        stats.max_single <= (max_block_quads * 4 * size_of::<voxel_mesh::Vertex>()) as u64,
        "the largest allocation must be one block's vertex buffer"
    );
}
