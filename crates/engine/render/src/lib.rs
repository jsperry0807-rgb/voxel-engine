//! GPU-side upload and binding layer for the voxel engine.
//!
//! This crate owns the wgpu resources a renderer reads; it does not draw
//! anything. It turns the CPU tiers into GPU-visible storage buffers and
//! publishes the shader contract those buffers are read with:
//!
//! - [`WgpuSink`] implements [`voxel_storage::gpu::GpuSink`] over wgpu storage
//!   buffers. The storage tier's [`voxel_storage::gpu::GpuStore`] drives it:
//!   the store decides *what* bytes to upload, the sink owns the buffers and
//!   the queue writes.
//! - [`MaterialBuffers`] uploads the block registry's runtime tables: the
//!   8-byte [`voxel_block::HotRecord`] array and the deduplicated six-face
//!   texture sets. It mirrors [`voxel_block::BlockRegistry::gpu_hot_words`]
//!   and [`voxel_block::BlockRegistry::gpu_texture_set_words`] word for word.
//! - [`GpuWorld`] wires one [`voxel_storage::gpu::GpuStore`], one [`WgpuSink`]
//!   and one [`MaterialBuffers`] together, so a frame can edit chunks, flush
//!   the dirty ranges, and bind the tables in one place.
//!
//! # Shader contract
//!
//! The WGSL sources are shipped as [`SVDAG_WGSL`] and [`MATERIAL_WGSL`].
//! WGSL has no include mechanism, so a renderer concatenates the two strings
//! into its pipeline module; each file declares the bindings of its own group,
//! so the pieces do not collide:
//!
//! | Group | Binding | Buffer | Rust owner |
//! |---|---|---|---|
//! | 0 | 0 | node pool (`array<u32>`) | [`WgpuSink::nodes`] |
//! | 0 | 1 | chunk table, 5 words per slot | [`WgpuSink::table`] |
//! | 0 | 2 | block overlays, 2 176 words per slot | [`WgpuSink::overlays`] |
//! | 0 | 3 | block tables, 64 words per chunk | [`WgpuSink::block_tables`] |
//! | 1 | 0 | hot records, 2 words per block | [`MaterialBuffers::hot`] |
//! | 1 | 1 | texture sets, 3 words per set | [`MaterialBuffers::texture_sets`] |
//!
//! [`SVDAG_WGSL`] exposes `chunk_slot`, `chunk_root`, `dag_get` and
//! `sample_chunk`; `sample_chunk` checks the 16^3 block overlay before
//! descending the DAG, so an unconsolidated edit is visible to the next frame.
//! [`MATERIAL_WGSL`] exposes the hot-record and per-face texture accessors.
//! [`SVDAG_WGSL`] returns a raw [`voxel_block::MaterialId`] value, which the
//! caller hands to the material accessors.
//!
//! WGSL has no include mechanism, so `svdag.wgsl` repeats the CPU layout
//! constants ([`voxel_storage::dag::ROOT_LEVEL`], [`WgpuSink::table`] slot
//! width, blocks per chunk and the overlay words). The `shader_constants_*`
//! tests parse those declarations and fail on any divergence from
//! `voxel_storage`, including a shader constant that has no Rust owner.
//!
//! # Binding lifetime
//!
//! [`WgpuSink`] reallocates its buffers when the storage tier grows a table. A
//! bind group created before such a resize keeps pointing at the old buffer, so
//! create bind groups *after* the frame's [`GpuWorld::flush`], or recreate them
//! whenever any `*_words` capacity changes.
//!
//! ```
//! use voxel_block::{BlockKind, BlockRegistryBuilder};
//!
//! // Buffer creation needs a wgpu device, so this example stops at the data
//! // the upload consumes.
//! let mut builder = BlockRegistryBuilder::new();
//! builder.register_kind(BlockKind::Stone).unwrap();
//! let registry = builder.freeze().unwrap();
//!
//! assert_eq!(registry.gpu_hot_words().len(), registry.len() * 2);
//! assert_eq!(
//!     registry.gpu_texture_set_words().len(),
//!     registry.texture_sets().len() * 3,
//! );
//! ```

mod materials;
mod sink;
mod world;

pub use materials::{
    HOT_WORDS_PER_BLOCK, MATERIAL_BIND_GROUP, MaterialBuffers, TEXTURE_SET_WORDS,
    material_bind_group_layout,
};
pub use sink::{SVDAG_BIND_GROUP, WgpuSink};
pub use world::GpuWorld;

/// WGSL accessors for the SVDAG node pool and chunk table (binding group 0).
pub const SVDAG_WGSL: &str = include_str!("shaders/svdag.wgsl");

/// WGSL accessors for the hot record table and texture sets (binding group 1).
pub const MATERIAL_WGSL: &str = include_str!("shaders/material.wgsl");
