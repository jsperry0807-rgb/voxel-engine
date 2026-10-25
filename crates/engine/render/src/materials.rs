//! GPU upload for the block registry's runtime tables.
//!
//! The registry already knows how to pack itself
//! ([`voxel_block::BlockRegistry::gpu_hot_words`] and
//! [`voxel_block::BlockRegistry::gpu_texture_set_words`]); this module owns the
//! two buffers those words live in and the bind group the material shader unit
//! reads.

use voxel_block::BlockRegistry;

use crate::sink::{storage_bind_group_layout, storage_buffer};

/// Binding group the material shader unit expects (`material.wgsl`).
pub const MATERIAL_BIND_GROUP: u32 = 1;

/// Words per [`voxel_block::HotRecord`] in the hot table.
pub const HOT_WORDS_PER_BLOCK: u32 = 2;

/// Words per six-face texture set in the texture-set table.
pub const TEXTURE_SET_WORDS: u32 = 3;

/// GPU tables for hot block data and deduplicated texture sets.
///
/// Both tables are indexed by [`voxel_block::MaterialId`] and
/// [`voxel_block::TextureSetId`] respectively, and both are immutable for the
/// lifetime of a frozen registry: build them once at startup.
pub struct MaterialBuffers {
    hot: wgpu::Buffer,
    texture_sets: wgpu::Buffer,
    blocks: u32,
    sets: u32,
}

impl MaterialBuffers {
    /// Uploads a frozen registry's hot records and texture sets.
    ///
    /// The buffers are sized from the registry and written once; an empty
    /// texture-set table (a registry where nothing draws) still gets a valid
    /// one-word buffer, because wgpu rejects zero-sized buffers.
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, registry: &BlockRegistry) -> Self {
        let blocks = registry.len() as u32;
        let sets = registry.texture_sets().len() as u32;

        let hot = registry.gpu_hot_words();
        let texture_sets = registry.gpu_texture_set_words();
        debug_assert_eq!(hot.len(), blocks as usize * HOT_WORDS_PER_BLOCK as usize);
        debug_assert_eq!(
            texture_sets.len(),
            sets as usize * TEXTURE_SET_WORDS as usize
        );

        Self {
            hot: upload_words(device, queue, "voxel-material-hot", &hot),
            texture_sets: upload_words(device, queue, "voxel-texture-sets", &texture_sets),
            blocks,
            sets,
        }
    }

    /// The hot-record buffer (`@group(1) @binding(0)` in `material.wgsl`).
    pub fn hot(&self) -> &wgpu::Buffer {
        &self.hot
    }

    /// The texture-set buffer (`@group(1) @binding(1)` in `material.wgsl`).
    pub fn texture_sets(&self) -> &wgpu::Buffer {
        &self.texture_sets
    }

    /// Number of hot records uploaded (one per registered block).
    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    /// Number of distinct texture sets uploaded.
    pub fn sets(&self) -> u32 {
        self.sets
    }

    /// Bind group for the current buffers, to be bound at
    /// [`MATERIAL_BIND_GROUP`].
    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel-material-bind-group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.hot.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.texture_sets.as_entire_binding(),
                },
            ],
        })
    }
}

/// Bind-group layout matching `material.wgsl` (group 1).
pub fn material_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    storage_bind_group_layout(device, "voxel-material-layout")
}

fn upload_words(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    words: &[u32],
) -> wgpu::Buffer {
    let buffer = storage_buffer(device, label, words.len());
    if !words.is_empty() {
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(words));
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::{HOT_WORDS_PER_BLOCK, TEXTURE_SET_WORDS};
    use voxel_block::HotRecord;

    #[test]
    fn stride_constants_match_the_packed_records() {
        assert_eq!(
            size_of::<HotRecord>(),
            HOT_WORDS_PER_BLOCK as usize * size_of::<u32>()
        );
        // A face set is six u16 ids: three u32 words.
        assert_eq!(TEXTURE_SET_WORDS, 3);
    }
}
