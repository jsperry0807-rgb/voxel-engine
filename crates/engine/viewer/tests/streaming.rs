//! M3-01 streaming shell tests that do not need a GPU.
//!
//! The GPU path is exercised by `cargo run -p voxel-viewer`: the ring logs
//! `256 loaded` once every chunk is resident, together with the maximum pump
//! time and the number of pumps over the 4 ms budget.

use std::collections::HashSet;

use voxel_core::content_constants::LIGHT_BYTES_PER_CHUNK;
use voxel_mesh::PackedVertex;
use voxel_storage::apron::{Face, SIDE};
use voxel_storage::{ChunkPos, local_index};
use voxel_viewer::chunk_content;
use voxel_viewer::streaming::{
    RING_CHUNKS, RING_HALF, ring_chunks, ring_delta, stream_apron, stream_volume, terrain_height,
};

#[test]
fn ring_is_a_16_by_16_square() {
    let ring = ring_chunks(ChunkPos::new(11, 4, -7));
    assert_eq!(ring.len(), RING_CHUNKS);
    assert_eq!(RING_CHUNKS, 256);
    assert!(ring.iter().all(|chunk| chunk.y == 0));
    let unique: HashSet<ChunkPos> = ring.iter().copied().collect();
    assert_eq!(unique.len(), 256);
    assert!(
        ring.iter()
            .all(|chunk| (-RING_HALF..RING_HALF).contains(&(chunk.x - 11)))
    );
    assert!(
        ring.iter()
            .all(|chunk| (-RING_HALF..RING_HALF).contains(&(chunk.z + 7)))
    );
}

#[test]
fn ring_plan_evicts_the_trailing_column_and_loads_the_leading_one() {
    let center = ChunkPos::new(0, 0, 0);
    let loaded: HashSet<ChunkPos> = ring_chunks(center).into_iter().collect();
    let delta = ring_delta(center, loaded.iter().copied(), std::iter::empty());
    assert!(delta.load.is_empty());
    assert!(delta.evict.is_empty());

    let shifted = ring_delta(
        ChunkPos::new(1, 0, 0),
        loaded.iter().copied(),
        std::iter::empty(),
    );
    assert_eq!(shifted.load.len(), 2 * RING_HALF as usize);
    assert_eq!(shifted.evict.len(), 2 * RING_HALF as usize);
    assert!(shifted.load.iter().all(|chunk| chunk.x == 8));
    assert!(shifted.evict.iter().all(|chunk| chunk.x == -8));

    // A requested chunk that is still in flight is not queued twice.
    let requested = shifted.load.iter().copied().take(3);
    let delta = ring_delta(ChunkPos::new(1, 0, 0), loaded.iter().copied(), requested);
    assert_eq!(delta.load.len(), 2 * RING_HALF as usize - 3);
}

#[test]
fn a_position_can_be_evicted_and_loaded_again() {
    let origin = ChunkPos::new(0, 0, 0);
    let far = ChunkPos::new(16, 0, 0);
    let origin_ring: HashSet<ChunkPos> = ring_chunks(origin).into_iter().collect();
    let far_ring: HashSet<ChunkPos> = ring_chunks(far).into_iter().collect();
    assert!(origin_ring.is_disjoint(&far_ring));

    let evict = ring_delta(far, origin_ring.iter().copied(), std::iter::empty());
    assert_eq!(evict.evict.len(), RING_CHUNKS);
    assert_eq!(evict.load.len(), RING_CHUNKS);

    let back = ring_delta(origin, far_ring.iter().copied(), std::iter::empty());
    assert_eq!(back.evict.len(), RING_CHUNKS);
    assert_eq!(back.load.len(), RING_CHUNKS);
    assert!(back.load.contains(&ChunkPos::new(-RING_HALF, 0, 0)));
    assert!(back.load.contains(&ChunkPos::new(RING_HALF - 1, 0, 0)));
}

#[test]
fn stream_volume_is_deterministic_and_differs_by_chunk() {
    let position = ChunkPos::new(2, 0, -3);
    assert_eq!(stream_volume(position), stream_volume(position));
    assert_ne!(
        stream_volume(position),
        stream_volume(ChunkPos::new(2, 0, -2))
    );
}

#[test]
fn terrain_height_is_continuous_across_chunk_edges() {
    for boundary in [-128, 0, 128, 256, 512] {
        for z in [-200, -1, 0, 37, 96, 511] {
            let left = terrain_height(boundary - 1, z);
            let right = terrain_height(boundary, z);
            assert!(
                left.abs_diff(right) <= 6,
                "height jumps from {left} to {right} at x={boundary}, z={z}"
            );
        }
    }
    for boundary in [-128, 0, 128, 256] {
        for x in [-200, -1, 0, 37, 96, 511] {
            let low = terrain_height(x, boundary - 1);
            let high = terrain_height(x, boundary);
            assert!(
                low.abs_diff(high) <= 6,
                "height jumps from {low} to {high} at x={x}, z={boundary}"
            );
        }
    }
}

#[test]
fn analytic_apron_matches_the_neighbour_border() {
    let position = ChunkPos::new(-2, 0, 3);
    let apron = stream_apron(position);
    let neighbour = stream_volume(ChunkPos::new(-1, 0, 3));
    for v in 0..SIDE {
        for u in 0..SIDE {
            let local = local_index(0, v, u) as usize;
            assert_eq!(
                apron.get(Face::PosX, u, v),
                neighbour[local],
                "PosX apron texel ({u}, {v}) disagrees with the neighbour"
            );
        }
    }
}

#[test]
fn analytic_apron_culls_the_chunk_border() {
    let position = ChunkPos::new(3, 0, -2);
    let volume = stream_volume(position);
    let open = chunk_content::mesh(&volume);
    let closed = chunk_content::mesh_with_apron(&volume, &stream_apron(position));
    assert!(
        closed.quads < open.quads,
        "an analytic apron must remove border faces: open {}, closed {}",
        open.quads,
        closed.quads
    );
}

#[test]
fn full_relief_chunks_fit_the_u16_index_stream() {
    for position in [
        ChunkPos::new(8, 0, 8),
        ChunkPos::new(-8, 0, -8),
        ChunkPos::new(16, 0, -3),
        ChunkPos::new(-5, 0, 12),
    ] {
        let volume = stream_volume(position);
        let content = chunk_content::mesh_with_apron(&volume, &stream_apron(position));
        assert!(
            content.quads <= 16_384,
            "{position:?} has {} quads, over the u16 index stream",
            content.quads
        );
    }
}

#[test]
fn the_center_chunk_meshes_within_the_u16_index_stream() {
    let position = ChunkPos::new(0, 0, 0);
    let content = chunk_content::build(&stream_volume(position), position);
    assert!(!content.mesh.indices.is_empty());
    let vertices = content.mesh.vertices.len() / PackedVertex::SIZE;
    assert!(vertices <= u16::MAX as usize + 1);
    assert_eq!(content.mesh.indices.len(), content.mesh.quads * 6);
    assert!(
        content.mesh.quads <= 16_384,
        "chunk mesh exceeds u16 indices"
    );
    assert_eq!(content.light.len(), LIGHT_BYTES_PER_CHUNK);
    assert!(!content.probes.is_empty());
}
