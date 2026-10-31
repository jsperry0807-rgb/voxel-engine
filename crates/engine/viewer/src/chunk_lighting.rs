//! GPU upload for the M2 light and probe volumes (M2-27).
//!
//! The chunk shader reads block light from a storage buffer. Probes are
//! uploaded and bound on the same bind group as a placeholder for the later
//! indirect pass; the shader does not sample them yet. Both buffers are
//! chunk-local: light is one 16 B cell per block, probes are the 12 B SH4
//! records followed by the packed validity mask.

use std::num::NonZeroU64;

use voxel_core::content_constants::LIGHT_BYTES_PER_CHUNK;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BufferBindingType, BufferDescriptor, BufferUsages, Device,
    ShaderStages,
};

/// Creates the chunk-data bind group layout (group 2).
///
/// Binding 0 is the light-cell array, binding 1 the probe SH/validity array,
/// both read-only storage buffers visible to the fragment stage. Binding 2 is
/// the chunk's world-space voxel origin, visible to the vertex stage.
pub fn layout(device: &Device) -> BindGroupLayout {
    // The light volume is exactly one 16 B cell per block, so a mismatched
    // upload (wrong chunk edge) is a validation error rather than silently
    // reading zeroes. Probe sizes follow the LOD and stay unbounded.
    let light = BindGroupLayoutEntry {
        binding: 0,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(LIGHT_BYTES_PER_CHUNK as u64),
        },
        count: None,
    };
    let probes = BindGroupLayoutEntry {
        binding: 1,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let origin = BindGroupLayoutEntry {
        binding: 2,
        visibility: ShaderStages::VERTEX,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(16),
        },
        count: None,
    };
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("chunk data"),
        entries: &[light, probes, origin],
    })
}

/// Uploaded light and probe buffers plus their bind group.
pub struct ChunkLighting {
    pub bind_group: BindGroup,
    /// Padded light-buffer size in bytes.
    pub light_bytes: u64,
    /// Padded probe-buffer size in bytes.
    pub probe_bytes: u64,
}

/// Uploads the light and probe bytes and binds them.
///
/// Buffers are padded to four bytes; a missing volume still gets a minimal
/// non-zero buffer so the binding stays valid.
pub fn upload(
    device: &Device,
    layout: &BindGroupLayout,
    light: &[u8],
    probes: &[u8],
    origin: [f32; 3],
) -> ChunkLighting {
    // Creation mappings keep these one-shot uploads out of the queue's
    // staging belt; a whole ring creates hundreds of them between frames.
    let light_bytes = upload_bytes(device, "chunk light", light);
    let probe_bytes = upload_bytes(device, "chunk probes", probes);
    let origin_buffer = upload_bytes_typed(
        device,
        "chunk origin",
        bytemuck::cast_slice(&[origin[0], origin[1], origin[2], 0.0]),
        BufferUsages::UNIFORM,
    );
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("chunk data"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: light_bytes.buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: probe_bytes.buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: origin_buffer.as_entire_binding(),
            },
        ],
    });
    ChunkLighting {
        bind_group,
        light_bytes: light_bytes.size,
        probe_bytes: probe_bytes.size,
    }
}

struct Uploaded {
    buffer: wgpu::Buffer,
    size: u64,
}

fn upload_bytes(device: &Device, label: &str, bytes: &[u8]) -> Uploaded {
    let size = (bytes.len().div_ceil(4) * 4).max(4) as u64;
    let buffer = upload_bytes_typed(device, label, bytes, BufferUsages::STORAGE);
    Uploaded { buffer, size }
}

/// Creates a padded buffer and writes it through a creation mapping.
fn upload_bytes_typed(
    device: &Device,
    label: &str,
    bytes: &[u8],
    usage: BufferUsages,
) -> wgpu::Buffer {
    let size = (bytes.len().div_ceil(4) * 4).max(4);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage,
        mapped_at_creation: true,
    });
    {
        let mut view = buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped-at-creation chunk buffer");
        view.slice(..bytes.len()).copy_from_slice(bytes);
        view.slice(bytes.len()..).fill(0);
    }
    buffer.unmap();
    buffer
}
