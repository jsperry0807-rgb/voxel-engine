//! Deterministic SH4 probe-bake reference tests (M1-30).
//!
//! The fixtures pin the four things the production bake inherits:
//!
//! 1. the near grid has one probe per block — 64 at 64³ and **512 at 128³**;
//! 2. a valid probe casts exactly 64 upper-hemisphere rays and stores four
//!    RGB8 coefficients in 12 B;
//! 3. a probe centre in a solid block is flagged invalid and carries no
//!    radiance;
//! 4. the seed is a pure function of chunk position, probe index and LOD, so
//!    ten serial runs and 1/4-thread runs produce byte-identical volumes.
//!
//! The ray/encoding tests are deliberately analytic: open sky has a known DC
//! coefficient, an enclosed box has none, and an RGB8 coefficient round-trips
//! inside one quantisation step. The SVDAG test then exercises the compiled
//! chunk edge without duplicating the ray logic.

use std::f64::consts::{PI, TAU};
use std::mem::size_of;
use std::thread;

use voxel_core::constants::{BLOCK_DIM, BLOCKS_PER_CHUNK, CHUNK_DIM, CHUNK_VOLUME};
use voxel_core::content_constants::PROBE_BYTES;
use voxel_storage::dag::Svdag;
use voxel_storage::reference::probe::{
    PROBE_CHANNELS, PROBE_RAYS, ProbeGrid, ProbeLod, SH_COEFFICIENTS, SH_L0, Sh4Probe, bake,
    bake_svdag, hemisphere_ray, probe_seed,
};
use voxel_storage::{ChunkPos, local_index};

/// Block edge that gives the pre-registered 512-probe near grid.
const PROBE_EDGE_BLOCKS: usize = 8;

/// A deliberately asymmetric fixture: a floor, a column and a floating block.
fn fixture_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let bx = x / block;
    let by = y / block;
    let bz = z / block;

    // The bottom block layer is solid everywhere.
    if y < block {
        return true;
    }
    // A column at block (2, 2, 5) reaching four blocks up.
    if bx == 2 && bz == 5 && by < 4 {
        return true;
    }
    // A single floating block at (6, 3, 1).
    bx == 6 && by == 3 && bz == 1
}

fn bake_fixture() -> voxel_storage::reference::probe::ProbeVolume {
    bake(
        ChunkPos::new(7, -3, 11),
        ProbeGrid::near(PROBE_EDGE_BLOCKS),
        fixture_solid,
    )
}

#[test]
fn near_grid_has_five_hundred_and_twelve_probes_at_the_pre_registered_edge() {
    assert_eq!(PROBE_EDGE_BLOCKS, 8);
    assert_eq!(ProbeGrid::near(PROBE_EDGE_BLOCKS).len(), 512);
    assert_eq!(ProbeGrid::near(4).len(), 64);

    // The other two rings follow ADR-005: half resolution mid, none far.
    assert_eq!(ProbeGrid::mid(PROBE_EDGE_BLOCKS).len(), 64);
    assert_eq!(ProbeGrid::far(PROBE_EDGE_BLOCKS).len(), 0);

    let volume = bake_fixture();
    assert_eq!(volume.len(), 512);
    assert_eq!(volume.probes().len(), 512);
    assert_eq!(volume.sh_bytes().len(), 512 * PROBE_BYTES);
    assert_eq!(volume.validity_bytes().len(), 512 / 8);
}

#[test]
fn probes_sit_at_block_centres() {
    let grid = ProbeGrid::near(PROBE_EDGE_BLOCKS);
    let half = (BLOCK_DIM / 2) as f64;

    for (index, (x, y, z)) in [(0, 0, 0), (1, 2, 3), (7, 7, 7)].into_iter().enumerate() {
        let expected = [
            x as f64 * BLOCK_DIM as f64 + half,
            y as f64 * BLOCK_DIM as f64 + half,
            z as f64 * BLOCK_DIM as f64 + half,
        ];
        // The literal tuple above is a block coordinate, not a linear index.
        let linear = x + y * PROBE_EDGE_BLOCKS + z * PROBE_EDGE_BLOCKS * PROBE_EDGE_BLOCKS;
        assert_eq!(grid.block_coords(linear), (x, y, z), "case {index}");
        assert_eq!(grid.centre(linear), expected, "case {index}");
    }
}

#[test]
fn hemisphere_has_sixty_four_unit_upward_rays() {
    assert_eq!(PROBE_RAYS, 64);

    for index in 0..PROBE_RAYS {
        for rotation in [0.0, 0.75, TAU - 0.1] {
            let direction = hemisphere_ray(index, rotation);
            assert!(
                direction[1] > 0.0,
                "ray {index} must point into the upper hemisphere"
            );
            let length = direction.iter().map(|axis| axis * axis).sum::<f64>().sqrt();
            assert!(
                (length - 1.0).abs() < 1e-12,
                "ray {index} is not unit length"
            );
        }
    }
}

#[test]
fn probe_record_is_twelve_bytes_of_sh4_rgb8() {
    assert_eq!(SH_COEFFICIENTS, 4);
    assert_eq!(PROBE_CHANNELS, 3);
    assert_eq!(PROBE_BYTES, 12);
    assert_eq!(size_of::<Sh4Probe>(), PROBE_BYTES);
    assert_eq!(Sh4Probe::ZERO.to_bytes(), [128; PROBE_BYTES]);

    let coefficients = [[-1.75, 0.0, 1.75], [0.5, -0.5, 0.0], [0.0; 3], [0.25; 3]];
    let record = Sh4Probe::from_coefficients(coefficients);
    let bytes = record.to_bytes();
    assert_eq!(bytes.len(), PROBE_BYTES);

    let decoded = record.coefficients();
    for (basis, expected) in decoded.into_iter().zip(coefficients) {
        for (channel, expected) in basis.into_iter().zip(expected) {
            assert!(
                (channel - expected).abs() <= 2.0 / 255.0,
                "RGB8 channel {channel} missed {expected}"
            );
        }
    }
}

#[test]
fn open_sky_encodes_the_uniform_hemisphere_dc() {
    let volume = bake(ChunkPos::new(0, 0, 0), ProbeGrid::near(2), |_, _, _| false);

    let expected = 2.0 * PI * SH_L0;
    assert!((expected - 1.772_453_851).abs() < 1e-6);

    for index in 0..volume.len() {
        assert!(volume.is_valid(index), "open sky is always valid");
        let coefficients = volume.probe(index).coefficients();
        for channel in coefficients[0] {
            assert!(
                (channel - expected).abs() < 0.03,
                "open-sky DC at probe {index} is {channel} not {expected}"
            );
        }
    }
}

#[test]
fn enclosed_box_has_no_sky_radiance() {
    let edge_blocks = 3;
    let edge = (edge_blocks * BLOCK_DIM) as u32;
    let volume = bake(
        ChunkPos::new(-1, 2, -3),
        ProbeGrid::near(edge_blocks),
        |x, y, z| x == 0 || y == 0 || z == 0 || x == edge - 1 || y == edge - 1 || z == edge - 1,
    );

    // The one-voxel shell is outside every probe centre, so all probes are
    // valid but every ray terminates on a solid boundary.
    for index in 0..volume.len() {
        assert!(volume.is_valid(index));
        let coefficients = volume.probe(index).coefficients();
        for basis in coefficients {
            for channel in basis {
                assert!(
                    channel.abs() < 0.02,
                    "enclosed probe {index} has radiance {channel}"
                );
            }
        }
    }

    // An enclosed bake still has the neutral encoded record, not an invalid
    // one. Only the validity mask distinguishes "no sky" from "unused".
    assert_eq!(volume.sh_bytes(), vec![128u8; volume.len() * PROBE_BYTES]);
}

#[test]
fn probe_centre_in_a_solid_block_is_flagged() {
    let volume = bake(ChunkPos::new(0, 0, 0), ProbeGrid::near(3), |_, y, _| {
        y < BLOCK_DIM as u32
    });

    // The bottom block layer is solid, so its 3×3 probes are invalid. Every
    // probe above it is valid.
    let mut valid = 0;
    for z in 0..3usize {
        for y in 0..3usize {
            for x in 0..3usize {
                let index = x + y * 3 + z * 9;
                let expected = y != 0;
                assert_eq!(volume.is_valid(index), expected, "probe ({x}, {y}, {z})");
                if expected {
                    valid += 1;
                } else {
                    assert_eq!(volume.probe(index), Sh4Probe::ZERO);
                }
            }
        }
    }

    let mask_bits: u32 = volume
        .validity_bytes()
        .iter()
        .map(|byte| byte.count_ones())
        .sum();
    assert_eq!(mask_bits, valid);
    assert_eq!(valid, 18);
}

#[test]
fn far_lod_bakes_an_empty_volume() {
    let mut called = false;
    let volume = bake(
        ChunkPos::new(1, 2, 3),
        ProbeGrid::far(PROBE_EDGE_BLOCKS),
        |_, _, _| {
            called = true;
            false
        },
    );

    assert!(volume.is_empty());
    assert!(volume.probes().is_empty());
    assert!(volume.sh_bytes().is_empty());
    assert!(volume.validity_bytes().is_empty());
    assert!(!called, "a far grid must not cast rays");
}

#[test]
fn seed_depends_only_on_chunk_position_probe_index_and_lod() {
    let chunk = ChunkPos::new(4, -5, 6);
    let seed = probe_seed(chunk, ProbeLod::Near, 17);

    assert_eq!(seed, probe_seed(chunk, ProbeLod::Near, 17));
    assert_ne!(
        seed,
        probe_seed(ChunkPos::new(5, -5, 6), ProbeLod::Near, 17)
    );
    assert_ne!(
        seed,
        probe_seed(ChunkPos::new(4, -4, 6), ProbeLod::Near, 17)
    );
    assert_ne!(
        seed,
        probe_seed(ChunkPos::new(4, -5, 7), ProbeLod::Near, 17)
    );
    assert_ne!(seed, probe_seed(chunk, ProbeLod::Near, 18));
    assert_ne!(seed, probe_seed(chunk, ProbeLod::Mid, 17));
    assert_ne!(seed, probe_seed(chunk, ProbeLod::Far, 17));

    // The seed is the only input to the per-probe azimuth, so a changed seed
    // must change the baked bytes on an asymmetric fixture.
    let near = bake(chunk, ProbeGrid::near(PROBE_EDGE_BLOCKS), fixture_solid);
    let moved = bake(
        ChunkPos::new(5, -5, 6),
        ProbeGrid::near(PROBE_EDGE_BLOCKS),
        fixture_solid,
    );
    assert_ne!(near.sh_bytes(), moved.sh_bytes());
}

#[test]
fn bake_is_byte_identical_across_ten_runs_and_one_or_four_threads() {
    let first = bake_fixture();
    let first_sh = first.sh_bytes();
    let first_mask = first.validity_bytes().to_vec();

    for run in 1..10 {
        let next = bake_fixture();
        assert_eq!(next.sh_bytes(), first_sh, "serial run {run} differs");
        assert_eq!(
            next.validity_bytes(),
            first_mask.as_slice(),
            "serial mask {run} differs"
        );
    }

    for workers in [1usize, 4] {
        let volumes = thread::scope(|scope| {
            let handles: Vec<_> = (0..workers).map(|_| scope.spawn(bake_fixture)).collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("probe bake thread panicked"))
                .collect::<Vec<_>>()
        });

        for (worker, volume) in volumes.into_iter().enumerate() {
            assert_eq!(
                volume.sh_bytes(),
                first_sh,
                "worker {worker} of {workers} produced different SH bytes"
            );
            assert_eq!(
                volume.validity_bytes(),
                first_mask.as_slice(),
                "worker {worker} of {workers} produced a different mask"
            );
        }
    }
}

#[test]
fn compiled_chunk_bake_matches_the_block_grid_and_flags_the_floor() {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM as u32 {
        for y in 0..BLOCK_DIM as u32 {
            for x in 0..CHUNK_DIM as u32 {
                dense[local_index(x, y, z) as usize] = 1;
            }
        }
    }
    let chunk = Svdag::from_dense(&dense);

    let grid = ProbeGrid::for_chunk(ProbeLod::Near);
    let volume = bake_svdag(&chunk, ChunkPos::new(2, -1, 5), grid);

    assert_eq!(volume.len(), BLOCKS_PER_CHUNK);
    let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
    assert_eq!(volume.len(), blocks_per_edge.pow(3));

    let invalid = blocks_per_edge * blocks_per_edge;
    let valid: u32 = volume
        .validity_bytes()
        .iter()
        .map(|byte| byte.count_ones())
        .sum();
    assert_eq!(valid as usize, volume.len() - invalid);

    // Every probe in the bottom block layer is invalid because its centre lies
    // inside the solid floor; the layer above is valid.
    for z in 0..blocks_per_edge {
        for x in 0..blocks_per_edge {
            let floor = x + z * blocks_per_edge * blocks_per_edge;
            let above = floor + blocks_per_edge;
            assert!(!volume.is_valid(floor), "floor probe ({x}, 0, {z})");
            assert!(volume.is_valid(above), "air probe ({x}, 1, {z})");
        }
    }
}
