//! GPU vertex and index buffers for one chunk mesh.
//!
//! The mesh is uploaded once at creation. Streaming and partial updates are M3
//! work; this type owns the vertex/index buffers for either the M1.5 naive mesh
//! or the M2 production stream.

use std::mem::size_of;

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device};

use voxel_mesh::PackedVertex;

use crate::naive_mesh::{Mesh, Vertex};

/// One chunk's vertex and index buffers.
pub struct ChunkMesh {
    /// Buffer of [`Vertex`] records.
    pub vertices: Buffer,
    /// Buffer of `u16` indices.
    pub indices: Buffer,
    /// Number of vertices in [`Self::vertices`].
    pub vertex_count: u32,
    /// Number of indices in [`Self::indices`].
    pub index_count: u32,
    /// Bytes per uploaded vertex record.
    pub vertex_stride: u64,
}

impl ChunkMesh {
    /// Uploads the M1.5 naive mesh, or returns `None` for an empty mesh.
    ///
    /// The screenshot example and the naive reference tests use this path; the
    /// runtime viewer uploads the M2 production stream with [`Self::from_parts`].
    pub fn new(device: &Device, mesh: &Mesh) -> Option<Self> {
        Self::upload(
            device,
            bytemuck::cast_slice(&mesh.vertices),
            &mesh.indices,
            size_of::<Vertex>() as u64,
        )
    }

    /// Uploads the packed 8 B production vertex stream and `u16` indices.
    ///
    /// Empty meshes do not allocate zero-sized buffers.
    ///
    /// # Panics
    ///
    /// Panics if the vertex bytes are not a whole number of 8 B records.
    pub fn from_parts(
        device: &Device,
        vertices_bytes: &[u8],
        indices_data: &[u16],
    ) -> Option<Self> {
        Self::upload(
            device,
            vertices_bytes,
            indices_data,
            PackedVertex::SIZE as u64,
        )
    }

    fn upload(
        device: &Device,
        vertices_bytes: &[u8],
        indices_data: &[u16],
        vertex_stride: u64,
    ) -> Option<Self> {
        if indices_data.is_empty() {
            return None;
        }
        assert_eq!(
            vertices_bytes.len() as u64 % vertex_stride,
            0,
            "chunk vertex bytes must be whole {vertex_stride} B records"
        );
        let vertex_count = vertices_bytes.len() / vertex_stride as usize;
        assert!(
            vertex_count <= u32::MAX as usize,
            "chunk vertex count exceeds u32"
        );
        assert!(indices_data.len() <= u32::MAX as usize);

        // Write at creation rather than through `queue.write_buffer`: the
        // streaming shell creates hundreds of buffers between frames, and the
        // queue's staging belt can block on an outstanding submission.
        let vertices = create_mapped(
            device,
            "chunk vertices",
            vertices_bytes,
            BufferUsages::VERTEX,
        );
        let indices = create_mapped(
            device,
            "chunk indices",
            bytemuck::cast_slice(indices_data),
            BufferUsages::INDEX,
        );

        Some(Self {
            vertices,
            indices,
            vertex_count: vertex_count as u32,
            index_count: indices_data.len() as u32,
            vertex_stride,
        })
    }

    /// Number of quads represented by the index buffer.
    pub fn quad_count(&self) -> usize {
        self.index_count as usize / 6
    }

    /// Bytes in the vertex buffer.
    pub fn vertex_bytes(&self) -> u64 {
        u64::from(self.vertex_count) * self.vertex_stride
    }

    /// Bytes in the index buffer.
    pub fn index_bytes(&self) -> u64 {
        u64::from(self.index_count) * size_of::<u16>() as u64
    }

    /// Total bytes; 76/quad for the naive 16 B path, 44/quad for packed.
    pub fn byte_len(&self) -> u64 {
        self.vertex_bytes() + self.index_bytes()
    }
}

/// Creates a buffer and fills it through a creation mapping.
///
/// Mapped-at-creation uploads do not enter the queue's staging belt, so an
/// upload stage cannot stall behind an earlier submission. The mapped range
/// must be a multiple of the four-byte copy alignment; the tail is zeroed.
fn create_mapped(device: &Device, label: &str, bytes: &[u8], usage: BufferUsages) -> Buffer {
    let size = ((bytes.len() as u64).div_ceil(4) * 4).max(4);
    let created = std::time::Instant::now();
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: true,
    });
    let create_time = created.elapsed();
    let mapped = std::time::Instant::now();
    {
        let mut view = buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped-at-creation chunk buffer");
        view.slice(..bytes.len()).copy_from_slice(bytes);
        view.slice(bytes.len()..).fill(0);
    }
    let write_time = mapped.elapsed();
    let unmap_started = std::time::Instant::now();
    buffer.unmap();
    let unmap_time = unmap_started.elapsed();
    log::trace!(
        "{label}: {size} B create {:.2} ms, map+write {:.2} ms, unmap {:.2} ms",
        create_time.as_secs_f64() * 1000.0,
        write_time.as_secs_f64() * 1000.0,
        unmap_time.as_secs_f64() * 1000.0,
    );
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::constants::{CHUNK_BITS, CHUNK_VOLUME};

    use crate::naive_mesh::naive_mesh;

    fn block_volume() -> Vec<u16> {
        let mut volume = vec![0u16; CHUNK_VOLUME];
        for z in 0..8 {
            for y in 0..8 {
                for x in 0..8 {
                    let index = x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS));
                    volume[index] = 1;
                }
            }
        }
        volume
    }

    #[test]
    fn mesh_bytes_are_seventy_six_per_quad() {
        let mesh = naive_mesh(&block_volume());
        let quads = mesh.quad_count();
        assert!(quads > 0);
        assert_eq!(mesh.vertices.len(), quads * 4);
        assert_eq!(mesh.indices.len(), quads * 6);
        assert_eq!(mesh.byte_len(), quads * 76);
    }

    #[test]
    fn empty_mesh_needs_no_buffers() {
        let mesh = Mesh::default();
        assert_eq!(mesh.quad_count(), 0);
        assert_eq!(mesh.byte_len(), 0);
    }
}
