//! Tier 2: resident chunks. Mining/placing is O(1) in the overlay, and the same
//! override goes straight to the GPU overlay so the next frame sees it. The
//! compressed DAG is rebuilt by [`World::tick`] when a
//! [`ConsolidationPolicy`] trigger fires, or explicitly on save/LOD events.
use crate::apron::Apron;
use crate::dag::Svdag;
use crate::disk::RegionStore;
use crate::gpu::{GpuSink, GpuStore};
use crate::hash::{FxMap, FxSet};
use crate::overlay::Overlay;
use crate::{CHUNK_VOLUME, ChunkPos, local_xyz, split_voxel};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use voxel_block::MaterialId;

/// Fill a zeroed dense chunk (index = x + y*64 + z*4096) with raw [`MaterialId`]s for `pos`.
pub type Generator = Box<dyn FnMut(ChunkPos, &mut [u16]) + Send>;

/// When to fold overlays into the DAG (ADR §10.2).
///
/// The overlay is the interactive path; folding it is maintenance. Folding
/// eagerly costs the frame that does it, folding never costs memory and makes
/// the GPU table grow. The policy is deliberately trigger-based:
///
/// * **size** — one chunk at [`Self::size_trigger`] entries is folded even when
///   the frame budget is otherwise idle;
/// * **idle** — after [`Self::idle_trigger`] with no edits, drain everything;
/// * **budget** — stop starting a new chunk once [`Self::frame_budget`] is
///   spent. A single fold may overshoot by one chunk's edit batch; the
///   guarantee is "do not start more work", not "finish under the budget";
/// * **event** — call [`World::consolidate`] directly on save, unload, or an
///   LOD transition when the caller needs the DAG to be authoritative.
#[derive(Clone, Debug)]
pub struct ConsolidationPolicy {
    /// Fold a chunk once its overlay reaches this many entries.
    pub size_trigger: usize,
    /// Drain all pending overlays after this long with no edits.
    pub idle_trigger: Duration,
    /// Maximum wall time spent folding per [`World::tick`].
    pub frame_budget: Duration,
    /// Safety cap on chunks folded per tick, even when the budget allows more.
    pub max_chunks: usize,
}

impl Default for ConsolidationPolicy {
    fn default() -> Self {
        Self {
            size_trigger: 4_096,
            idle_trigger: Duration::from_secs(30),
            frame_budget: Duration::from_millis(2),
            max_chunks: 8,
        }
    }
}

pub struct Chunk {
    pub dag: Arc<Svdag>,
    pub overlay: Overlay,
    unsaved: bool,
}

impl Chunk {
    /// The material at local index `local`: overlay first, then the DAG.
    ///
    /// [`World::get`] and the apron both use this so a border read cannot
    /// disagree with a direct voxel read.
    #[inline]
    #[must_use]
    pub fn material(&self, local: u32) -> u16 {
        self.overlay.get(local).unwrap_or_else(|| {
            let (x, y, z) = local_xyz(local);
            self.dag.get(x, y, z)
        })
    }
}

pub struct Consolidated {
    pub chunks: usize,
    /// pass to `World::reclaim` after GPU frames submitted before this flush have finished
    pub epoch: u64,
    /// dirty chunks still holding at least one overlay entry
    pub remaining: usize,
}

pub struct World<S: GpuSink> {
    chunks: FxMap<ChunkPos, Chunk>,
    edited: FxSet<ChunkPos>,
    disk: RegionStore,
    gpu: GpuStore,
    sink: S,
    generator: Generator,
    scratch: Vec<u16>,
    policy: ConsolidationPolicy,
    last_edit: Instant,
    oversized: bool,
}

impl<S: GpuSink> World<S> {
    pub fn new(dir: impl Into<PathBuf>, generator: Generator, sink: S) -> io::Result<Self> {
        Ok(Self {
            chunks: FxMap::default(),
            edited: FxSet::default(),
            disk: RegionStore::open(dir)?,
            gpu: GpuStore::new(),
            sink,
            generator,
            scratch: vec![0; CHUNK_VOLUME],
            policy: ConsolidationPolicy::default(),
            last_edit: Instant::now(),
            oversized: false,
        })
    }

    /// Load from disk (dag + overlay snapshot) or generate; registers base DAG with the GPU tier.
    pub fn load(&mut self, p: ChunkPos) -> io::Result<()> {
        if self.chunks.contains_key(&p) {
            return Ok(());
        }
        let (dag, overlay) = match self.disk.load(p)? {
            Some(t) => t,
            None => {
                self.scratch.fill(0);
                (self.generator)(p, &mut self.scratch);
                (Svdag::from_dense(&self.scratch), Overlay::default())
            }
        };
        self.gpu.set_chunk(p, &dag);
        if !overlay.is_empty() {
            self.gpu.set_chunk_overlay(p, overlay.iter());
            self.edited.insert(p);
            if overlay.len() >= self.policy.size_trigger {
                self.oversized = true;
            }
        }
        self.chunks.insert(
            p,
            Chunk {
                dag: Arc::new(dag),
                overlay,
                unsaved: false,
            },
        );
        Ok(())
    }

    /// Overlay first, then DAG. None if the chunk isn't resident.
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<MaterialId> {
        let (p, li) = split_voxel(x, y, z);
        let c = self.chunks.get(&p)?;
        Some(MaterialId::from_raw(c.material(li)))
    }

    /// The six 1-voxel face planes around `p` (M2-01).
    ///
    /// Each plane is [`Apron::PLANE`] raw [`MaterialId`]s read overlay-first
    /// from the chunk that shares that face. Every face plane therefore costs
    /// one neighbour lookup and a tree walk, never a dense padded copy.
    ///
    /// A neighbour that is not resident reads as [`AIR`](crate::AIR). Load the
    /// six face neighbours first when a border must be exact. AO samples at an
    /// edge or corner belong to an edge or corner neighbour and also read as
    /// [`AIR`](crate::AIR); see [`crate::apron`].
    #[must_use]
    pub fn apron(&self, p: ChunkPos) -> Apron {
        Apron::gather(self, p)
    }

    /// Fills `out` with the six face planes around `p`, reusing its buffer.
    ///
    /// Same data as [`Self::apron`], but a mesher that visits many chunks per
    /// frame can keep one [`Apron`] and skip a `6 x SIDE²` allocation per
    /// chunk.
    pub fn apron_into(&self, p: ChunkPos, out: &mut Apron) {
        Apron::gather_into(self, p, out);
    }

    /// O(1): overlay insert (or removal when the edit reverts to the DAG value).
    ///
    /// The GPU overlay is updated in the same call, so the edit is visible to the
    /// next frame after [`Self::flush_gpu`] — no consolidation needed.
    pub fn set(&mut self, x: i32, y: i32, z: i32, v: MaterialId) -> io::Result<()> {
        let (p, li) = split_voxel(x, y, z);
        self.load(p)?;
        let (revert, len) = {
            let c = self.chunks.get_mut(&p).expect("load inserts the chunk");
            let (lx, ly, lz) = local_xyz(li);
            let revert = v.as_raw() == c.dag.get(lx, ly, lz);
            if revert {
                c.overlay.remove(li);
            } else {
                c.overlay.set(li, v.as_raw());
            }
            c.unsaved = true;
            (revert, c.overlay.len())
        };
        if revert {
            self.gpu.clear_overlay(p, li);
        } else {
            self.gpu.set_overlay(p, li, v.as_raw());
        }
        self.edited.insert(p);
        if len >= self.policy.size_trigger {
            self.oversized = true;
        }
        self.last_edit = Instant::now();
        Ok(())
    }

    fn persist(&mut self, p: ChunkPos) -> io::Result<()> {
        let Some(c) = self.chunks.get_mut(&p) else {
            return Ok(());
        };
        if !c.unsaved {
            return Ok(());
        }
        if !self.disk.has_dag(p)? {
            self.disk.put_dag(p, &c.dag)?;
        }
        self.disk.put_diff(p, &c.overlay)?;
        c.unsaved = false;
        Ok(())
    }

    /// Cheap WAL-style durability: write overlay snapshots, no DAG rebuild.
    pub fn flush_edits(&mut self) -> io::Result<()> {
        let ps: Vec<ChunkPos> = self.edited.iter().copied().collect();
        for p in ps {
            self.persist(p)?;
            if self.chunks.get(&p).is_some_and(|c| c.overlay.is_empty()) {
                self.edited.remove(&p);
            }
        }
        self.refresh_oversized();
        self.disk.sync()
    }

    /// Chunks that still hold at least one overlay entry.
    ///
    /// [`Self::dirty_chunks`] also counts entries whose overlay was fully
    /// reverted and is waiting for [`Self::flush_edits`] to persist the empty
    /// diff, so use this for consolidation decisions.
    #[must_use]
    pub fn pending_chunks(&self) -> usize {
        self.edited
            .iter()
            .filter(|p| {
                self.chunks
                    .get(p)
                    .is_some_and(|chunk| !chunk.overlay.is_empty())
            })
            .count()
    }

    /// The current consolidation policy.
    pub fn policy(&self) -> &ConsolidationPolicy {
        &self.policy
    }

    /// Replace the policy and re-evaluate the size trigger.
    pub fn set_policy(&mut self, policy: ConsolidationPolicy) {
        self.policy = policy;
        self.refresh_oversized();
    }

    /// Frame hook: fold overlays when the policy says so, within the frame budget.
    ///
    /// Returns `None` when no trigger fired. Call this once per frame; it is
    /// cheap when there is nothing to do. The caller must still call
    /// [`Self::reclaim`] with the returned epoch after the GPU work submitted
    /// before the following flush has completed.
    pub fn tick(&mut self, now: Instant) -> io::Result<Option<Consolidated>> {
        if self.dirty_chunks() == 0 || self.policy.max_chunks == 0 {
            return Ok(None);
        }
        let idle = now.saturating_duration_since(self.last_edit) >= self.policy.idle_trigger;
        if !self.oversized && !idle {
            return Ok(None);
        }
        // A reverted edit can leave an "edited" slot with no entries.
        if self.pending_chunks() == 0 {
            self.oversized = false;
            return Ok(None);
        }
        self.consolidate_budgeted(self.policy.max_chunks, self.policy.frame_budget)
            .map(Some)
    }

    /// Event trigger: fold up to `max_chunks` overlays now.
    ///
    /// Use this on save, unload, or an LOD transition when the caller needs the
    /// DAG to be authoritative. For the per-frame path use [`Self::tick`].
    pub fn consolidate(&mut self, max_chunks: usize) -> io::Result<Consolidated> {
        self.consolidate_budgeted(max_chunks, Duration::MAX)
    }

    /// Fold up to `max_chunks` overlays (largest first) into new DAGs, persist them, and push
    /// the whole batch to the GPU tier in one COW pass + one upload flush.
    ///
    /// Stops starting a new chunk once `budget` has elapsed, but always makes
    /// progress on at least one chunk when there is work.
    pub fn consolidate_budgeted(
        &mut self,
        max_chunks: usize,
        budget: Duration,
    ) -> io::Result<Consolidated> {
        let started = Instant::now();
        let mut cand: Vec<(usize, ChunkPos)> = self
            .edited
            .iter()
            .filter_map(|p| {
                let n = self.chunks.get(p)?.overlay.len();
                (n > 0).then_some((n, *p))
            })
            .collect();
        cand.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
        cand.truncate(max_chunks);

        let mut done = 0usize;
        for (_, p) in cand {
            if done > 0 && started.elapsed() >= budget {
                break;
            }
            let c = self
                .chunks
                .get_mut(&p)
                .expect("consolidation candidate is resident");
            // Persist the folded DAG before touching either in-memory tier: on
            // an I/O error the CPU keeps the old DAG + overlay and the GPU is
            // untouched, so the two cannot diverge.
            let new = Arc::new(c.dag.apply_edits(c.overlay.iter())); // CPU tier
            self.disk.put_dag(p, &new)?; // supersedes the diff record
            // Clear the GPU overlay first: the folded DAG writes land in the same flush,
            // so no frame sees a hole between the two updates.
            self.gpu.clear_chunk_overlay(p);
            self.gpu.apply_edits(p, c.overlay.iter()); // GPU tier: edit changed paths in place
            c.dag = new;
            c.overlay.clear();
            c.unsaved = false;
            self.edited.remove(&p);
            done += 1;
        }
        let epoch = self.gpu.end_batch();
        self.gpu.flush(&mut self.sink);
        self.disk.sync()?;
        self.refresh_oversized();
        Ok(Consolidated {
            chunks: done,
            epoch,
            remaining: self.pending_chunks(),
        })
    }

    fn refresh_oversized(&mut self) {
        let threshold = self.policy.size_trigger;
        self.oversized = self.edited.iter().any(|p| {
            self.chunks
                .get(p)
                .is_some_and(|chunk| chunk.overlay.len() >= threshold)
        });
    }

    /// Push pending loads/evictions to the GPU without consolidating edits.
    pub fn flush_gpu(&mut self) {
        self.gpu.flush(&mut self.sink)
    }

    pub fn reclaim(&mut self, epoch: u64) {
        self.gpu.reclaim(epoch)
    }

    /// Drop a chunk from RAM + GPU. Edits are persisted as an overlay snapshot (no rebuild).
    pub fn evict(&mut self, p: ChunkPos) -> io::Result<()> {
        self.persist(p)?;
        if self.chunks.remove(&p).is_some() {
            self.edited.remove(&p);
            self.gpu.remove_chunk(p);
            self.refresh_oversized();
        }
        Ok(())
    }

    pub fn chunk(&self, p: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&p)
    }
    /// Chunks with an unsaved overlay snapshot.
    ///
    /// Includes fully reverted chunks that still need [`Self::flush_edits`] to
    /// write the empty diff; use [`Self::pending_chunks`] for consolidation.
    #[must_use]
    pub fn dirty_chunks(&self) -> usize {
        self.edited.len()
    }
    pub fn gpu(&self) -> &GpuStore {
        &self.gpu
    }
    pub fn sink(&self) -> &S {
        &self.sink
    }
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
    pub fn disk_mut(&mut self) -> &mut RegionStore {
        &mut self.disk
    }
}
