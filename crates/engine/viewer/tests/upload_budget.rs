//! M3-03 upload queue tests.
//!
//! The acceptance case is a 10 MB backlog: it must drain completely, one frame
//! at a time, without any frame moving more than its byte budget. The rest pins
//! ordering, the oversized-job escape hatch and the reported counters.
//!
//! The GPU path (staging belt + copy in the frame encoder) is exercised by
//! `cargo run -p voxel-viewer`, whose load log prints the per-frame maximum and
//! the peak backlog.

use voxel_viewer::upload::{
    DEFAULT_UPLOAD_BYTES, MAX_BACKLOG_BYTES, STAGING_CHUNK_BYTES, UploadBytes, UploadQueue,
};

/// One queued mesh of a given size.
struct Job {
    id: usize,
    bytes: usize,
}

impl UploadBytes for Job {
    fn upload_bytes(&self) -> usize {
        self.bytes
    }
}

fn job(id: usize, bytes: usize) -> Job {
    Job { id, bytes }
}

/// Pushes `jobs` one at a time and drains frame by frame, returning the taken
/// ids per frame and the bytes per frame.
fn drain(queue: &mut UploadQueue<Job>, budget: usize) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut frames = Vec::new();
    let mut frame_bytes = Vec::new();
    while !queue.is_empty() {
        let frame = queue.take_frame(budget);
        assert!(!frame.is_empty(), "a non-empty queue must yield a job");
        frame_bytes.push(frame.iter().map(|job| job.bytes).sum());
        frames.push(frame.iter().map(|job| job.id).collect());
        // Every drain call is a frame, including a full one.
        assert!(frames.len() < 1000, "the queue did not drain");
    }
    (frames, frame_bytes)
}

#[test]
fn ten_megabytes_drain_within_the_budget() {
    // The measured 128³ mean mesh is ~470 KiB; 23 of them are just over 10 MB.
    let mesh_bytes = 470 * 1024;
    let total = 10 * 1024 * 1024;
    let mut queue = UploadQueue::new();
    let mut jobs = 0;
    let mut pushed = 0;
    while pushed < total {
        queue.push(job(jobs, mesh_bytes));
        jobs += 1;
        pushed += mesh_bytes;
    }
    assert!(queue.backlog_bytes() >= total);

    let (frames, frame_bytes) = drain(&mut queue, DEFAULT_UPLOAD_BYTES);
    assert!(
        frames.len() >= total.div_ceil(DEFAULT_UPLOAD_BYTES),
        "the 10 MB backlog cannot drain in fewer frames than the budget allows"
    );
    for bytes in &frame_bytes {
        assert!(
            *bytes <= DEFAULT_UPLOAD_BYTES,
            "a frame uploaded {bytes} B over a {} B budget",
            DEFAULT_UPLOAD_BYTES
        );
    }
    let ids: Vec<usize> = frames.into_iter().flatten().collect();
    assert_eq!(ids.len(), jobs);
    assert_eq!(
        ids,
        (0..jobs).collect::<Vec<_>>(),
        "jobs drain in order and exactly once"
    );
    assert_eq!(queue.backlog_bytes(), 0);

    let stats = queue.stats();
    assert_eq!(stats.jobs, jobs as u64);
    assert_eq!(stats.bytes, (jobs * mesh_bytes) as u64);
    assert!(stats.max_frame_bytes <= DEFAULT_UPLOAD_BYTES);
    assert!(stats.peak_backlog_bytes >= total);
    assert_eq!(stats.overruns, 0, "no single mesh exceeds the budget");
}

// The cap is backpressure, not a drain target: it only has to hold several
// frames of work so the pipeline does not thrash between "stop draining" and
// "resume draining", and stay small against the 1.5 GiB host budget. A staging
// chunk should cover a quarter frame, so a frame's writes share chunks.
const _: () = assert!(MAX_BACKLOG_BYTES >= DEFAULT_UPLOAD_BYTES * 4);
const _: () = assert!(MAX_BACKLOG_BYTES <= 128 * 1024 * 1024);
const _: () = assert!(STAGING_CHUNK_BYTES >= (DEFAULT_UPLOAD_BYTES / 4) as u64);

#[test]
fn the_backlog_cap_is_reported() {
    // Runtime companion: the queue reaches the cap and keeps accounting for it,
    // which is what `pump` checks before draining the worker channel.
    let mut queue = UploadQueue::new();
    let job_bytes = MAX_BACKLOG_BYTES / 4;
    for id in 0..4 {
        queue.push(job(id, job_bytes));
    }
    assert_eq!(queue.backlog_bytes(), MAX_BACKLOG_BYTES);
    assert_eq!(queue.peak_bytes(), MAX_BACKLOG_BYTES);
}

#[test]
fn a_mesh_larger_than_the_budget_still_drains() {
    let mut queue = UploadQueue::new();
    queue.push(job(0, 300));
    queue.push(job(1, 2 * 1024 * 1024));
    queue.push(job(2, 300));

    let budget = 64 * 1024;
    let first = queue.take_frame(budget);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, 0);
    let second = queue.take_frame(budget);
    assert_eq!(second.len(), 1, "the oversized mesh is taken alone");
    assert_eq!(second[0].id, 1);
    assert_eq!(queue.stats().overruns, 1);
    let third = queue.take_frame(budget);
    assert_eq!(third[0].id, 2);
    assert!(queue.is_empty());
}

#[test]
fn a_frame_stops_at_the_next_job_that_does_not_fit() {
    let mut queue = UploadQueue::new();
    for id in 0..5 {
        queue.push(job(id, 300 * 1024));
    }
    let frame = queue.take_frame(700 * 1024);
    assert_eq!(frame.len(), 2, "two meshes fit; the third would exceed");
    assert_eq!(queue.backlog_bytes(), 900 * 1024);
    assert_eq!(queue.peak_bytes(), 1500 * 1024);
}

#[test]
fn an_empty_queue_takes_nothing_and_still_counts_the_frame() {
    let mut queue: UploadQueue<Job> = UploadQueue::new();
    assert!(queue.take_frame(DEFAULT_UPLOAD_BYTES).is_empty());
    assert_eq!(queue.stats().frames, 1);
    assert_eq!(queue.stats().jobs, 0);
    assert_eq!(queue.stats().bytes, 0);
}
