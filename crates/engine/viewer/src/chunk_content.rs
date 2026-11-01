//! Production M2 content for the viewer's demo chunk (M2-27).
//!
//! The viewer draws one terrain patch. This module turns its dense voxel volume
//! into the three M2 content volumes the renderer needs:
//!
//! - the packed 8 B vertex stream from the production greedy mesher with AO
//!   (`greedy_block` + `mesh_block`), replacing the M1.5 naive per-face mesh;
//! - block-light cells from the production full pass (`voxel_mesh::light`);
//! - the near-grid SH4 probe volume from the production bake
//!   (`voxel_mesh::probe`).
//!
//! Mesh vertices are chunk-local and the emitted material id is the atlas slot:
//! the demo volume already stores atlas ids, so no registry resolution is
//! needed (the atlas is unchanged, as M2-27 requires). A real chunk stores
//! block ids and must use `mesh_resolved` with the registry before packing (M3
//! streaming).
//!
//! The chunk-wide index stream is `u16`, so a single mesh is capped at 16,384
//! quads (65,536 vertices). The 48×48 demo patch fits at both chunk edges; a
//! fully populated 128³ chunk does not and needs per-block meshlets (M3).
//!
//! Everything here is GPU-free and testable; `chunk_lighting` uploads the bytes
//! and `gpu` owns the buffers.

use voxel_core::constants::{BLOCK_DIM, CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME};
use voxel_core::content_constants::LIGHT_CELLS_PER_EDGE;
use voxel_mesh::light::{BlockLight, LightFill, full_pass};
use voxel_mesh::probe::{ProbeGrid, ProbeLod, bake};
use voxel_mesh::{PackedVertex, face_from_code, for_each_block, greedy_block, mesh_block};
use voxel_storage::{Apron, ChunkPos};

/// One chunk's production mesh as raw packed upload bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkMeshData {
    /// 8 B packed vertices (4 per quad + six `u16` indices = 44 B/quad).
    pub vertices: Vec<u8>,
    /// `u16` indices, chunk-wide.
    pub indices: Vec<u16>,
    /// Number of quads before flattening.
    pub quads: usize,
    /// Exposed voxel faces covered by those quads; equal to the naive per-face
    /// mesh's quad count when the merge loses no coverage.
    pub faces: usize,
    /// Blocks that emit at least one visible face.
    ///
    /// A fully enclosed non-empty block has no faces and is not counted: this
    /// is the geometry the draw actually covers, which is what M3-06 reports.
    pub blocks: usize,
}

impl ChunkMeshData {
    /// Vertex bytes plus index bytes.
    pub fn byte_len(&self) -> usize {
        self.vertices.len() + self.indices.len() * 2
    }

    /// The nearest naive per-face quad count for the same volume, for the
    /// M2-27 merge-ratio comparison.
    pub fn naive_quad_count(volume: &[u16]) -> usize {
        // The screenshot example keeps the M1.5 naive mesher as the reference;
        // this is the same count its `Mesh::quad_count` reports.
        let mut quads = 0;
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    if volume[local(x, y, z)] == 0 {
                        continue;
                    }
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let nx = x as i64 + dx;
                        let ny = y as i64 + dy;
                        let nz = z as i64 + dz;
                        let outside = nx < 0
                            || ny < 0
                            || nz < 0
                            || nx >= CHUNK_DIM as i64
                            || ny >= CHUNK_DIM as i64
                            || nz >= CHUNK_DIM as i64;
                        if outside || volume[local(nx as usize, ny as usize, nz as usize)] == 0 {
                            quads += 1;
                        }
                    }
                }
            }
        }
        quads
    }
}

/// One chunk's M2 content: mesh, light cells and probes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkContent {
    pub mesh: ChunkMeshData,
    /// `LIGHT_CELLS_PER_EDGE³ × 16 B`, chunk-local light cells.
    pub light: Vec<u8>,
    /// Near-grid SH4 records followed by the packed validity mask.
    pub probes: Vec<u8>,
}

/// Builds the mesh, light and probes for `volume`.
///
/// `volume` is a dense `CHUNK_DIM³` array with material 0 as air. Materials are
/// atlas slots for this demo chunk.
pub fn build(volume: &[u16], chunk: ChunkPos) -> ChunkContent {
    build_with_apron(volume, chunk, &Apron::new())
}

/// Like [`build`], but culls the chunk border against `apron`.
///
/// The streaming path fills the apron from the same analytic terrain function
/// as the volume, so border faces are culled against the real neighbour and AO
/// does not treat the chunk edge as air.
pub fn build_with_apron(volume: &[u16], chunk: ChunkPos, apron: &Apron) -> ChunkContent {
    assert_eq!(volume.len(), CHUNK_VOLUME, "a chunk volume is CHUNK_VOLUME");
    ChunkContent {
        mesh: mesh_with_apron(volume, apron),
        light: light(volume),
        probes: probes(volume, chunk),
    }
}

/// Meshes the chunk with the production greedy + AO path.
///
/// The volume must already store atlas ids. A real block-id chunk should use
/// [`mesh_resolved`] so the packed vertices carry atlas slots.
pub fn mesh(volume: &[u16]) -> ChunkMeshData {
    mesh_with_apron(volume, &Apron::new())
}

/// Meshes the chunk against `apron`, resolving materials with identity.
pub fn mesh_with_apron(volume: &[u16], apron: &Apron) -> ChunkMeshData {
    mesh_resolved_with_apron(volume, apron, |material, _face| material)
}

/// Meshes the chunk and resolves each vertex material before packing.
///
/// `resolve(block_material, face)` maps the chunk's block id to an atlas slot.
pub fn mesh_resolved<F>(volume: &[u16], resolve: F) -> ChunkMeshData
where
    F: FnMut(u16, voxel_core::Face) -> u16,
{
    mesh_resolved_with_apron(volume, &Apron::new(), resolve)
}

/// Meshes the chunk against `apron` and resolves each vertex material.
pub fn mesh_resolved_with_apron<F>(volume: &[u16], apron: &Apron, mut resolve: F) -> ChunkMeshData
where
    F: FnMut(u16, voxel_core::Face) -> u16,
{
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut quads = Vec::new();
    let mut quad_count = 0usize;
    let mut face_count = 0usize;
    let mut block_count = 0usize;

    for_each_block(volume, apron, |block, scratch| {
        greedy_block(scratch, &mut quads);
        if !quads.is_empty() {
            block_count += 1;
        }
        face_count += quads.iter().map(|quad| quad.area() as usize).sum::<usize>();
        let meshlet = mesh_block(&quads, block);
        let base = vertices.len() / PackedVertex::SIZE;
        assert!(
            base + meshlet.vertices.len() <= u16::MAX as usize + 1,
            "chunk mesh exceeds the u16 index range"
        );
        for vertex in &meshlet.vertices {
            let mut resolved = *vertex;
            resolved.material = resolve(vertex.material, face_from_code(vertex.face));
            vertices.extend_from_slice(&PackedVertex::from_vertex(&resolved).encode());
        }
        indices.extend(meshlet.indices.iter().map(|index| base as u16 + index));
        quad_count += meshlet.quad_count();
    });

    ChunkMeshData {
        vertices,
        indices,
        quads: quad_count,
        faces: face_count,
        blocks: block_count,
    }
}

/// Runs the production full light pass over the chunk's block cells.
pub fn light_fill(volume: &[u16]) -> LightFill {
    let cells = LIGHT_CELLS_PER_EDGE;
    let block = BLOCK_DIM;
    full_pass(
        cells,
        |cx, cy, cz| cell_light(volume, cx, cy, cz, block),
        |cx, cy, cz| cell_sky(volume, cx, cy, cz, block),
    )
}

/// Serializes the chunk-local light cells: 4 × `u16` per channel, two channels.
///
/// The shader reads the intensity halves at words 1 and 3 of each 16 B cell;
/// `tests/chunk_content.rs` pins that layout.
pub fn light(volume: &[u16]) -> Vec<u8> {
    let cells = LIGHT_CELLS_PER_EDGE;
    let fill = light_fill(volume);

    let mut bytes = Vec::with_capacity(cells * cells * cells * 16);
    for z in 0..cells {
        for y in 0..cells {
            for x in 0..cells {
                let cell = fill.local(x, y, z);
                for channel in [cell.sky, cell.block] {
                    for value in [
                        channel.rgb[0],
                        channel.rgb[1],
                        channel.rgb[2],
                        channel.intensity,
                    ] {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
        }
    }
    bytes
}

/// Bakes the near-grid probe volume and serializes SH + validity.
pub fn probes(volume: &[u16], chunk: ChunkPos) -> Vec<u8> {
    let grid = ProbeGrid::for_chunk(ProbeLod::Near);
    let volume = bake(chunk, grid, |x, y, z| {
        volume[local(x as usize, y as usize, z as usize)] != 0
    });
    let mut bytes = volume.sh_bytes();
    bytes.extend_from_slice(volume.validity_bytes());
    bytes
}

/// The viewer's deterministic terrain patch.
///
/// This mirrors the frozen M1.5 screenshot example's patch; the example stays
/// the canonical copy for the historical acceptance images.
///
/// 48×48 columns centred in the chunk, with a mound, a ripple and three
/// materials. Small enough to keep the chunk-wide `u16` index stream valid and
/// large enough to show slopes, material changes and AO corners.
pub fn demo_volume() -> Vec<u16> {
    let mut volume = vec![0u16; CHUNK_VOLUME];
    for z in 8..56 {
        for x in 8..56 {
            let fx = (x as f32 - 32.0) / 16.0;
            let fz = (z as f32 - 32.0) / 16.0;
            let mound = (1.0 - (fx * fx + fz * fz).sqrt()).max(0.0);
            let ripple = (x as f32 * 0.35).sin() * 0.5 + (z as f32 * 0.4).cos() * 0.5;
            let height = (14.0 + mound * 12.0 + ripple) as usize;
            for y in 0..height {
                let material = if y + 1 == height {
                    3
                } else if y + 4 >= height {
                    2
                } else {
                    1
                };
                let index = x | (y << CHUNK_BITS) | (z << (2 * CHUNK_BITS));
                volume[index] = material;
            }
        }
    }
    volume
}

fn cell_light(volume: &[u16], cx: i32, cy: i32, cz: i32, block: usize) -> BlockLight {
    let cells = LIGHT_CELLS_PER_EDGE as i32;
    if cx < 0 || cy < 0 || cz < 0 || cx >= cells || cy >= cells || cz >= cells {
        return BlockLight::AIR;
    }
    let mut solid = 0;
    let total = block * block * block;
    for z in 0..block {
        for y in 0..block {
            for x in 0..block {
                if volume[local(
                    cx as usize * block + x,
                    cy as usize * block + y,
                    cz as usize * block + z,
                )] != 0
                {
                    solid += 1;
                }
            }
        }
    }
    if solid == total {
        BlockLight::OPAQUE
    } else if solid > 0 {
        BlockLight::new(0, 8)
    } else {
        BlockLight::AIR
    }
}

fn cell_sky(volume: &[u16], cx: i32, cy: i32, cz: i32, block: usize) -> bool {
    let cells = LIGHT_CELLS_PER_EDGE as i32;
    if cx < 0 || cy < 0 || cz < 0 || cx >= cells || cy >= cells || cz >= cells {
        return true;
    }
    let x = cx as usize * block;
    let z = cz as usize * block;
    for y in (cy as usize + 1) * block..CHUNK_DIM {
        if volume[local(x, y, z)] != 0 {
            return false;
        }
    }
    true
}

fn local(x: usize, y: usize, z: usize) -> usize {
    x + y * CHUNK_DIM + z * CHUNK_DIM * CHUNK_DIM
}
