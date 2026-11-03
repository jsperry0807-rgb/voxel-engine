//! Heap measurement through a counting global allocator.
//!
//! Install [`CountingAllocator`] as the process allocator (the binary does
//! this), then wrap a workload in [`measure`]. The counters are process-wide
//! and reset per measurement, so measurements must not overlap. The suite is
//! single-threaded on purpose: it is measuring the data structures, not the
//! scheduler.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

/// A [`GlobalAlloc`] that counts allocated bytes, allocation calls, live bytes
/// and the high-water mark.
///
/// It forwards to [`System`], so its overhead is one atomic increment per
/// allocation plus one fetch-max. That makes it suitable for measuring large
/// structures; for nanosecond-level microbenchmarks use Criterion instead.
pub struct CountingAllocator;

#[inline]
fn record_alloc(size: usize) {
    let size = size as u64;
    ALLOCATED.fetch_add(size, Ordering::Relaxed);
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

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
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarding to the system allocator with the same arguments.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            record_alloc(new_size);
        }
        new_ptr
    }
}

/// What one workload cost.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemStats {
    /// Bytes the result keeps alive after the workload.
    pub resident: u64,
    /// Bytes allocated over the workload, scratch included.
    pub allocated: u64,
    /// Allocation calls over the workload.
    pub allocs: u64,
    /// High-water mark of live bytes above the pre-workload baseline.
    pub peak: u64,
}

/// Current live heap bytes.
pub fn live_bytes() -> u64 {
    LIVE.load(Ordering::Relaxed)
}

/// Runs `f` with the allocation counters armed and reports what it cost.
///
/// The baseline is the live bytes before the call, so a workload measured
/// next to other live data reports only its own footprint.
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, MemStats) {
    let before = LIVE.load(Ordering::Relaxed);
    ALLOCATED.store(0, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);

    let out = f();

    let after = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(before);
    let stats = MemStats {
        resident: after.saturating_sub(before),
        allocated: ALLOCATED.load(Ordering::Relaxed),
        allocs: ALLOCS.load(Ordering::Relaxed),
        peak,
    };
    (out, stats)
}

/// Snapshot of the counters around an already-running workload.
///
/// Use this when the workload is a loop rather than a single call; `baseline`
/// is the value of [`live_bytes`] captured before the loop.
pub fn snapshot(baseline: u64, peak: u64) -> MemStats {
    MemStats {
        resident: live_bytes().saturating_sub(baseline),
        allocated: ALLOCATED.load(Ordering::Relaxed),
        allocs: ALLOCS.load(Ordering::Relaxed),
        peak: peak.saturating_sub(baseline),
    }
}

/// Resets the allocation counters and seeds the peak at the current live size.
pub fn reset() -> u64 {
    let live = LIVE.load(Ordering::Relaxed);
    ALLOCATED.store(0, Ordering::Relaxed);
    ALLOCS.store(0, Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

/// Peak live bytes above `baseline` since the last [`reset`].
pub fn peak_since(baseline: u64) -> u64 {
    PEAK.load(Ordering::Relaxed).saturating_sub(baseline)
}
