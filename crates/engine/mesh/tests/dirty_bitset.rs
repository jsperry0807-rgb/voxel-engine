//! M2-22: per-block dirty flags derived from the COW diff.
//!
//! One 16³ block is one mesh unit, so an edit only has to report which blocks
//! changed. A block's 18³ scratch reads its own voxels plus a one-voxel border,
//! so a voxel on a block boundary is read by the blocks on both sides: one,
//! two, four or eight blocks for an interior, face, edge or corner voxel. A
//! chunk-border voxel marks only the blocks inside the chunk; the neighbour's
//! side is `mark_apron_cell`.
//!
//! The diff compares an edit batch against the pre-edit `Svdag`, so added,
//! removed and changed voxels all mark a block and a no-op write marks nothing.
//! A solid → solid change marks only its own block, because culling and AO are
//! binary.

use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::dirty::DirtyBlocks;
use voxel_mesh::{BlockPos, GreedyQuad, for_each_block, greedy_block};
use voxel_storage::dag::Svdag;
use voxel_storage::{AIR, Apron, Face, local_index};

fn blocks_per_edge() -> usize {
    CHUNK_DIM / BLOCK_DIM
}

fn block(x: usize, y: usize, z: usize) -> BlockPos {
    BlockPos::new(x as u32, y as u32, z as u32)
}

#[test]
fn dirty_bitset_interior_edit_marks_exactly_one_block() {
    let mut dirty = DirtyBlocks::new();
    assert!(dirty.is_empty());

    // (24, 40, 8) is interior to block (1, 2, 0) at both chunk edges.
    dirty.mark_voxel(24, 40, 8);
    assert_eq!(dirty.count(), 1);
    assert!(dirty.is_dirty(block(1, 2, 0)));
    assert_eq!(dirty.iter().collect::<Vec<_>>(), vec![block(1, 2, 0)]);
}

#[test]
fn dirty_bitset_face_edit_marks_the_block_and_its_neighbour() {
    // A voxel on a block's low face is in the previous block's +face apron.
    let mut low = DirtyBlocks::new();
    low.mark_voxel(BLOCK_DIM as u32, 40, 8);
    assert_eq!(low.count(), 2, "own block plus the low neighbour");
    assert!(low.is_dirty(block(1, 2, 0)));
    assert!(low.is_dirty(block(0, 2, 0)));

    // A voxel on the high face is in the next block's -face apron.
    let mut high = DirtyBlocks::new();
    let high_x = (2 * BLOCK_DIM + BLOCK_DIM - 1) as u32;
    high.mark_voxel(high_x, 40, 8);
    assert_eq!(high.count(), 2, "own block plus the high neighbour");
    assert!(high.is_dirty(block(2, 2, 0)));
    assert!(high.is_dirty(block(3, 2, 0)));
}

#[test]
fn dirty_bitset_edge_and_corner_edits_mark_four_and_eight_blocks() {
    let mut edge = DirtyBlocks::new();
    // Low face in x and y: the four blocks whose scratches contain the voxel.
    edge.mark_voxel(BLOCK_DIM as u32, 2 * BLOCK_DIM as u32, 8);
    assert_eq!(edge.count(), 4);
    for y in 0..2 {
        for x in 0..2 {
            assert!(edge.is_dirty(block(1 - x, 2 - y, 0)));
        }
    }

    let mut corner = DirtyBlocks::new();
    // Low face in x, y and z: the full 2×2×2 block cube.
    corner.mark_voxel(BLOCK_DIM as u32, 2 * BLOCK_DIM as u32, BLOCK_DIM as u32);
    assert_eq!(corner.count(), 8);
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                assert!(corner.is_dirty(block(1 - x, 2 - y, 1 - z)));
            }
        }
    }
}

#[test]
fn dirty_bitset_matches_the_block_scratch_coverage() {
    // Independent check of the rule: a block's 18³ scratch spans
    // [origin - 1, origin + BLOCK_DIM], so the dirty set must be exactly the
    // blocks whose scratch contains the edited voxel.
    let n = blocks_per_edge();
    let voxels = [
        (8u32, 8, 8),
        (24, 40, 8),
        (BLOCK_DIM as u32, 40, 8),
        ((2 * BLOCK_DIM + BLOCK_DIM - 1) as u32, 40, 8),
        (BLOCK_DIM as u32, 2 * BLOCK_DIM as u32, 8),
        (BLOCK_DIM as u32, 2 * BLOCK_DIM as u32, BLOCK_DIM as u32),
        (0, 40, 8),
        ((CHUNK_DIM - 1) as u32, 40, 8),
        (
            (CHUNK_DIM - 1) as u32,
            (CHUNK_DIM - 1) as u32,
            (CHUNK_DIM - 1) as u32,
        ),
    ];

    for (vx, vy, vz) in voxels {
        let mut dirty = DirtyBlocks::new();
        dirty.mark_voxel(vx, vy, vz);

        let mut expected = Vec::new();
        for bz in 0..n {
            for by in 0..n {
                for bx in 0..n {
                    let covers = |v: u32, origin: usize| {
                        let origin = origin as i32;
                        let v = v as i32;
                        v >= origin - 1 && v <= origin + BLOCK_DIM as i32
                    };
                    if covers(vx, bx * BLOCK_DIM)
                        && covers(vy, by * BLOCK_DIM)
                        && covers(vz, bz * BLOCK_DIM)
                    {
                        expected.push(block(bx, by, bz));
                    }
                }
            }
        }

        assert_eq!(
            dirty.iter().collect::<Vec<_>>(),
            expected,
            "dirty blocks for voxel ({vx}, {vy}, {vz})"
        );
    }
}

#[test]
fn dirty_bitset_apron_cells_mark_the_neighbour_side_blocks() {
    // The apron cell is one voxel outside the face; the dirty blocks are the
    // blocks on this side whose 18³ scratch contains it.
    let n = blocks_per_edge();
    let last = CHUNK_DIM as u32 - 1;
    let cells = [(0u32, 0u32), (8, 8), (last, last), (16, 16), (16, 32)];

    for face in Face::ALL {
        for (u, v) in cells {
            let mut dirty = DirtyBlocks::new();
            dirty.mark_apron_cell(face, u, v);

            let (ox, oy, oz) = face.offset(u, v);
            let mut expected = Vec::new();
            for bz in 0..n {
                for by in 0..n {
                    for bx in 0..n {
                        let covers = |voxel: i32, origin: usize| {
                            let origin = origin as i32;
                            voxel >= origin - 1 && voxel <= origin + BLOCK_DIM as i32
                        };
                        if covers(ox, bx * BLOCK_DIM)
                            && covers(oy, by * BLOCK_DIM)
                            && covers(oz, bz * BLOCK_DIM)
                        {
                            expected.push(block(bx, by, bz));
                        }
                    }
                }
            }

            assert_eq!(
                dirty.iter().collect::<Vec<_>>(),
                expected,
                "face {face:?}, apron cell ({u}, {v})"
            );
        }
    }
}

#[test]
fn dirty_bitset_chunk_border_has_no_outside_neighbour() {
    let last = (CHUNK_DIM - 1) as u32;
    let n = blocks_per_edge();

    let mut low = DirtyBlocks::new();
    low.mark_voxel(0, 40, 8);
    assert_eq!(low.count(), 1, "x = 0 has no block before it");
    assert!(low.is_dirty(block(0, 2, 0)));

    let mut high = DirtyBlocks::new();
    high.mark_voxel(last, 40, 8);
    assert_eq!(high.count(), 1, "the last block has no block after it");
    assert!(high.is_dirty(block(n - 1, 2, 0)));
}

#[test]
fn dirty_bitset_follows_the_cow_diff() {
    let mut dense = vec![AIR; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for x in 0..CHUNK_DIM as u32 {
            for y in 0..BLOCK_DIM as u32 {
                dense[local_index(x, y, z) as usize] = 1;
            }
        }
    }
    let dag = Svdag::from_dense(&dense);

    // Added: air above the floor, interior to block (0, 1, 0).
    let added = local_index(8, 24, 8);
    // Removed: the top face of the floor, block (0, 0, 0) plus the apron above.
    let removed = local_index(8, BLOCK_DIM as u32 - 1, 8);
    // Changed: solid to a different solid on the same high face.
    let changed = local_index(24, BLOCK_DIM as u32 - 1, 8);
    // Unchanged: writes the material already there.
    let same = local_index(40, BLOCK_DIM as u32 - 1, 8);

    let dirty =
        DirtyBlocks::from_edits(&dag, [(added, 7), (removed, AIR), (changed, 9), (same, 1)]);

    assert!(dirty.is_dirty(block(0, 1, 0)), "added voxel");
    assert!(dirty.is_dirty(block(0, 0, 0)), "removed voxel");
    assert!(dirty.is_dirty(block(1, 0, 0)), "changed voxel");
    assert!(
        !dirty.is_dirty(block(1, 1, 0)),
        "a solid → solid change does not dirty the apron"
    );
    assert!(
        !dirty.is_dirty(block(2, 0, 0)),
        "an unchanged write is not dirty"
    );
    assert_eq!(dirty.count(), 3);

    // A batch that changes a voxel and changes it back is not dirty.
    let revert = DirtyBlocks::from_edits(&dag, [(removed, 5), (removed, 1)]);
    assert!(revert.is_empty());

    // Applying the batch to the COW chunk produces the expected content.
    let next = dag.apply_edits([(added, 7), (removed, AIR), (changed, 9), (same, 1)]);
    assert_eq!(next.get(8, 24, 8), 7);
    assert_eq!(next.get(8, BLOCK_DIM as u32 - 1, 8), AIR);
}

#[test]
fn dirty_bitset_mark_edit_ignores_an_unchanged_write() {
    let mut dirty = DirtyBlocks::new();
    assert!(!dirty.mark_edit(8, 8, 8, 3, 3));
    assert!(dirty.is_empty());

    assert!(dirty.mark_edit(8, 8, 8, 0, 3));
    assert_eq!(dirty.count(), 1);
    assert!(dirty.is_dirty(block(0, 0, 0)));
}

#[test]
fn dirty_bitset_solid_to_solid_marks_only_its_block() {
    let mut dirty = DirtyBlocks::new();
    // A corner voxel: an added/removed edit would mark eight blocks, but the
    // mesher's culling and AO are binary and the vertex carries no light.
    assert!(dirty.mark_edit(
        BLOCK_DIM as u32,
        2 * BLOCK_DIM as u32,
        BLOCK_DIM as u32,
        1,
        2
    ));
    assert_eq!(dirty.count(), 1);
    assert!(dirty.is_dirty(block(1, 2, 1)));
}

#[test]
fn dirty_bitset_layout_covers_every_block() {
    let n = blocks_per_edge();
    let mut dirty = DirtyBlocks::new();

    assert_eq!(dirty.blocks_per_edge(), n);
    assert_eq!(dirty.len(), n * n * n);
    assert_eq!(dirty.words().len(), (n * n * n).div_ceil(64));
    #[cfg(feature = "chunk128")]
    assert_eq!(dirty.words().len(), 8, "512 block bits at 128³");
    #[cfg(not(feature = "chunk128"))]
    assert_eq!(dirty.words().len(), 1, "64 block bits at 64³");
    assert!(!dirty.is_dirty(block(0, 0, 0)));

    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                dirty.mark_block(block(x, y, z));
            }
        }
    }
    assert_eq!(dirty.count(), n * n * n);
    assert_eq!(dirty.iter().count(), n * n * n);
    assert_eq!(dirty.iter().next(), Some(block(0, 0, 0)));
    assert_eq!(dirty.iter().last(), Some(block(n - 1, n - 1, n - 1)));

    // The scheduler retires one block at a time.
    assert!(dirty.clear_block(block(0, 0, 0)));
    assert!(!dirty.clear_block(block(0, 0, 0)));
    assert!(!dirty.is_dirty(block(0, 0, 0)));
    assert_eq!(dirty.count(), n * n * n - 1);

    // Two batches between frames merge with `union`.
    let mut other = DirtyBlocks::new();
    other.mark_block(block(0, 0, 0));
    dirty.union(&other);
    assert_eq!(dirty.count(), n * n * n);

    dirty.clear();
    assert!(dirty.is_empty());
    assert_eq!(dirty.count(), 0);
    assert_eq!(dirty.iter().count(), 0);
}

/// A floor at the bottom and a roof at the top, so toggling a surface voxel
/// creates or removes faces and moves AO.
fn scene() -> Vec<u16> {
    let mut voxels = vec![AIR; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for x in 0..CHUNK_DIM as u32 {
            for y in 0..BLOCK_DIM as u32 {
                voxels[local_index(x, y, z) as usize] = 1;
            }
            for y in CHUNK_DIM as u32 - BLOCK_DIM as u32..CHUNK_DIM as u32 {
                voxels[local_index(x, y, z) as usize] = 1;
            }
        }
    }
    voxels
}

fn meshes(voxels: &[u16]) -> Vec<Vec<GreedyQuad>> {
    let n = blocks_per_edge();
    let mut out = vec![Vec::new(); n * n * n];
    let apron = Apron::new();
    for_each_block(voxels, &apron, |pos, scratch| {
        let mut quads = Vec::new();
        greedy_block(scratch, &mut quads);
        let index = pos.x as usize + pos.y as usize * n + pos.z as usize * n * n;
        out[index] = quads;
    });
    out
}

#[test]
fn dirty_bitset_covers_every_block_the_mesher_changes() {
    // The real consumer check: toggle one voxel, remesh every block, and assert
    // that every block whose quads changed was flagged. Running against
    // `greedy_block` is what ties the bitset to the 18³ scratch geometry.
    let base = scene();
    let before = meshes(&base);
    let n = blocks_per_edge();
    let last = CHUNK_DIM as u32 - 1;
    let samples = [
        (8u32, 7u32, 8u32),                       // top of the floor, interior block
        (8, 8, 8),                                // air just above the floor
        (BLOCK_DIM as u32, 15, 8),                // face voxel in x and y
        (BLOCK_DIM as u32, 15, BLOCK_DIM as u32), // corner voxel
        (0, 7, 8),                                // chunk border
        (last, 7, 8),                             // far chunk border
        (24, CHUNK_DIM as u32 - 1 - 1, 16),       // roof face
        (24, 40, 24),                             // free-floating voxel
    ];

    for (vx, vy, vz) in samples {
        let mut edited = base.clone();
        let index = local_index(vx, vy, vz) as usize;
        edited[index] = if edited[index] == AIR { 3 } else { AIR };
        let after = meshes(&edited);

        let mut dirty = DirtyBlocks::new();
        dirty.mark_voxel(vx, vy, vz);

        let mut changed = 0;
        for bz in 0..n {
            for by in 0..n {
                for bx in 0..n {
                    let block = bx + by * n + bz * n * n;
                    if before[block] != after[block] {
                        changed += 1;
                        assert!(
                            dirty.is_dirty(BlockPos::new(bx as u32, by as u32, bz as u32)),
                            "voxel ({vx}, {vy}, {vz}) changed block ({bx}, {by}, {bz}) \
                             but did not dirty it"
                        );
                    }
                }
            }
        }
        assert!(changed > 0, "voxel ({vx}, {vy}, {vz}) must change a mesh");

        // A solid → solid edit uses the precise branch: only the voxel's own
        // block can change, and the mesher must not move any other block.
        if base[index] != AIR {
            let mut recoloured = base.clone();
            recoloured[index] = if base[index] == 2 { 3 } else { 2 };
            let after_recolour = meshes(&recoloured);
            let mut solid_dirty = DirtyBlocks::new();
            assert!(solid_dirty.mark_edit(vx, vy, vz, base[index], recoloured[index]));

            for bz in 0..n {
                for by in 0..n {
                    for bx in 0..n {
                        let block = bx + by * n + bz * n * n;
                        if before[block] != after_recolour[block] {
                            assert!(
                                solid_dirty
                                    .is_dirty(BlockPos::new(bx as u32, by as u32, bz as u32)),
                                "solid → solid at ({vx}, {vy}, {vz}) changed block \
                                 ({bx}, {by}, {bz}) but did not dirty it"
                            );
                        }
                    }
                }
            }
        }
    }
}
