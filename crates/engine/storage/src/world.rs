//! Tier 2: resident chunks. Mining/placing only touches the overlay (O(1)).
//! The compressed DAG is rebuilt in batches by `consolidate`, then pushed to the GPU tier.
use crate::dag::Svdag;
use crate::disk::RegionStore;
use crate::gpu::{GpuSink, GpuStore};
use crate::hash::{FxMap, FxSet};
use crate::overlay::Overlay;
use crate::{local_xyz, split_voxel, ChunkPos, CHUNK_VOLUME};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use voxel_block::MaterialId;

/// Fill a zeroed dense chunk (index = x + y*64 + z*4096) with raw [`MaterialId`]s for `pos`.
pub type Generator = Box<dyn FnMut(ChunkPos, &mut [u16]) + Send>;

pub struct Chunk {
    pub dag: Arc<Svdag>,
    pub overlay: Overlay,
    unsaved: bool,
}

pub struct Consolidated {
    pub chunks: usize,
    /// pass to `World::reclaim` after GPU frames submitted before this flush have finished
    pub epoch: u64,
}

pub struct World<S: GpuSink> {
    chunks: FxMap<ChunkPos, Chunk>,
    edited: FxSet<ChunkPos>,
    disk: RegionStore,
    gpu: GpuStore,
    sink: S,
    generator: Generator,
    scratch: Vec<u16>,
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
            self.edited.insert(p);
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
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<MaterialId> {
        let (p, li) = split_voxel(x, y, z);
        let c = self.chunks.get(&p)?;
        Some(MaterialId::from_raw(c.overlay.get(li).unwrap_or_else(|| {
            let (lx, ly, lz) = local_xyz(li);
            c.dag.get(lx, ly, lz)
        })))
    }

    /// O(1): overlay insert (or removal when the edit reverts to the DAG value).
    pub fn set(&mut self, x: i32, y: i32, z: i32, v: MaterialId) -> io::Result<()> {
        let (p, li) = split_voxel(x, y, z);
        self.load(p)?;
        let c = self.chunks.get_mut(&p).unwrap();
        let (lx, ly, lz) = local_xyz(li);
        if v.as_raw() == c.dag.get(lx, ly, lz) {
            c.overlay.remove(li);
        } else {
            c.overlay.set(li, v.as_raw());
        }
        c.unsaved = true;
        self.edited.insert(p);
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
            if self.chunks[&p].overlay.is_empty() {
                self.edited.remove(&p);
            }
        }
        self.disk.sync()
    }

    /// Fold up to `max_chunks` overlays (largest first) into new DAGs, persist them, and push
    /// the whole batch to the GPU tier in one COW pass + one upload flush.
    pub fn consolidate(&mut self, max_chunks: usize) -> io::Result<Consolidated> {
        let mut cand: Vec<(usize, ChunkPos)> = self
            .edited
            .iter()
            .filter_map(|p| {
                let n = self.chunks[p].overlay.len();
                (n > 0).then_some((n, *p))
            })
            .collect();
        cand.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        cand.truncate(max_chunks);

        let mut done = 0usize;
        for (_, p) in cand {
            let c = self.chunks.get_mut(&p).unwrap();
            self.gpu.apply_edits(p, c.overlay.iter()); // GPU tier: edit changed paths in place
            let new = Arc::new(c.dag.apply_edits(c.overlay.iter())); // CPU tier: same edits
            self.disk.put_dag(p, &new)?; // supersedes the diff record
            c.dag = new;
            c.overlay.clear();
            c.unsaved = false;
            self.edited.remove(&p);
            done += 1;
        }
        let epoch = self.gpu.end_batch();
        self.gpu.flush(&mut self.sink);
        self.disk.sync()?;
        Ok(Consolidated {
            chunks: done,
            epoch,
        })
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
            self.gpu.set_chunk(p, &Svdag::empty());
        }
        Ok(())
    }

    pub fn chunk(&self, p: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&p)
    }
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
