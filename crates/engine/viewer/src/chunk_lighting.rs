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
    Queue, ShaderStages,
};

/// Creates the chunk-data bind group layout (group 2).
///
/// Binding 0 is the light-cell array, binding 1 the probe SH/validity array.
/// Both are read-only storage buffers visible to the fragment stage.
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
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("chunk data"),
        entries: &[light, probes],
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
    queue: &Queue,
    layout: &BindGroupLayout,
    light: &[u8],
    probes: &[u8],
) -> ChunkLighting {
    let light_bytes = upload_bytes(device, queue, "chunk light", light);
    let probe_bytes = upload_bytes(device, queue, "chunk probes", probes);
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

fn upload_bytes(device: &Device, queue: &Queue, label: &str, bytes: &[u8]) -> Uploaded {
    let size = (bytes.len().div_ceil(4) * 4).max(4) as u64;
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut padded = bytes.to_vec();
    padded.resize(size as usize, 0);
    queue.write_buffer(&buffer, 0, &padded);
    Uploaded { buffer, size }
}
