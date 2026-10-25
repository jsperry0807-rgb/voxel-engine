//! One place to own the GPU tables a frame reads.
//!
//! [`GpuWorld`] is a thin composition, not a renderer: it keeps the storage
//! tier's [`GpuStore`], the wgpu [`WgpuSink`] that receives its dirty ranges,
//! and the immutable [`MaterialBuffers`] side by side. Game code edits the
//! store and calls [`GpuWorld::flush`]; the renderer binds the resulting
//! buffers with [`GpuWorld::terrain_bind_group`] and
//! [`GpuWorld::material_bind_group`].

use voxel_block::BlockRegistry;
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::GpuStore;

use crate::materials::{MaterialBuffers, material_bind_group_layout};
use crate::sink::{SVDAG_BIND_GROUP, WgpuSink};

/// The GPU-side world: storage tables, their upload sink, and material tables.
pub struct GpuWorld {
    device: wgpu::Device,
    store: GpuStore,
    sink: WgpuSink,
    materials: MaterialBuffers,
    material_layout: wgpu::BindGroupLayout,
    terrain_layout: wgpu::BindGroupLayout,
}

impl GpuWorld {
    /// Builds the GPU tables from a frozen block registry.
    ///
    /// The registry is immutable, so this is a startup operation. Rebuilding it
    /// on a registry change means rebuilding the world's material bind group
    /// too.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, registry: &BlockRegistry) -> Self {
        Self {
            device: device.clone(),
            store: GpuStore::new(),
            sink: WgpuSink::new(device, queue),
            materials: MaterialBuffers::upload(device, queue, registry),
            material_layout: material_bind_group_layout(device),
            terrain_layout: WgpuSink::bind_group_layout(device),
        }
    }

    /// Points a chunk at `dag`, copying it into the GPU node pool.
    ///
    /// Empty DAGs clear the root. The chunk stays addressable while it has an
    /// overlay. This is copy-on-write; the previous tree is retired and freed by
    /// [`Self::reclaim`] once the GPU has finished with it.
    pub fn set_chunk(&mut self, pos: ChunkPos, dag: &Svdag) {
        self.store.set_chunk(pos, dag);
    }

    /// Applies `(local voxel index, material)` edits to a chunk's overlay.
    ///
    /// The overlay shadows the DAG in `sample_chunk`, so the edit is visible to
    /// the next frame after [`Self::flush`]. Storage is allocated per 16^3
    /// block: one edit costs one ~8 KiB block slot, not one per chunk. Call
    /// [`Self::clear_chunk_overlay`] after folding the same edits into the DAG
    /// with [`Self::apply_edits`].
    pub fn set_overlay(&mut self, pos: ChunkPos, idx: u32, material: u16) {
        self.store.set_overlay(pos, idx, material);
    }

    /// Removes one overlay entry, falling back to the DAG value.
    pub fn clear_overlay(&mut self, pos: ChunkPos, idx: u32) {
        self.store.clear_overlay(pos, idx);
    }

    /// Installs a whole overlay snapshot, e.g. after loading a chunk from disk.
    pub fn set_chunk_overlay(
        &mut self,
        pos: ChunkPos,
        edits: impl IntoIterator<Item = (u32, u16)>,
    ) {
        self.store.set_chunk_overlay(pos, edits);
    }

    /// Drops a chunk's overlay without folding it into the DAG.
    pub fn clear_chunk_overlay(&mut self, pos: ChunkPos) {
        self.store.clear_chunk_overlay(pos);
    }

    /// Applies `(local voxel index, material)` edits to a chunk in place.
    ///
    /// The cost is proportional to the number of edited paths, not the chunk
    /// volume.
    pub fn apply_edits(&mut self, pos: ChunkPos, edits: impl IntoIterator<Item = (u32, u16)>) {
        self.store.apply_edits(pos, edits);
    }

    /// Uploads every dirty node and table range to the GPU.
    ///
    /// Call once per frame after edits. The sink writes through the queue, so
    /// the upload is visible to the next submitted command buffer.
    pub fn flush(&mut self) {
        self.store.flush(&mut self.sink);
    }

    /// Frees nodes retired at or before `completed`.
    ///
    /// Call with the epoch tag returned by [`Self::end_batch`] after the GPU
    /// work submitted before the following flush has completed.
    pub fn reclaim(&mut self, completed: u64) {
        self.store.reclaim(completed);
    }

    /// Closes an edit batch and returns the epoch tag for [`Self::reclaim`].
    pub fn end_batch(&mut self) -> u64 {
        self.store.end_batch()
    }

    /// The storage tier's GPU replica.
    pub fn store(&self) -> &GpuStore {
        &self.store
    }

    /// The storage tier's GPU replica, for batching and direct edits.
    pub fn store_mut(&mut self) -> &mut GpuStore {
        &mut self.store
    }

    /// The node-pool and chunk-table buffers.
    pub fn sink(&self) -> &WgpuSink {
        &self.sink
    }

    /// The hot-record and texture-set buffers.
    pub fn materials(&self) -> &MaterialBuffers {
        &self.materials
    }

    /// Bind group for the SVDAG tables (group 0).
    ///
    /// Create it after [`Self::flush`]: a flush that grows a table installs a
    /// new buffer, and an older bind group would keep reading the old one.
    pub fn terrain_bind_group(&self) -> wgpu::BindGroup {
        self.sink.bind_group(&self.device, &self.terrain_layout)
    }

    /// Bind group for the material tables (group 1).
    pub fn material_bind_group(&self) -> wgpu::BindGroup {
        self.materials
            .bind_group(&self.device, &self.material_layout)
    }

    /// Bind-group index of the terrain tables: [`SVDAG_BIND_GROUP`].
    pub const TERRAIN_GROUP: u32 = SVDAG_BIND_GROUP;

    /// Bind-group index of the material tables:
    /// [`crate::MATERIAL_BIND_GROUP`].
    pub const MATERIAL_GROUP: u32 = crate::MATERIAL_BIND_GROUP;
}
