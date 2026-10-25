//! wgpu storage buffers behind the storage tier's [`GpuSink`] trait.
//!
//! The storage tier owns the layout and the dirty-range bookkeeping; this type
//! is deliberately dumb: it owns two storage buffers and copies word ranges into
//! them through the queue.

use bytemuck::cast_slice;
use voxel_storage::gpu::GpuSink;

/// Binding group the SVDAG shader unit expects (`svdag.wgsl`).
pub const SVDAG_BIND_GROUP: u32 = 0;

/// Bytes in one `u32` word of the GPU tables.
const WORD: u64 = 4;

/// GPU-resident node pool, chunk table, block tables and edit-overlay pool.
///
/// Implements [`GpuSink`], so it plugs directly into
/// [`voxel_storage::gpu::GpuStore::flush`]. All four buffers carry
/// `COPY_SRC` in addition to `STORAGE | COPY_DST`; that costs nothing at
/// runtime and lets tests and debug tooling read the tables back.
pub struct WgpuSink {
    device: wgpu::Device,
    queue: wgpu::Queue,
    nodes: wgpu::Buffer,
    table: wgpu::Buffer,
    overlays: wgpu::Buffer,
    block_tables: wgpu::Buffer,
    node_words: usize,
    table_words: usize,
    overlay_words: usize,
    block_table_words: usize,
}

impl WgpuSink {
    /// Creates empty tables, ready for the first
    /// [`voxel_storage::gpu::GpuStore::flush`].
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            nodes: storage_buffer(device, "voxel-svdag-nodes", 1),
            table: storage_buffer(device, "voxel-chunk-table", 1),
            overlays: storage_buffer(device, "voxel-overlay-pool", 1),
            block_tables: storage_buffer(device, "voxel-block-tables", 1),
            node_words: 1,
            table_words: 1,
            overlay_words: 1,
            block_table_words: 1,
        }
    }

    /// The node-pool buffer (`@group(0) @binding(0)` in `svdag.wgsl`).
    pub fn nodes(&self) -> &wgpu::Buffer {
        &self.nodes
    }

    /// The chunk-table buffer (`@group(0) @binding(1)` in `svdag.wgsl`).
    pub fn table(&self) -> &wgpu::Buffer {
        &self.table
    }

    /// The block-overlay buffer (`@group(0) @binding(2)` in `svdag.wgsl`).
    pub fn overlays(&self) -> &wgpu::Buffer {
        &self.overlays
    }

    /// The per-chunk block-table buffer (`@group(0) @binding(3)` in `svdag.wgsl`).
    pub fn block_tables(&self) -> &wgpu::Buffer {
        &self.block_tables
    }

    /// Capacity of the node pool, in words.
    pub fn node_words(&self) -> usize {
        self.node_words
    }

    /// Capacity of the chunk table, in words.
    pub fn table_words(&self) -> usize {
        self.table_words
    }

    /// Capacity of the block-overlay pool, in words.
    pub fn overlay_words(&self) -> usize {
        self.overlay_words
    }

    /// Capacity of the block-table pool, in words.
    pub fn block_table_words(&self) -> usize {
        self.block_table_words
    }

    /// Bind-group layout matching `svdag.wgsl` (group 0).
    pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        storage_bind_group_layout(device, "voxel-svdag-layout", 4)
    }

    /// Bind group for the current buffers, to be bound at [`SVDAG_BIND_GROUP`].
    ///
    /// Recreate it after a resize: a resize installs a new buffer.
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel-svdag-bind-group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.nodes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.table.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.overlays.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.block_tables.as_entire_binding(),
                },
            ],
        })
    }

    fn resize(buffer: &mut wgpu::Buffer, words: usize, device: &wgpu::Device, label: &str) {
        *buffer = storage_buffer(device, label, words);
    }
}

impl GpuSink for WgpuSink {
    fn resize_nodes(&mut self, words: usize) {
        let words = words.max(1);
        if words != self.node_words {
            Self::resize(&mut self.nodes, words, &self.device, "voxel-svdag-nodes");
            self.node_words = words;
        }
    }

    fn write_nodes(&mut self, word_off: usize, data: &[u32]) {
        write_words(&self.queue, &self.nodes, word_off, data, self.node_words);
    }

    fn resize_table(&mut self, words: usize) {
        let words = words.max(1);
        if words != self.table_words {
            Self::resize(&mut self.table, words, &self.device, "voxel-chunk-table");
            self.table_words = words;
        }
    }

    fn write_table(&mut self, word_off: usize, data: &[u32]) {
        write_words(&self.queue, &self.table, word_off, data, self.table_words);
    }

    fn resize_overlays(&mut self, words: usize) {
        let words = words.max(1);
        if words != self.overlay_words {
            Self::resize(
                &mut self.overlays,
                words,
                &self.device,
                "voxel-overlay-pool",
            );
            self.overlay_words = words;
        }
    }

    fn write_overlays(&mut self, word_off: usize, data: &[u32]) {
        write_words(
            &self.queue,
            &self.overlays,
            word_off,
            data,
            self.overlay_words,
        );
    }

    fn resize_block_tables(&mut self, words: usize) {
        let words = words.max(1);
        if words != self.block_table_words {
            Self::resize(
                &mut self.block_tables,
                words,
                &self.device,
                "voxel-block-tables",
            );
            self.block_table_words = words;
        }
    }

    fn write_block_tables(&mut self, word_off: usize, data: &[u32]) {
        write_words(
            &self.queue,
            &self.block_tables,
            word_off,
            data,
            self.block_table_words,
        );
    }
}

fn write_words(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    word_off: usize,
    data: &[u32],
    capacity: usize,
) {
    if data.is_empty() {
        return;
    }
    assert!(
        word_off + data.len() <= capacity,
        "GPU table write out of bounds: {word_off}..{} > {capacity}",
        word_off + data.len(),
    );
    queue.write_buffer(buffer, word_off as u64 * WORD, cast_slice(data));
}

pub(crate) fn storage_buffer(device: &wgpu::Device, label: &str, words: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        // A zero-sized buffer is invalid in wgpu. One spare word is harmless:
        // the CPU tables are never empty when they are bound.
        size: words.max(1) as u64 * WORD,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

pub(crate) fn storage_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
    bindings: u32,
) -> wgpu::BindGroupLayout {
    let entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let entries: Vec<_> = (0..bindings).map(entry).collect();

    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}
