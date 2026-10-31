//! M3-01 streaming shell: a precomputed chunk ring, background generation and
//! upload, and a bounded per-frame install budget.
//!
//! The viewer keeps a 16×16 ring of chunks (256 chunks) around the camera's
//! horizontal chunk position. Missing ring positions are generated and meshed
//! on worker threads, uploaded to the GPU by one uploader thread, and installed
//! by [`Streaming::pump`]. The frame thread never creates a buffer or waits on
//! a driver allocation: it only drains finished resources within
//! [`PUMP_BUDGET`].
//!
//! The uploader is deliberate. Per-chunk buffer creation and mapped writes
//! measured 10–20 ms stalls on the frame thread on the reference machine, so a
//! frame-thread upload budget cannot be met by doing the work there. M3-02's
//! arena ([`crate::mesh_pool`]) removes the per-chunk mesh allocation; the
//! write path is still one staging copy per mesh, which M3-03 replaces with a
//! budgeted upload queue.
//!
//! The ring is horizontal: every chunk is generated at `y = 0`. That keeps the
//! M3-01 shell's working set at 256 chunks while the camera moves over the
//! terrain; vertical streaming is later work.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use voxel_core::constants::{CHUNK_DIM, CHUNK_VOLUME, VOXEL_SIZE};
use voxel_storage::apron::{Face, SIDE};
use voxel_storage::{Apron, ChunkPos};
use wgpu::{BindGroup, BindGroupLayout, Device, RenderPass};

use crate::chunk_content::{self, ChunkContent};
use crate::chunk_lighting::{self, ChunkLighting};
use crate::chunk_render::ChunkPipeline;
use crate::mesh_pool::{self, MeshAllocation, MeshFormat, MeshPool};
use crate::upload::{self, UploadBytes, UploadQueue};

/// Half the ring width. The ring spans `-RING_HALF ..= RING_HALF - 1` on each
/// horizontal axis, so `2 * RING_HALF = 16` chunks per axis.
pub const RING_HALF: i32 = 8;
/// Chunks in one ring: `16 * 16`.
pub const RING_CHUNKS: usize = (2 * RING_HALF as usize) * (2 * RING_HALF as usize);
/// Frame-thread install budget for one frame.
pub const PUMP_BUDGET: Duration = Duration::from_millis(4);
/// Number of chunk results buffered between pipeline stages. A small bound
/// keeps a slow stage from accumulating a large CPU/GPU backlog.
const RESULT_BACKLOG: usize = 16;

/// Environment override for the M3-02 pool page size, in MiB.
const POOL_PAGE_ENV: &str = "VOXEL_POOL_PAGE_MIB";

/// The pool page size: [`mesh_pool::DEFAULT_PAGE_BYTES`], or
/// `VOXEL_POOL_PAGE_MIB`, clamped to the pool's range and the adapter's
/// `max_buffer_size` so a small-limit adapter cannot fail at startup.
fn pool_page_bytes(max_buffer_size: u64) -> u64 {
    let default_mib = mesh_pool::DEFAULT_PAGE_BYTES / (1024 * 1024);
    let mib = std::env::var(POOL_PAGE_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default_mib);
    let requested = (mib * 1024 * 1024).clamp(mesh_pool::MIN_PAGE_BYTES, mesh_pool::MAX_PAGE_BYTES);
    requested.min(max_buffer_size)
}

/// One chunk's uploaded resources.
pub struct ChunkResources {
    /// The chunk's pooled mesh ranges, or `None` for an empty volume.
    pub mesh: Option<MeshAllocation>,
    /// Light, probes and world-origin bind group.
    pub lighting: ChunkLighting,
}

/// The desired ring positions, nearest first.
///
/// The ring is centered on `center.x`/`center.z` and generated at `y = 0`.
/// Ordering is stable: distance squared, then `z`, then `x`.
pub fn ring_chunks(center: ChunkPos) -> Vec<ChunkPos> {
    let base = ChunkPos::new(center.x, 0, center.z);
    let mut chunks = Vec::with_capacity(RING_CHUNKS);
    for dz in -RING_HALF..RING_HALF {
        for dx in -RING_HALF..RING_HALF {
            chunks.push(ChunkPos::new(base.x + dx, 0, base.z + dz));
        }
    }
    chunks.sort_by_key(|chunk| {
        let dx = i64::from(chunk.x - base.x);
        let dz = i64::from(chunk.z - base.z);
        (dx * dx + dz * dz, chunk.z, chunk.x)
    });
    chunks
}

/// The difference between a new ring and the renderer's current state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RingDelta {
    /// Missing positions, nearest first.
    pub load: Vec<ChunkPos>,
    /// Loaded or in-flight positions outside the ring.
    pub evict: Vec<ChunkPos>,
}

/// Computes the load/evict delta for `center`.
pub fn ring_delta(
    center: ChunkPos,
    loaded: impl Iterator<Item = ChunkPos>,
    requested: impl Iterator<Item = ChunkPos>,
) -> RingDelta {
    let desired = ring_chunks(center);
    let wanted: HashSet<ChunkPos> = desired.iter().copied().collect();
    let loaded: HashSet<ChunkPos> = loaded.collect();
    let requested: HashSet<ChunkPos> = requested.collect();
    let mut evict: Vec<ChunkPos> = loaded.difference(&wanted).copied().collect();
    evict.extend(requested.difference(&wanted).copied());
    evict.sort_by_key(|position| (position.x, position.y, position.z));
    evict.dedup();

    let load = desired
        .into_iter()
        .filter(|position| !loaded.contains(position) && !requested.contains(position))
        .collect();
    RingDelta { load, evict }
}

/// Statistics for one [`Streaming::pump`] call.
#[derive(Clone, Copy, Debug, Default)]
pub struct PumpStats {
    /// Chunks resident after this pump.
    pub loaded: usize,
    /// Desired chunks not yet resident (includes in-flight work).
    pub pending: usize,
    /// Finished uploads moved from the channel into the upload queue.
    pub queued: usize,
    /// Time spent in this pump.
    pub elapsed: Duration,
    /// Largest pump time seen since construction.
    pub max_elapsed: Duration,
    /// Pumps that exceeded [`PUMP_BUDGET`].
    pub hitches: u32,
}

/// A deterministic terrain volume for one chunk.
///
/// The height field is a smooth, world-coherent surface, so neighbouring
/// chunks share edge columns and greedy meshing finds long runs. The surface is
/// deliberately shallow near the viewer's start position so the default camera
/// begins above it.
pub fn stream_volume(chunk: ChunkPos) -> Vec<u16> {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    fill_stream_volume(chunk, &mut volume);
    volume
}

/// Fills `volume` with the chunk's terrain, clearing it first.
///
/// Workers reuse one volume buffer, which keeps a 16×16 ring from churning
/// hundreds of 4 MiB allocations through the global allocator.
pub fn fill_stream_volume(chunk: ChunkPos, volume: &mut [u16]) {
    assert_eq!(volume.len(), CHUNK_VOLUME, "a chunk volume is CHUNK_VOLUME");
    volume.fill(0);
    let base_x = chunk.x * CHUNK_DIM as i32;
    let base_y = chunk.y * CHUNK_DIM as i32;
    let base_z = chunk.z * CHUNK_DIM as i32;

    for local_z in 0..CHUNK_DIM {
        for local_x in 0..CHUNK_DIM {
            let world_x = base_x + local_x as i32;
            let world_z = base_z + local_z as i32;
            let surface = terrain_height(world_x, world_z) as i32;
            let top = (surface - base_y).clamp(0, CHUNK_DIM as i32) as usize;
            for local_y in 0..top {
                let world_y = base_y + local_y as i32;
                let index = local_x
                    | (local_y << voxel_core::constants::CHUNK_BITS)
                    | (local_z << (2 * voxel_core::constants::CHUNK_BITS));
                volume[index] = terrain_material(world_x, world_y, world_z);
            }
        }
    }
}

/// The six face planes around `chunk`, sampled from the same height field as
/// [`stream_volume`].
///
/// A chunk meshed with an all-air apron emits faces at every border and treats
/// the chunk edge as air for AO. The analytic apron removes those false faces
/// and makes the border AO match the neighbour. Corner samples still read air,
/// which is the documented one-voxel-apron limitation.
pub fn stream_apron(chunk: ChunkPos) -> Apron {
    let base_x = chunk.x * CHUNK_DIM as i32;
    let base_y = chunk.y * CHUNK_DIM as i32;
    let base_z = chunk.z * CHUNK_DIM as i32;
    let mut apron = Apron::new();
    for face in Face::ALL {
        for v in 0..SIDE {
            for u in 0..SIDE {
                let (dx, dy, dz) = face.offset(u, v);
                apron.set(
                    face,
                    u,
                    v,
                    terrain_material(base_x + dx, base_y + dy, base_z + dz),
                );
            }
        }
    }
    apron
}

/// The material at one world voxel, or air.
fn terrain_material(world_x: i32, world_y: i32, world_z: i32) -> u16 {
    let surface = terrain_height(world_x, world_z) as i32;
    if world_y < 0 || world_y >= surface {
        0
    } else if world_y + 1 >= surface {
        3
    } else if world_y + 4 >= surface {
        2
    } else {
        1
    }
}

/// World-voxel surface height for a terrain column.
///
/// Pure and deterministic: integer-hash value noise plus a shallow basin around the
/// viewer's start column.
pub fn terrain_height(world_x: i32, world_z: i32) -> usize {
    let noise = value_noise(world_x as f32 / 48.0, world_z as f32 / 48.0, 0x5EED_1234);
    let detail = value_noise(world_x as f32 / 19.0, world_z as f32 / 19.0, 0xC0FF_EE01);
    let relief = 0.72 * noise + 0.28 * detail;

    // The default camera starts at world voxel (72, 96). Flatten the terrain
    // there so the view is not born inside a hill; the effect fades over
    // roughly three chunks.
    let dx = (world_x - 72) as f32;
    let dz = (world_z - 96) as f32;
    let distance_sq = dx * dx + dz * dz;
    let basin = 1.0 - (distance_sq / (3.0 * CHUNK_DIM as f32).powi(2)).min(1.0);
    let height = 12.0 + relief * 30.0 * (1.0 - 0.8 * basin);
    height.round() as usize
}

/// Smooth 2D value noise in `0..=1`.
fn value_noise(x: f32, z: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let tx = smoothstep(x - x0);
    let tz = smoothstep(z - z0);
    let x0 = x0 as i32;
    let z0 = z0 as i32;
    let a = hash_to_unit(x0, z0, seed);
    let b = hash_to_unit(x0 + 1, z0, seed);
    let c = hash_to_unit(x0, z0 + 1, seed);
    let d = hash_to_unit(x0 + 1, z0 + 1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * tz
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn hash_to_unit(x: i32, z: i32, seed: u32) -> f32 {
    let mut hash = (x as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((z as u32).wrapping_mul(0x85EB_CA77))
        .wrapping_add(seed);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    (hash & 0xFFFF) as f32 / 65_535.0
}

/// World-voxel origin of a chunk for the shader's `chunk_origin` uniform.
pub fn chunk_origin(chunk: ChunkPos) -> [f32; 3] {
    [
        (chunk.x * CHUNK_DIM as i32) as f32,
        (chunk.y * CHUNK_DIM as i32) as f32,
        (chunk.z * CHUNK_DIM as i32) as f32,
    ]
}

/// Converts a world position to the horizontal ring center at `y = 0`.
pub fn center_for(position: [f32; 4]) -> ChunkPos {
    let edge = CHUNK_DIM as f32 * VOXEL_SIZE;
    ChunkPos::new(
        (position[0] / edge).floor() as i32,
        0,
        (position[2] / edge).floor() as i32,
    )
}

/// One generator result.
struct Job {
    position: ChunkPos,
    /// `None` when generation panicked; the worker logs the reason.
    content: Option<ChunkContent>,
    /// Time this worker spent generating, excluding the queue wait.
    generated: Duration,
}
/// One finished chunk waiting for the frame thread to upload its mesh bytes.
///
/// The arena range and the light bind group are ready; `vertices` and `indices`
/// are the CPU bytes the frame copies through the staging belt under the
/// per-frame budget.
pub struct PendingUpload {
    /// Where the chunk belongs in the ring.
    pub position: ChunkPos,
    /// The reserved pool range, or `None` for a chunk with no visible faces.
    pub mesh: Option<MeshAllocation>,
    /// Packed 8 B vertex stream, moved rather than cloned.
    pub vertices: Vec<u8>,
    /// `u16` index stream.
    pub indices: Vec<u16>,
    /// Light, probes and world-origin bind group.
    pub lighting: ChunkLighting,
}

impl UploadBytes for PendingUpload {
    fn upload_bytes(&self) -> usize {
        self.vertices.len() + self.indices.len() * 2
    }
}

/// An uploader result: `None` when generation or preparation failed. The
/// failing stage logs the reason; the frame thread only accounts for the hole.
type UploadedJob = (ChunkPos, Option<PendingUpload>);

/// Shared work queue for the generator threads.
struct WorkQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct QueueState {
    /// Positions and the instant they were queued, so a worker can report how
    /// long a job waited behind the pool.
    jobs: VecDeque<(ChunkPos, Instant)>,
    closed: bool,
}

impl WorkQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                jobs: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn enqueue(&self, jobs: impl IntoIterator<Item = ChunkPos>) {
        let mut state = self.lock();
        let queued = Instant::now();
        state
            .jobs
            .extend(jobs.into_iter().map(|position| (position, queued)));
        self.ready.notify_all();
    }

    /// Takes the next job and the instant it was queued.
    fn take(&self) -> Option<(ChunkPos, Instant)> {
        let mut state = self.lock();
        loop {
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        // Shutdown must not drain the backlog: a 256-chunk ring can still have
        // seconds of queued work when the window closes.
        state.jobs.clear();
        drop(state);
        self.ready.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The streaming ring and its worker pool.
///
/// Three stages keep the frame thread out of the costly work:
/// generator/mesher workers, one preparer that allocates pool ranges and light
/// buffers, and the frame thread, which copies at most the upload budget and
/// installs the result. Pool allocation and light-buffer creation still block
/// on the driver for milliseconds, so they stay off the frame thread.
pub struct Streaming {
    chunks: HashMap<ChunkPos, ChunkResources>,
    requested: HashSet<ChunkPos>,
    desired: HashSet<ChunkPos>,
    center: Option<ChunkPos>,
    work: Arc<WorkQueue>,
    /// The M3-02 mesh arena, shared with the uploader thread.
    pool: Arc<Mutex<MeshPool>>,
    /// Finished meshes waiting for a frame's upload budget (M3-03).
    uploads: UploadQueue<PendingUpload>,
    /// Mesh bytes one frame may copy into the pool.
    upload_budget: usize,
    receiver: Option<Receiver<UploadedJob>>,
    workers: Vec<JoinHandle<()>>,
    uploader: Option<JoinHandle<()>>,
    max_pump: Duration,
    last_budget: Duration,
    /// Frames that moved bytes, and the slowest/total time spent copying them.
    upload_frames: u64,
    upload_frame_total: Duration,
    upload_frame_max: Duration,
    hitches: u32,
    complete_logged: bool,
    uploader_down: bool,
}

impl Streaming {
    /// Starts the generator pool. Nothing is queued until the first
    /// [`Self::pump`] supplies a camera center.
    pub fn new(device: Device, data_layout: BindGroupLayout, upload_budget: usize) -> Self {
        let page_bytes = pool_page_bytes(device.limits().max_buffer_size);
        let pool = Arc::new(Mutex::new(MeshPool::new(
            device.clone(),
            MeshFormat::PACKED_8B,
            page_bytes,
        )));
        log::debug!(
            "pool: {} MiB pages for the {} B packed stream",
            page_bytes / (1024 * 1024),
            MeshFormat::PACKED_8B.vertex_stride()
        );
        let work = Arc::new(WorkQueue::new());
        let (content_sender, content_receiver) = sync_channel::<Job>(RESULT_BACKLOG);
        let worker_count = std::env::var("VOXEL_STREAM_WORKERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, std::num::NonZeroUsize::get)
                    .min(4)
            })
            .clamp(1, 8);
        let workers = (0..worker_count)
            .map(|index| {
                let work = Arc::clone(&work);
                let sender = content_sender.clone();
                std::thread::Builder::new()
                    .name(format!("chunk-stream-{index}"))
                    .spawn(move || worker_loop(&work, &sender))
                    .expect("spawn chunk-stream worker")
            })
            .collect();
        drop(content_sender);

        let (upload_sender, upload_receiver) = sync_channel::<UploadedJob>(RESULT_BACKLOG);
        let uploader = std::thread::Builder::new()
            .name("chunk-upload".to_owned())
            .spawn({
                let pool = Arc::clone(&pool);
                move || uploader_loop(device, data_layout, pool, content_receiver, upload_sender)
            })
            .expect("spawn chunk uploader");
        log::debug!("stream: {worker_count} generator workers, ring {RING_CHUNKS} chunks");
        Self {
            chunks: HashMap::with_capacity(RING_CHUNKS),
            requested: HashSet::with_capacity(RING_CHUNKS),
            desired: HashSet::with_capacity(RING_CHUNKS),
            center: None,
            work,
            pool,
            uploads: UploadQueue::new(),
            upload_budget,
            receiver: Some(upload_receiver),
            workers,
            uploader: Some(uploader),
            max_pump: Duration::ZERO,
            last_budget: PUMP_BUDGET,
            upload_frames: 0,
            upload_frame_total: Duration::ZERO,
            upload_frame_max: Duration::ZERO,
            hitches: 0,
            complete_logged: false,
            uploader_down: false,
        }
    }

    /// Resident chunk count.
    pub fn loaded(&self) -> usize {
        self.chunks.len()
    }

    /// Desired ring positions not resident yet (includes in-flight work).
    pub fn pending(&self) -> usize {
        self.desired.len().saturating_sub(self.chunks.len())
    }

    /// Largest pump time since construction.
    pub fn max_pump(&self) -> Duration {
        self.max_pump
    }

    /// Pumps that exceeded [`PUMP_BUDGET`].
    pub fn hitches(&self) -> u32 {
        self.hitches
    }

    /// Re-centers the ring if needed, then moves finished uploads from the
    /// worker channel into the byte-budgeted queue within `budget`.
    ///
    /// Moving a finished upload is a channel pop plus a map insert, so a pump
    /// normally finishes well inside the budget. The GPU copy happens later, in
    /// [`Self::take_uploads`] and the frame's command encoder, under the M3-03
    /// byte budget. `max_pump`/`hitches` cover this pump only (including the
    /// recenter); they do not include draw, submit or the upload copy.
    pub fn pump(&mut self, center: ChunkPos, budget: Duration) -> PumpStats {
        let started = Instant::now();
        self.last_budget = budget;
        if self.center != Some(center) {
            self.recenter(center);
        }

        let mut queued = 0usize;
        while started.elapsed() < budget && self.uploads.backlog_bytes() < upload::MAX_BACKLOG_BYTES
        {
            let Some(receiver) = &self.receiver else {
                break;
            };
            match receiver.try_recv() {
                Ok((position, upload)) => {
                    self.requested.remove(&position);
                    match upload {
                        Some(upload) => {
                            self.uploads.push(upload);
                            queued += 1;
                        }
                        None => {
                            log::error!(
                                "stream: {position:?} failed in generation or preparation (see the worker/preparer log)"
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.uploader_down {
                        self.uploader_down = true;
                        log::error!("stream: uploader disconnected; the ring cannot complete");
                    }
                    break;
                }
            }
        }

        let elapsed = started.elapsed();
        self.max_pump = self.max_pump.max(elapsed);
        if elapsed > budget {
            self.hitches += 1;
            log::warn!(
                "stream hitch: pump took {:.2} ms, budget {:.2} ms",
                elapsed.as_secs_f64() * 1000.0,
                budget.as_secs_f64() * 1000.0
            );
        }

        let stats = PumpStats {
            loaded: self.chunks.len(),
            pending: self.pending(),
            queued,
            elapsed,
            max_elapsed: self.max_pump,
            hitches: self.hitches,
        };
        self.maybe_log_complete();
        stats
    }

    /// Takes at most `budget` bytes of finished uploads for this frame.
    ///
    /// Results for positions the camera already left are freed here instead of
    /// being installed, so a recenter cannot leak pooled ranges. The returned
    /// jobs must be passed to [`Self::install_uploads`] after their bytes have
    /// been copied.
    pub fn take_uploads(&mut self, budget: usize) -> Vec<PendingUpload> {
        let mut keep = Vec::new();
        for upload in self.uploads.take_frame(budget) {
            if self.desired.contains(&upload.position) {
                keep.push(upload);
            } else if let Some(mesh) = upload.mesh {
                self.pool
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .free(mesh);
            }
        }
        keep
    }

    /// Installs uploads whose copies are recorded in the frame being submitted.
    ///
    /// Returns how many became resident. A duplicate result for an already
    /// resident position replaces it and frees the superseded ranges.
    pub fn install_uploads(&mut self, uploads: Vec<PendingUpload>) -> usize {
        let mut installed = 0usize;
        for upload in uploads {
            let resources = ChunkResources {
                mesh: upload.mesh,
                lighting: upload.lighting,
            };
            if let Some(previous) = self.chunks.insert(upload.position, resources)
                && let Some(mesh) = previous.mesh
            {
                self.pool
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .free(mesh);
            }
            installed += 1;
            log::trace!("stream: installed {:?}", upload.position);
        }
        self.maybe_log_complete();
        installed
    }

    /// Upload bytes waiting for the frame thread.
    pub fn upload_backlog_bytes(&self) -> usize {
        self.uploads.backlog_bytes()
    }

    /// Upload queue counters.
    pub fn upload_stats(&self) -> upload::UploadStats {
        self.uploads.stats()
    }

    /// Records the CPU time one frame spent copying uploads into the pool.
    /// Frames that moved no bytes are not counted, so the mean is per upload
    /// frame rather than per rendered frame.
    pub fn note_upload_frame(&mut self, bytes: usize, elapsed: Duration) {
        if bytes == 0 {
            return;
        }
        self.upload_frames += 1;
        self.upload_frame_total += elapsed;
        self.upload_frame_max = self.upload_frame_max.max(elapsed);
    }

    /// Logs the ring, pool and upload summaries once, when the ring completes.
    fn maybe_log_complete(&mut self) {
        if self.complete_logged || self.chunks.len() != RING_CHUNKS {
            return;
        }
        self.complete_logged = true;
        log::info!(
            "stream: {RING_CHUNKS} loaded, {} pending; max pump {:.2} ms, {} hitches over {:.0} ms",
            self.pending(),
            self.max_pump.as_secs_f64() * 1000.0,
            self.hitches,
            self.last_budget.as_secs_f64() * 1000.0
        );
        let (pool, page_creation) = {
            let pool = self
                .pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (pool.stats(), pool.page_creation_time())
        };
        log::info!(
            "pool: {} pages x {:.0} MiB = {:.1} MiB for {:.1} MiB of meshes; {:.1} MiB slack, {:.1} MiB free in holes ({:.1}% fragmented); {} live meshes, {} mesh allocations, pages created in {:.2} s",
            pool.pages,
            pool.page_bytes as f64 / (1024.0 * 1024.0),
            pool.capacity_bytes as f64 / (1024.0 * 1024.0),
            pool.used_bytes as f64 / (1024.0 * 1024.0),
            pool.slack_bytes as f64 / (1024.0 * 1024.0),
            pool.hole_bytes as f64 / (1024.0 * 1024.0),
            pool.fragmentation * 100.0,
            self.chunks.len(),
            pool.allocations,
            page_creation.as_secs_f64(),
        );
        let upload = self.uploads.stats();
        let mean_frame = if self.upload_frames == 0 {
            0.0
        } else {
            self.upload_frame_total.as_secs_f64() * 1000.0 / self.upload_frames as f64
        };
        log::info!(
            "upload: {} meshes, {:.1} MiB in {} frames; max frame {:.0} KiB of {:.0} KiB, peak backlog {:.1} MiB, {} overruns; copy {:.2} ms mean, {:.2} ms max",
            upload.jobs,
            upload.bytes as f64 / (1024.0 * 1024.0),
            upload.frames,
            upload.max_frame_bytes as f64 / 1024.0,
            self.upload_budget as f64 / 1024.0,
            upload.peak_backlog_bytes as f64 / (1024.0 * 1024.0),
            upload.overruns,
            mean_frame,
            self.upload_frame_max.as_secs_f64() * 1000.0,
        );
    }

    fn recenter(&mut self, center: ChunkPos) {
        let delta = ring_delta(
            center,
            self.chunks.keys().copied(),
            self.requested.iter().copied(),
        );
        if !delta.evict.is_empty() {
            // Free the mesh ranges before queueing the new column, so the
            // uploader can reuse them for the chunks that replace them.
            let mut pool = self
                .pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for position in &delta.evict {
                if let Some(resources) = self.chunks.remove(position)
                    && let Some(mesh) = resources.mesh
                {
                    pool.free(mesh);
                }
                self.requested.remove(position);
            }
            drop(pool);
            log::debug!("stream: evicted {} chunks", delta.evict.len());
        }
        let desired = ring_chunks(center);
        // Reuse the set's capacity: a recenter must not churn the allocator.
        self.desired.clear();
        self.desired.extend(desired.iter().copied());
        self.center = Some(center);
        if !delta.load.is_empty() {
            self.requested.extend(delta.load.iter().copied());
            self.work.enqueue(delta.load.iter().copied());
            log::debug!(
                "stream: center {center:?}; queued {} chunks ({} resident)",
                delta.load.len(),
                self.chunks.len()
            );
        }
    }

    /// Draws every resident chunk with its own data bind group.
    ///
    /// Meshes come out of the shared arena; a draw binds its two page ranges
    /// and no per-chunk mesh buffer exists.
    pub fn draw(
        &self,
        pass: &mut RenderPass<'_>,
        pipeline: &ChunkPipeline,
        camera: &BindGroup,
        atlas: &BindGroup,
    ) {
        for resources in self.chunks.values() {
            let Some(mesh) = &resources.mesh else {
                continue;
            };
            pipeline.draw_pooled(pass, mesh, camera, atlas, &resources.lighting.bind_group);
        }
    }

    /// A snapshot of the mesh pool: pages, live/free bytes and fragmentation.
    pub fn pool_stats(&self) -> mesh_pool::ArenaStats {
        self.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stats()
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        self.work.close();
        // Drop the receiver before joining so a worker blocked on a full
        // result channel observes the hang-up instead of deadlocking.
        self.receiver.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        if let Some(uploader) = self.uploader.take() {
            let _ = uploader.join();
        }
    }
}

/// Uploads finished CPU content and forwards the GPU resources to the frame
/// thread. Runs off the frame thread on purpose: page creation and the
/// remaining per-chunk light buffers can still block on the driver.
#[allow(clippy::too_many_arguments)]
fn uploader_loop(
    device: Device,
    data_layout: BindGroupLayout,
    pool: Arc<Mutex<MeshPool>>,
    receiver: Receiver<Job>,
    sender: SyncSender<UploadedJob>,
) {
    loop {
        let wait_started = Instant::now();
        let Ok(job) = receiver.recv() else {
            break;
        };
        // The uploader is the only consumer, so this is the time it spent idle
        // waiting for a generator result: the generation hole between jobs.
        let starved = wait_started.elapsed();
        let Job {
            position,
            content,
            generated,
        } = job;
        let Some(content) = content else {
            if sender.send((position, None)).is_err() {
                break;
            }
            continue;
        };
        let resources = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare_upload(
                &device,
                &data_layout,
                &pool,
                position,
                content,
                generated,
                starved,
            )
        }))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            log::error!("stream: {position:?} preparation panicked: {message}");
            None
        });
        if sender.send((position, resources)).is_err() {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_upload(
    device: &Device,
    data_layout: &BindGroupLayout,
    pool: &Arc<Mutex<MeshPool>>,
    position: ChunkPos,
    content: ChunkContent,
    generated: Duration,
    starved: Duration,
) -> Option<PendingUpload> {
    let started = Instant::now();
    let mesh = reserve_mesh(pool, &content.mesh);
    let allocate = started.elapsed();
    let lighting_started = Instant::now();
    let lighting = chunk_lighting::upload(
        device,
        data_layout,
        &content.light,
        &content.probes,
        chunk_origin(position),
    );
    log::trace!(
        "stream: prepared {position:?}: {} quads, {} mesh B; generated {:.2} ms, starved {:.2} ms, alloc {:.2} ms, lighting {:.2} ms",
        content.mesh.quads,
        content.mesh.byte_len(),
        generated.as_secs_f64() * 1000.0,
        starved.as_secs_f64() * 1000.0,
        allocate.as_secs_f64() * 1000.0,
        lighting_started.elapsed().as_secs_f64() * 1000.0,
    );
    Some(PendingUpload {
        position,
        mesh,
        vertices: content.mesh.vertices,
        indices: content.mesh.indices,
        lighting,
    })
}

/// Reserves a pool range for `mesh`; the frame thread writes the bytes.
///
/// The pool lock covers the allocation only, and the range stays exclusively
/// owned until the frame thread frees it.
fn reserve_mesh(
    pool: &Arc<Mutex<MeshPool>>,
    mesh: &crate::chunk_content::ChunkMeshData,
) -> Option<MeshAllocation> {
    if mesh.indices.is_empty() {
        return None;
    }
    let vertex_count =
        u32::try_from(mesh.vertices.len() / MeshFormat::PACKED_8B.vertex_stride() as usize)
            .expect("a chunk mesh has at most u32::MAX vertices");
    let index_count =
        u32::try_from(mesh.indices.len()).expect("a chunk mesh has at most u32 indices");
    Some(
        pool.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .allocate(vertex_count, index_count),
    )
}

fn worker_loop(work: &WorkQueue, sender: &SyncSender<Job>) {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    while let Some((position, queued)) = work.take() {
        let queued_for = queued.elapsed();
        let started = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fill_stream_volume(position, &mut volume);
            let apron = stream_apron(position);
            chunk_content::build_with_apron(&volume, position, &apron)
        }));
        let generated = started.elapsed();
        let content = match result {
            Ok(content) => Some(content),
            Err(payload) => {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_owned());
                // Keep the pool alive: one bad chunk must not starve the ring.
                log::error!("stream: {position:?} panicked: {message}");
                None
            }
        };
        log::trace!(
            "stream: generated {position:?} in {:.2} ms after waiting {:.2} ms",
            generated.as_secs_f64() * 1000.0,
            queued_for.as_secs_f64() * 1000.0,
        );
        if sender
            .send(Job {
                position,
                content,
                generated,
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_discards_queued_jobs() {
        let queue = WorkQueue::new();
        queue.enqueue([ChunkPos::new(1, 0, 1)]);
        queue.close();
        assert!(queue.take().is_none());
    }

    #[test]
    fn ring_has_256_unique_chunks() {
        let ring = ring_chunks(ChunkPos::new(3, 0, -2));
        assert_eq!(ring.len(), RING_CHUNKS);
        let unique: HashSet<ChunkPos> = ring.iter().copied().collect();
        assert_eq!(unique.len(), RING_CHUNKS);
        assert!(ring.iter().all(|chunk| chunk.y == 0));
        assert!(ring.iter().all(|chunk| {
            (-RING_HALF..RING_HALF).contains(&(chunk.x - 3))
                && (-RING_HALF..RING_HALF).contains(&(chunk.z + 2))
        }));
    }

    #[test]
    fn ring_is_nearest_first() {
        let center = ChunkPos::new(0, 0, 0);
        let ring = ring_chunks(center);
        let distances: Vec<i64> = ring
            .iter()
            .map(|chunk| {
                i64::from(chunk.x) * i64::from(chunk.x) + i64::from(chunk.z) * i64::from(chunk.z)
            })
            .collect();
        assert!(distances.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn ring_delta_loads_and_evicts() {
        let first = ring_chunks(ChunkPos::new(0, 0, 0));
        let loaded: HashSet<ChunkPos> = first.iter().copied().collect();
        let delta = ring_delta(
            ChunkPos::new(1, 0, 0),
            loaded.iter().copied(),
            std::iter::empty(),
        );
        assert_eq!(delta.load.len(), RING_HALF as usize * 2);
        assert_eq!(delta.evict.len(), RING_HALF as usize * 2);
        assert!(delta.load.iter().all(|chunk| chunk.x == 1 + RING_HALF - 1));
        assert!(delta.evict.iter().all(|chunk| chunk.x == -RING_HALF));
    }

    #[test]
    fn stream_volume_is_deterministic() {
        let position = ChunkPos::new(-2, 0, 5);
        let first = stream_volume(position);
        let second = stream_volume(position);
        assert_eq!(first, second);
        assert_ne!(first, stream_volume(ChunkPos::new(-2, 0, 6)));
    }

    #[test]
    fn neighbouring_columns_see_the_same_surface() {
        for x in [-113, -1, 0, 127, 128] {
            assert_eq!(terrain_height(x, 37), terrain_height(x, 37));
        }
        let left = terrain_height(127, 0);
        let right = terrain_height(128, 0);
        assert!(left.abs_diff(right) <= 4, "terrain seam jumps at x=128");
    }

    #[test]
    fn stream_volume_has_a_surface_and_no_floating_blocks() {
        let volume = stream_volume(ChunkPos::new(0, 0, 0));
        let mut solid = 0usize;
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let surface = terrain_height(x as i32, z as i32);
                let mut below_air = false;
                for y in 0..CHUNK_DIM {
                    let index = x
                        | (y << voxel_core::constants::CHUNK_BITS)
                        | (z << (2 * voxel_core::constants::CHUNK_BITS));
                    let material = volume[index];
                    if y < surface {
                        if below_air {
                            panic!("air below solid at {x},{y},{z}");
                        }
                        assert_ne!(material, 0, "hole at {x},{y},{z}");
                        solid += 1;
                    } else {
                        if material != 0 {
                            panic!("solid above the surface at {x},{y},{z}");
                        }
                        below_air = true;
                    }
                }
            }
        }
        assert!(solid > 0);
    }

    #[test]
    fn center_rounds_down_to_the_chunk_edge() {
        let edge = CHUNK_DIM as f32 * VOXEL_SIZE;
        assert_eq!(center_for([0.0, 3.0, 0.0, 1.0]), ChunkPos::new(0, 0, 0));
        assert_eq!(
            center_for([edge, 3.0, -edge - 0.01, 1.0]),
            ChunkPos::new(1, 0, -2)
        );
    }
}
