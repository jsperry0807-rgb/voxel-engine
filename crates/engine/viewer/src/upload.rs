//! M3-03 upload queue: finished chunk meshes waiting for a frame's byte budget.
//!
//! The uploader thread allocates the pool range and prepares the light bind
//! group, but it no longer writes mesh bytes. Finished work travels to the
//! frame thread in a [`PendingUpload`]; each frame takes at most
//! [`DEFAULT_UPLOAD_BYTES`] of mesh data and copies it into the pool through a
//! `StagingBelt`, so:
//!
//! - the GPU copy is recorded in the same command buffer as the draw that first
//!   uses the mesh, which orders it without a fence;
//! - the per-frame cost is bounded by a byte budget instead of a chunk count;
//! - the staging buffer is a belt chunk reused across frames, not a new
//!   allocation per mesh (the M3-02 measurement showed `Queue::write_buffer`
//!   doing exactly that, with 110–360 ms outliers).
//!
//! The queue always yields at least one job when it is not empty, so a mesh
//! larger than the budget cannot stall the ring; that frame is counted as an
//! overrun instead.

use std::collections::VecDeque;

/// Mesh bytes one frame may upload: 512 KiB.
///
/// Sized from the M3-09 hitch measurement on the reference machine. Mapped
/// staging memory takes ~2.5 µs/KiB there (a discrete GPU under WSL/Dozen), so
/// the copy dominates the frame-thread upload path: at 1 MiB the upload stage's
/// p99 was 6.8 ms over 1,000 frames of motion, over the 4 ms budget; at 512 KiB
/// it measured 1.7–1.8 ms p99 with a total-frame max under 8 ms. The cost is
/// more upload frames to drain the same ring (about twice as many).
///
/// A mesh above the budget is taken alone and counted as an overrun rather than
/// stalling the queue, so a chunk larger than 512 KiB still makes progress.
pub const DEFAULT_UPLOAD_BYTES: usize = 512 * 1024;

/// Staging-belt chunk size. A chunk is reused across frames; it is grown to fit
/// a single write that is larger than this.
pub const STAGING_CHUNK_BYTES: u64 = 2 * 1024 * 1024;

/// CPU-side backlog cap. The uploader blocks on the result channel once this
/// much finished work is waiting, which keeps queued mesh bytes bounded.
pub const MAX_BACKLOG_BYTES: usize = 32 * 1024 * 1024;

/// An item that costs bytes against a frame's upload budget.
pub trait UploadBytes {
    /// Bytes this item moves into the pool.
    fn upload_bytes(&self) -> usize;
}

/// A FIFO of finished uploads drained under a per-frame byte budget.
#[derive(Debug)]
pub struct UploadQueue<T> {
    jobs: VecDeque<T>,
    bytes: usize,
    peak_bytes: usize,
    frames: u64,
    jobs_taken: u64,
    bytes_taken: u64,
    max_frame_bytes: usize,
    overruns: u64,
}

impl<T> Default for UploadQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> UploadQueue<T> {
    /// Creates an empty queue.
    pub fn new() -> Self {
        Self {
            jobs: VecDeque::new(),
            bytes: 0,
            peak_bytes: 0,
            frames: 0,
            jobs_taken: 0,
            bytes_taken: 0,
            max_frame_bytes: 0,
            overruns: 0,
        }
    }

    /// Jobs waiting.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

impl<T: UploadBytes> UploadQueue<T> {
    /// Appends one finished upload.
    pub fn push(&mut self, job: T) {
        self.bytes += job.upload_bytes();
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        self.jobs.push_back(job);
    }

    /// Mesh bytes waiting.
    pub fn backlog_bytes(&self) -> usize {
        self.bytes
    }

    /// Largest backlog seen since construction.
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }

    /// Removes up to `budget` bytes of work for one frame.
    ///
    /// Jobs are taken in order and the first job that does not fit stops the
    /// frame. A single job larger than `budget` is taken alone, so a big mesh
    /// drains instead of blocking the queue; that frame is counted as an
    /// overrun. An empty queue returns an empty vector.
    pub fn take_frame(&mut self, budget: usize) -> Vec<T> {
        let mut taken = Vec::new();
        let mut frame_bytes = 0usize;
        while let Some(job) = self.jobs.front() {
            let cost = job.upload_bytes();
            if !taken.is_empty() && frame_bytes + cost > budget {
                break;
            }
            let job = self.jobs.pop_front().expect("front was checked");
            self.bytes -= cost;
            frame_bytes += cost;
            if cost > budget {
                self.overruns += 1;
            }
            taken.push(job);
        }
        self.frames += 1;
        self.jobs_taken += taken.len() as u64;
        self.bytes_taken += frame_bytes as u64;
        self.max_frame_bytes = self.max_frame_bytes.max(frame_bytes);
        taken
    }

    /// Cumulative drain statistics.
    pub fn stats(&self) -> UploadStats {
        UploadStats {
            frames: self.frames,
            jobs: self.jobs_taken,
            bytes: self.bytes_taken,
            max_frame_bytes: self.max_frame_bytes,
            peak_backlog_bytes: self.peak_bytes,
            overruns: self.overruns,
        }
    }
}

/// Counters for the log line and the M3-06 overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadStats {
    /// Frames that drained the queue, including empty ones.
    pub frames: u64,
    /// Jobs moved into the pool.
    pub jobs: u64,
    /// Mesh bytes moved into the pool.
    pub bytes: u64,
    /// Largest single frame's bytes.
    pub max_frame_bytes: usize,
    /// Largest backlog seen.
    pub peak_backlog_bytes: usize,
    /// Frames where one job exceeded the budget.
    pub overruns: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Job {
        bytes: usize,
    }

    impl UploadBytes for Job {
        fn upload_bytes(&self) -> usize {
            self.bytes
        }
    }

    fn job(bytes: usize) -> Job {
        Job { bytes }
    }

    #[test]
    fn an_empty_queue_takes_nothing() {
        let mut queue = UploadQueue::<Job>::new();
        assert!(queue.take_frame(1 << 20).is_empty());
        assert_eq!(queue.stats().frames, 1);
    }

    #[test]
    fn a_frame_stops_before_it_would_exceed_the_budget() {
        let mut queue = UploadQueue::new();
        queue.push(job(300));
        queue.push(job(300));
        queue.push(job(300));
        let frame = queue.take_frame(700);
        assert_eq!(frame.len(), 2, "two 300 B jobs fit, the third does not");
        assert_eq!(queue.backlog_bytes(), 300);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn one_oversized_job_drains_alone() {
        let mut queue = UploadQueue::new();
        queue.push(job(2_000));
        let frame = queue.take_frame(1_000);
        assert_eq!(frame.len(), 1);
        assert_eq!(queue.stats().overruns, 1);
        assert_eq!(queue.backlog_bytes(), 0);
    }
}
