//! GPU vertex and index buffers for one chunk mesh.
//!
//! The mesh is uploaded once at creation. Streaming and partial updates are M3
//! work; this type only proves the M1.5-10 mesh bytes reach the GPU unchanged.

use std::mem::size_of;

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

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
}

impl ChunkMesh {
    /// Uploads `mesh`, or returns `None` for an empty mesh.
    ///
    /// Empty chunks do not allocate zero-sized buffers; they simply have no
    /// draw work.
    pub fn new(device: &Device, queue: &Queue, mesh: &Mesh) -> Option<Self> {
        if mesh.indices.is_empty() {
            return None;
        }
        assert!(
            mesh.vertices.len() <= u32::MAX as usize,
            "chunk vertex count exceeds u32"
        );
        assert!(mesh.indices.len() <= u32::MAX as usize);

        let vertex_bytes = (mesh.vertices.len() * size_of::<Vertex>()) as u64;
        let index_bytes = (mesh.indices.len() * size_of::<u16>()) as u64;

        let vertices = device.create_buffer(&BufferDescriptor {
            label: Some("chunk vertices"),
            size: vertex_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = device.create_buffer(&BufferDescriptor {
            label: Some("chunk indices"),
            size: index_bytes,
            usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        queue.write_buffer(&vertices, 0, bytemuck::cast_slice(&mesh.vertices));
        queue.write_buffer(&indices, 0, bytemuck::cast_slice(&mesh.indices));

        Some(Self {
            vertices,
            indices,
            vertex_count: mesh.vertices.len() as u32,
            index_count: mesh.indices.len() as u32,
        })
    }

    /// Number of quads represented by the index buffer.
    pub fn quad_count(&self) -> usize {
        self.index_count as usize / 6
    }

    /// Bytes in the vertex buffer.
    pub fn vertex_bytes(&self) -> u64 {
        u64::from(self.vertex_count) * size_of::<Vertex>() as u64
    }

    /// Bytes in the index buffer.
    pub fn index_bytes(&self) -> u64 {
        u64::from(self.index_count) * size_of::<u16>() as u64
    }

    /// Total bytes, which must be 76 per quad for the ADR-010 format.
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
