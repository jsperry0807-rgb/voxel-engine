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
//! arena can move uploads back to the frame thread once allocations are cheap.
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
use crate::chunk_mesh::ChunkMesh;
use crate::chunk_render::ChunkPipeline;

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

/// One chunk's uploaded resources.
pub struct ChunkResources {
    /// The chunk's GPU mesh, or `None` for an empty volume.
    pub mesh: Option<ChunkMesh>,
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
    /// Chunks installed by this pump.
    pub installed: usize,
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

/// A generator result: `None` when generation panicked.
type Job = (ChunkPos, Option<ChunkContent>);
/// An uploader result: `None` when generation or upload failed. The failing
/// stage logs the reason; the frame thread only accounts for the hole.
type UploadedJob = (ChunkPos, Option<ChunkResources>);

/// Shared work queue for the generator threads.
struct WorkQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct QueueState {
    jobs: VecDeque<ChunkPos>,
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
        state.jobs.extend(jobs);
        self.ready.notify_all();
    }

    fn take(&self) -> Option<ChunkPos> {
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
        self.lock().closed = true;
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
/// generator/mesher workers, one GPU uploader, and the frame thread, which
/// only installs finished resources into the resident map. Buffer creation and
/// mapped writes can occasionally block for several milliseconds on this
/// driver; doing them on the frame thread would turn any such allocation into
/// a render hitch.
pub struct Streaming {
    chunks: HashMap<ChunkPos, ChunkResources>,
    requested: HashSet<ChunkPos>,
    desired: HashSet<ChunkPos>,
    center: Option<ChunkPos>,
    work: Arc<WorkQueue>,
    receiver: Option<Receiver<UploadedJob>>,
    workers: Vec<JoinHandle<()>>,
    uploader: Option<JoinHandle<()>>,
    max_pump: Duration,
    hitches: u32,
    complete_logged: bool,
    uploader_down: bool,
}

impl Streaming {
    /// Starts the generator pool. Nothing is queued until the first
    /// [`Self::pump`] supplies a camera center.
    pub fn new(device: Device, data_layout: BindGroupLayout) -> Self {
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
            .spawn(move || uploader_loop(device, data_layout, content_receiver, upload_sender))
            .expect("spawn chunk uploader");
        log::debug!("stream: {worker_count} generator workers, ring {RING_CHUNKS} chunks");
        Self {
            chunks: HashMap::with_capacity(RING_CHUNKS),
            requested: HashSet::with_capacity(RING_CHUNKS),
            desired: HashSet::with_capacity(RING_CHUNKS),
            center: None,
            work,
            receiver: Some(upload_receiver),
            workers,
            uploader: Some(uploader),
            max_pump: Duration::ZERO,
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

    /// Re-centers the ring if needed, then installs ready resources within
    /// `budget`.
    ///
    /// The budget is checked before every install. Installing a finished
    /// resource is a map insert, so a pump normally finishes well inside the
    /// budget. `max_pump`/`hitches` cover this pump only (including the
    /// recenter that runs before the loop); they do not include draw, submit or
    /// the off-thread upload.
    pub fn pump(&mut self, center: ChunkPos, budget: Duration) -> PumpStats {
        let started = Instant::now();
        if self.center != Some(center) {
            self.recenter(center);
        }

        let mut installed = 0usize;
        while started.elapsed() < budget {
            let Some(receiver) = &self.receiver else {
                break;
            };
            match receiver.try_recv() {
                Ok((position, resources)) => {
                    self.requested.remove(&position);
                    if !self.desired.contains(&position) {
                        continue;
                    }
                    match resources {
                        Some(resources) => {
                            self.chunks.insert(position, resources);
                            installed += 1;
                            log::trace!("stream: installed {position:?}");
                        }
                        None => {
                            log::error!(
                                "stream: {position:?} failed in generation or upload (see the worker/uploader log)"
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
            installed,
            elapsed,
            max_elapsed: self.max_pump,
            hitches: self.hitches,
        };
        if !self.complete_logged && stats.loaded == RING_CHUNKS {
            self.complete_logged = true;
            log::info!(
                "stream: {RING_CHUNKS} loaded, {} pending; max pump {:.2} ms, {} hitches over {:.0} ms",
                stats.pending,
                self.max_pump.as_secs_f64() * 1000.0,
                self.hitches,
                budget.as_secs_f64() * 1000.0
            );
        }
        stats
    }

    fn recenter(&mut self, center: ChunkPos) {
        let delta = ring_delta(
            center,
            self.chunks.keys().copied(),
            self.requested.iter().copied(),
        );
        if !delta.evict.is_empty() {
            self.chunks
                .retain(|position, _| !delta.evict.contains(position));
            for position in &delta.evict {
                self.requested.remove(position);
            }
            log::debug!("stream: evicted {} chunks", delta.evict.len());
        }
        let desired = ring_chunks(center);
        self.desired = desired.iter().copied().collect();
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
            pipeline.draw(pass, mesh, camera, atlas, &resources.lighting.bind_group);
        }
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
/// thread. Runs off the frame thread on purpose: buffer creation and mapped
/// writes can block on the driver for longer than the frame budget.
fn uploader_loop(
    device: Device,
    data_layout: BindGroupLayout,
    receiver: Receiver<Job>,
    sender: SyncSender<UploadedJob>,
) {
    while let Ok((position, content)) = receiver.recv() {
        let Some(content) = content else {
            if sender.send((position, None)).is_err() {
                break;
            }
            continue;
        };
        let resources = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            upload_chunk(&device, &data_layout, position, &content)
        }))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            log::error!("stream: {position:?} upload panicked: {message}");
            None
        });
        if sender.send((position, resources)).is_err() {
            break;
        }
    }
}

fn upload_chunk(
    device: &Device,
    data_layout: &BindGroupLayout,
    position: ChunkPos,
    content: &ChunkContent,
) -> Option<ChunkResources> {
    let started = Instant::now();
    let mesh = ChunkMesh::from_parts(device, &content.mesh.vertices, &content.mesh.indices);
    let lighting = chunk_lighting::upload(
        device,
        data_layout,
        &content.light,
        &content.probes,
        chunk_origin(position),
    );
    log::trace!(
        "stream: uploaded {position:?}: {} quads, {} mesh B in {:.2} ms",
        content.mesh.quads,
        content.mesh.byte_len(),
        started.elapsed().as_secs_f64() * 1000.0,
    );
    Some(ChunkResources { mesh, lighting })
}

fn worker_loop(work: &WorkQueue, sender: &SyncSender<Job>) {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    while let Some(position) = work.take() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fill_stream_volume(position, &mut volume);
            let apron = stream_apron(position);
            chunk_content::build_with_apron(&volume, position, &apron)
        }));
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
        if sender.send((position, content)).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
