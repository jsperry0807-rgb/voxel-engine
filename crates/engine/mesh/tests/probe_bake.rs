//! M2-18: production SH4 probe bake equals the M1-30 reference.
//!
//! Three fixtures exercise a floor, a roofed cave and a lumpy terrain with
//! floating blocks. Every record and every validity bit must match the oracle at
//! the pre-registered 512-probe (8-block) edge, and the SVDAG path must match at
//! the compiled chunk edge.
//!
//! The second half pins the production difference: the bake is a frame-budgeted
//! state machine, so the same volume must come out of a one-ray budget and out of
//! an unbounded one, and no call may exceed the budget it was given.

use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME};
use voxel_core::content_constants::PROBE_BYTES;
use voxel_mesh::probe::{ProbeBake, ProbeGrid, ProbeLod, bake, bake_svdag};
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::local_index;
use voxel_storage::reference::probe as oracle;

/// Block edge that gives the pre-registered 512-probe near grid.
const TARGET_EDGE_BLOCKS: usize = 8;

/// Rays per probe, duplicated here so the test also pins the public constant.
const PROBE_RAYS_PER_PROBE: usize = 64;

type Solid = fn(u32, u32, u32) -> bool;

/// Bottom block layer solid everywhere, open sky above.
fn floor_solid(_x: u32, y: u32, _z: u32) -> bool {
    y < BLOCK_DIM as u32
}

/// A floor, a roofed half, a pillar and a floating block. Some probes sit in
/// solid cells; the roof gives rays a directional blocker and an enclosed pocket.
fn cave_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let bx = x / block;
    let by = y / block;
    let bz = z / block;

    if y < block {
        return true;
    }
    // Roof over the positive-x half, leaving a sky gap at z < 2.
    if (3..=6).contains(&by) && bx >= 3 && bz >= 2 {
        return true;
    }
    // A pillar reaching three blocks up.
    if bx == 1 && bz == 4 && by < 4 {
        return true;
    }
    // A floating block, so a ray can end mid-air through a hole.
    bx == 6 && by == 5 && bz == 1
}

/// Deterministic block-scale height field with two floating blocks.
fn terrain_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let bx = x / block;
    let by = y / block;
    let bz = z / block;

    let height = 1 + (bx * 7 + bz * 13) % 3;
    if by < height {
        return true;
    }
    if bx == 6 && by == 4 && bz == 2 {
        return true;
    }
    bx == 2 && by == 6 && bz == 6
}

fn fixtures() -> [(&'static str, Solid); 3] {
    [
        ("floor", floor_solid as Solid),
        ("cave", cave_solid as Solid),
        ("terrain", terrain_solid as Solid),
    ]
}

#[test]
fn probe_bake_matches_reference_on_three_fixtures() {
    let chunk = ChunkPos::new(7, -3, 11);

    for (name, solid) in fixtures() {
        let grid = ProbeGrid::near(TARGET_EDGE_BLOCKS);
        assert_eq!(
            grid.len(),
            512,
            "{name}: the near grid is one probe per block"
        );

        let volume = bake(chunk, grid, solid);
        let expected = oracle::bake(chunk, oracle::ProbeGrid::near(TARGET_EDGE_BLOCKS), solid);

        assert_eq!(volume.len(), expected.len(), "{name}: probe count");
        assert_eq!(
            volume.validity_bytes(),
            expected.validity_bytes(),
            "{name}: validity mask"
        );
        for index in 0..expected.len() {
            assert_eq!(
                volume.is_valid(index),
                expected.is_valid(index),
                "{name}: validity bit {index}"
            );
            assert_eq!(
                volume.probe(index).to_bytes(),
                expected.probe(index).to_bytes(),
                "{name}: record {index}"
            );
        }
        assert_eq!(volume.sh_bytes(), expected.sh_bytes(), "{name}: SH payload");
        assert_eq!(
            volume.len() * PROBE_BYTES,
            volume.sh_bytes().len(),
            "{name}: every record is 12 B"
        );
    }
}

fn dense_from(solid: Solid) -> Vec<u16> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for y in 0..CHUNK_DIM as u32 {
            for x in 0..CHUNK_DIM as u32 {
                if solid(x, y, z) {
                    dense[local_index(x, y, z) as usize] = 1;
                }
            }
        }
    }
    dense
}

#[test]
fn probe_bake_svdag_matches_reference_at_the_compiled_edge() {
    let chunk_pos = ChunkPos::new(-4, 2, 7);

    for (name, solid) in fixtures() {
        let dense = dense_from(solid);
        let chunk = Svdag::from_dense(&dense);
        let grid = ProbeGrid::for_chunk(ProbeLod::Near);
        let volume = bake_svdag(&chunk, chunk_pos, grid);
        let expected = oracle::bake_svdag(
            &chunk,
            chunk_pos,
            oracle::ProbeGrid::for_chunk(oracle::ProbeLod::Near),
        );

        assert_eq!(
            volume.len(),
            grid.len(),
            "{name}: the volume follows the compiled edge"
        );
        assert_eq!(
            volume.validity_bytes(),
            expected.validity_bytes(),
            "{name}: validity mask"
        );
        assert_eq!(volume.sh_bytes(), expected.sh_bytes(), "{name}: SH payload");
    }
}

/// A fixture that stays non-degenerate at a small edge: height variation, a
/// wall and a floating block. Invalid probes and blocked rays both occur.
fn split_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let bx = x / block;
    let by = y / block;
    let bz = z / block;

    if by < (bx + bz) % 3 + 1 {
        return true;
    }
    if bx == 2 && by >= 3 {
        return true;
    }
    bx == 3 && by == 2 && bz == 1
}

#[test]
fn probe_bake_split_across_frames_is_byte_identical() {
    let chunk = ChunkPos::new(3, -1, 5);
    let grid = ProbeGrid::near(4);
    let whole = bake(chunk, grid, split_solid);
    let valid = (0..whole.len())
        .filter(|&index| whole.is_valid(index))
        .count();
    assert!(
        valid > 0 && valid < whole.len(),
        "the fixture must contain valid and invalid probes"
    );
    assert_ne!(
        whole.sh_bytes(),
        bake(chunk, grid, |_, _, _| false).sh_bytes(),
        "the fixture must block some rays"
    );

    for budget in [1usize, 7, 63, 64, 65, 1_000] {
        let mut bake = ProbeBake::new(chunk, grid, split_solid);
        let mut rays = 0usize;
        let mut calls = 0usize;

        while !bake.is_done() {
            let step = bake.advance(budget);
            assert!(
                step.rays <= budget,
                "budget {budget}: a call cast {} rays",
                step.rays
            );
            assert!(
                step.probes > 0 || step.rays > 0,
                "budget {budget}: a non-zero budget must make progress"
            );
            rays += step.rays;
            calls += 1;
            assert!(
                calls <= grid.len() * (PROBE_RAYS_PER_PROBE + 1),
                "budget {budget}: the bake did not converge"
            );
        }

        assert_eq!(rays, valid * PROBE_RAYS_PER_PROBE, "budget {budget}: rays");
        assert_eq!(
            bake.rays_cast() as usize,
            rays,
            "budget {budget}: reported rays"
        );
        let volume = bake.finish();
        assert_eq!(volume.sh_bytes(), whole.sh_bytes(), "budget {budget}");
        assert_eq!(
            volume.validity_bytes(),
            whole.validity_bytes(),
            "budget {budget}"
        );
    }
}

#[test]
fn probe_bake_invalid_centres_cost_no_rays() {
    let grid = ProbeGrid::near(4);
    let mut bake = ProbeBake::new(ChunkPos::new(0, 0, 0), grid, floor_solid);
    let step = bake.advance(usize::MAX);
    assert!(step.done);
    let rays = bake.rays_cast() as usize;

    let volume = bake.finish();
    let invalid = (0..volume.len())
        .filter(|&index| !volume.is_valid(index))
        .count();
    let edge = grid.probes_per_edge();

    assert_eq!(invalid, edge * edge, "the bottom block layer is solid");
    assert_eq!(
        rays,
        (grid.len() - invalid) * PROBE_RAYS_PER_PROBE,
        "an invalid probe casts nothing"
    );
    for index in 0..volume.len() {
        if !volume.is_valid(index) {
            assert_eq!(
                volume.probe(index).to_bytes(),
                [128u8; PROBE_BYTES],
                "an invalid probe keeps the neutral record"
            );
        }
    }
}

/// Deterministic voxel-granularity speckle, deliberately not block aligned.
/// It exercises the DDA tie-break and probes whose centre is inside a solid.
fn speckle_solid(x: u32, y: u32, z: u32) -> bool {
    let mut hash = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= hash >> 32;
    hash.is_multiple_of(11)
}

#[test]
fn probe_bake_voxel_granularity_matches_reference() {
    let chunk = ChunkPos::new(-8, 3, 2);

    for edge_blocks in [3usize, 5] {
        let volume = bake(chunk, ProbeGrid::near(edge_blocks), speckle_solid);
        let expected = oracle::bake(chunk, oracle::ProbeGrid::near(edge_blocks), speckle_solid);

        assert_eq!(
            volume.validity_bytes(),
            expected.validity_bytes(),
            "edge {edge_blocks}: validity mask"
        );
        assert_eq!(
            volume.sh_bytes(),
            expected.sh_bytes(),
            "edge {edge_blocks}: SH payload"
        );
        assert!(
            (0..volume.len()).any(|index| !volume.is_valid(index)),
            "edge {edge_blocks}: the speckle must flag some solid centres"
        );
    }
}

#[test]
fn probe_bake_mid_and_far_grids_follow_adr_005() {
    let chunk = ChunkPos::new(1, 2, 3);
    let near = bake(chunk, ProbeGrid::near(TARGET_EDGE_BLOCKS), terrain_solid);
    let mid = bake(chunk, ProbeGrid::mid(TARGET_EDGE_BLOCKS), terrain_solid);
    let far = bake(chunk, ProbeGrid::far(TARGET_EDGE_BLOCKS), terrain_solid);

    let mid_expected = oracle::bake(
        chunk,
        oracle::ProbeGrid::mid(TARGET_EDGE_BLOCKS),
        terrain_solid,
    );

    assert_eq!(near.len(), 512);
    assert_eq!(mid.len(), 64, "mid is one probe per 2×2×2 blocks");
    assert_eq!(
        mid.validity_bytes(),
        mid_expected.validity_bytes(),
        "mid validity mask"
    );
    assert_eq!(mid.sh_bytes(), mid_expected.sh_bytes(), "mid SH payload");
    assert!(far.is_empty(), "a far chunk carries no probe volume");
    assert!(far.sh_bytes().is_empty());
    assert!(far.validity_bytes().is_empty());
}
