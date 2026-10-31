//! GPU vertex and index buffers for one chunk mesh.
//!
//! The mesh is uploaded once at creation. Streaming and partial updates are M3
//! work; this type owns the vertex/index buffers for either the M1.5 naive mesh
//! or the M2 production stream.

use std::mem::size_of;

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

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
    pub fn new(device: &Device, queue: &Queue, mesh: &Mesh) -> Option<Self> {
        Self::upload(
            device,
            queue,
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
        queue: &Queue,
        vertices_bytes: &[u8],
        indices_data: &[u16],
    ) -> Option<Self> {
        Self::upload(
            device,
            queue,
            vertices_bytes,
            indices_data,
            PackedVertex::SIZE as u64,
        )
    }

    fn upload(
        device: &Device,
        queue: &Queue,
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

        let vertices = device.create_buffer(&BufferDescriptor {
            label: Some("chunk vertices"),
            size: vertices_bytes.len() as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = device.create_buffer(&BufferDescriptor {
            label: Some("chunk indices"),
            size: std::mem::size_of_val(indices_data) as u64,
            usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        queue.write_buffer(&vertices, 0, vertices_bytes);
        queue.write_buffer(&indices, 0, bytemuck::cast_slice(indices_data));

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
