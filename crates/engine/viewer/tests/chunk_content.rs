//! M2-27: the viewer draws the production M2 content.
//!
//! The demo chunk must mesh with fewer quads than the M1.5 naive per-face
//! mesher, keep the ADR-010 16 B vertex layout, and carry both the block-light
//! cells and the near-grid probe volume for upload.

use voxel_core::constants::CHUNK_DIM;
use voxel_core::content_constants::LIGHT_CELLS_PER_EDGE;
use voxel_storage::ChunkPos;
use voxel_viewer::chunk_content::{self, ChunkMeshData};
use voxel_viewer::naive_mesh::naive_mesh;

fn content() -> chunk_content::ChunkContent {
    chunk_content::build(&chunk_content::demo_volume(), ChunkPos::new(0, 0, 0))
}

#[test]
fn production_mesh_beats_the_naive_face_count() {
    let volume = chunk_content::demo_volume();
    let content = chunk_content::build(&volume, ChunkPos::new(0, 0, 0));
    let naive = naive_mesh(&volume);

    assert!(content.mesh.quads > 0);
    assert!(
        content.mesh.quads < naive.quad_count(),
        "production {} quads vs naive {}",
        content.mesh.quads,
        naive.quad_count()
    );
    // Same exposed-face coverage as the naive mesh: the merge only removes
    // interior seams, so the silhouette and every visible face stay.
    assert_eq!(
        content.mesh.faces,
        naive.quad_count(),
        "the greedy mesh must cover every naive face"
    );
    assert_eq!(content.mesh.vertices.len(), content.mesh.quads * 4 * 16);
    assert_eq!(content.mesh.indices.len(), content.mesh.quads * 6);
    // The shader derives the AO corner from `vertex_index % 4`, which relies on
    // four consecutive vertices per quad and the [0,1,2,0,2,3] pattern.
    for (quad, indices) in content.mesh.indices.as_chunks::<6>().0.iter().enumerate() {
        let base = quad * 4;
        assert_eq!(base % 4, 0);
        assert_eq!(
            *indices,
            [
                base as u16,
                base as u16 + 1,
                base as u16 + 2,
                base as u16,
                base as u16 + 2,
                base as u16 + 3,
            ]
        );
    }
    println!(
        "M2-27 mesh: {} quads cover {} faces (naive {} faces, merge {:.1}%), {} B",
        content.mesh.quads,
        content.mesh.faces,
        naive.quad_count(),
        100.0 - content.mesh.quads as f64 * 100.0 / naive.quad_count() as f64,
        content.mesh.byte_len(),
    );
}

#[test]
fn production_mesh_is_deterministic_and_inside_the_chunk() {
    let first = content();
    let second = content();
    assert_eq!(first.mesh, second.mesh, "the mesh must be deterministic");
    assert_eq!(first.light, second.light);
    assert_eq!(first.probes, second.probes);

    let vertices = first.mesh.vertices.len() / 16;
    assert_eq!(vertices, first.mesh.quads * 4);
    for vertex in first.mesh.vertices.as_chunks::<16>().0 {
        let local = [
            u16::from_le_bytes([vertex[0], vertex[1]]),
            u16::from_le_bytes([vertex[2], vertex[3]]),
            u16::from_le_bytes([vertex[4], vertex[5]]),
        ];
        assert!(
            local.iter().all(|&axis| axis <= CHUNK_DIM as u16),
            "local {local:?} is outside the chunk"
        );
        assert!(vertex[8] <= 5, "face code {} is out of range", vertex[8]);
        assert_eq!(&vertex[13..16], &[0, 0, 0], "reserved bytes must be zero");
    }
    for &index in &first.mesh.indices {
        assert!((index as usize) < vertices, "index {index} out of range");
    }
}

#[test]
fn production_content_carries_light_and_probes() {
    let content = content();
    let cells = LIGHT_CELLS_PER_EDGE;
    assert_eq!(content.light.len(), cells * cells * cells * 16);
    // Near grid: one probe per block in a 4³ or 8³ block chunk, 12 B each,
    // plus one packed validity bit per probe.
    let probes = (CHUNK_DIM / 16) * (CHUNK_DIM / 16) * (CHUNK_DIM / 16);
    assert_eq!(content.probes.len(), probes * 12 + probes / 8);
    assert_eq!(
        content.mesh.byte_len(),
        content.mesh.vertices.len() + content.mesh.indices.len() * 2
    );
    assert_eq!(content.mesh.vertices.len(), content.mesh.quads * 4 * 16);
    // The chunk-wide u16 index stream is capped at 16,384 quads.
    assert!(content.mesh.indices.len() <= 6 * 16_384);
    assert_eq!(
        ChunkMeshData::naive_quad_count(&chunk_content::demo_volume()),
        naive_mesh(&chunk_content::demo_volume()).quad_count()
    );

    // The shader reads sky intensity from word 1 and block intensity from word
    // 3 of each 16 B cell (both in the high u16 half).
    let volume = chunk_content::demo_volume();
    let fill = chunk_content::light_fill(&volume);
    let bytes = chunk_content::light(&volume);
    for z in 0..cells {
        for y in 0..cells {
            for x in 0..cells {
                let cell = fill.local(x, y, z);
                let base = (x + y * cells + z * cells * cells) * 16;
                let word = |offset: usize| {
                    u32::from_le_bytes([
                        bytes[base + offset * 4],
                        bytes[base + offset * 4 + 1],
                        bytes[base + offset * 4 + 2],
                        bytes[base + offset * 4 + 3],
                    ])
                };
                assert_eq!(word(1) >> 16, u32::from(cell.sky.intensity));
                assert_eq!(word(3) >> 16, u32::from(cell.block.intensity));
            }
        }
    }
}
