//! M2-19: invalid-in-solid probes fall back to the nearest valid probe.
//!
//! ADR-005 §4 fixes the sampling rule this file pins:
//!
//! 1. sample the containing cell; if all eight corners are valid, blend them;
//! 2. if any corner is invalid, use the nearest valid probe in the 3×3×3 window
//!    around the sample, clamped to this chunk's grid, ties to the lowest linear
//!    index;
//! 3. if the window holds no valid probe, the contribution is zero;
//! 4. no path returns a NaN, including a degenerate normal or a position outside
//!    the chunk.

use voxel_core::constants::BLOCK_DIM;
use voxel_mesh::probe::{ProbeGrid, ProbeSample, ProbeVolume, SH_L0, SH_L1, SampleSource, bake};
use voxel_storage::ChunkPos;

/// 4-block edge: 16-voxel probe cells and 64 near probes.
const EDGE: usize = 4;

/// Probe cell edge in voxels, one block.
const CELL: f64 = BLOCK_DIM as f64;

/// The bottom block layer is solid, so every probe at `y = 0` is invalid.
fn floor_solid(_x: u32, y: u32, _z: u32) -> bool {
    y < BLOCK_DIM as u32
}

/// A wall at block `x = 2` from `y = 1` up, leaving the cells in front of it
/// with eight valid corners and directional SH variation.
fn wall_solid(x: u32, y: u32, _z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    y / block >= 1 && x / block == 2
}

/// A deterministic height field with a floating block.
fn terrain_solid(x: u32, y: u32, z: u32) -> bool {
    let block = BLOCK_DIM as u32;
    let (bx, by, bz) = (x / block, y / block, z / block);
    let height = 1 + (bx * 7 + bz * 13) % 3;
    by < height || (bx == 2 && by == 6 && bz == 6)
}

fn linear(x: usize, y: usize, z: usize, edge: usize) -> usize {
    x + y * edge + z * edge * edge
}

fn corners(x: usize, y: usize, z: usize, edge: usize) -> [usize; 8] {
    let mut out = [0usize; 8];
    let mut next = 0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                out[next] = linear(x + dx, y + dy, z + dz, edge);
                next += 1;
            }
        }
    }
    out
}

#[test]
fn probe_fallback_blends_a_cell_whose_eight_corners_are_valid() {
    let grid = ProbeGrid::near(EDGE);
    let volume = bake(ChunkPos::new(2, -1, 4), grid, wall_solid);
    let edge = grid.probes_per_edge();
    let (x, y, z) = find_varying_valid_cell(&volume, edge);

    // A sample at a probe's own centre returns that probe exactly: the cell
    // lattice is anchored on the probe centres, not on block boundaries.
    let probe_index = linear(x, y, z, edge);
    let at_probe = volume.sample(grid.centre(probe_index));
    assert_eq!(at_probe.source, SampleSource::Blend);
    assert_eq!(
        at_probe.coefficients,
        volume.probe(probe_index).coefficients()
    );

    // At the geometric centre of the eight probes every corner has weight 0.125.
    let centre = [
        (x + 1) as f64 * CELL,
        (y + 1) as f64 * CELL,
        (z + 1) as f64 * CELL,
    ];
    let sample = volume.sample(centre);
    assert_eq!(sample.source, SampleSource::Blend);

    let mut expected = [[0.0f64; 3]; 4];
    for index in corners(x, y, z, edge) {
        let record = volume.probe(index).coefficients();
        for (basis, values) in expected.iter_mut().zip(record) {
            for (channel, value) in basis.iter_mut().zip(values) {
                *channel += value * 0.125;
            }
        }
    }
    for (basis, expected) in sample.coefficients.iter().zip(expected) {
        for (channel, expected) in basis.iter().zip(expected) {
            assert!(
                (channel - expected).abs() < 1e-12,
                "blended channel {channel} missed {expected} at cell ({x}, {y}, {z})"
            );
        }
    }
}

#[test]
fn probe_fallback_probe_centres_return_their_own_record() {
    // The interpolation lattice is anchored on the probe centres. Every probe
    // that is a corner of a fully valid cell must be reproduced exactly when
    // sampled at its own centre; a block-boundary anchor would blend the
    // neighbouring probes instead.
    let grid = ProbeGrid::near(EDGE);
    let volume = bake(ChunkPos::new(-3, 2, 5), grid, wall_solid);
    let edge = grid.probes_per_edge();
    let mut checked = 0;

    for z in 0..edge - 1 {
        for y in 0..edge - 1 {
            for x in 0..edge - 1 {
                let indices = corners(x, y, z, edge);
                if !indices.iter().all(|&index| volume.is_valid(index)) {
                    continue;
                }
                for index in indices {
                    let sample = volume.sample(grid.centre(index));
                    assert_eq!(
                        sample.coefficients,
                        volume.probe(index).coefficients(),
                        "probe {index} was not reproduced at its own centre"
                    );
                    // A probe centre lies on the boundary between two cells; if
                    // the upper cell has an invalid corner the exact value still
                    // comes from this probe, through the zero-distance fallback.
                    let source_ok = match sample.source {
                        SampleSource::Blend => true,
                        SampleSource::Fallback { probe } => probe == index,
                        SampleSource::Zero => false,
                    };
                    assert!(source_ok, "probe {index} sampled as {:?}", sample.source);
                    checked += 1;
                }
            }
        }
    }

    assert!(checked > 0, "the fixture needs a fully valid cell");
}

/// Finds a cell with eight valid corners whose records are not all identical,
/// so the blend test cannot pass on a constant field.
fn find_varying_valid_cell(volume: &ProbeVolume, edge: usize) -> (usize, usize, usize) {
    for z in 0..edge - 1 {
        for y in 0..edge - 1 {
            for x in 0..edge - 1 {
                let indices = corners(x, y, z, edge);
                if !indices.iter().all(|&index| volume.is_valid(index)) {
                    continue;
                }
                let first = volume.probe(indices[0]).to_bytes();
                if indices[1..]
                    .iter()
                    .any(|&index| volume.probe(index).to_bytes() != first)
                {
                    return (x, y, z);
                }
            }
        }
    }
    panic!("the fixture needs a valid cell with varying corner records");
}

#[test]
fn probe_fallback_inside_a_wall_returns_the_nearest_valid_probe() {
    let grid = ProbeGrid::near(EDGE);
    let volume = bake(ChunkPos::new(0, 0, 0), grid, floor_solid);

    // Inside the solid floor, between blocks 1 and 2 in x and at the bottom.
    let position = [CELL, CELL / 2.0, CELL / 2.0];
    let sample = volume.sample(position);

    // Probes 4 (0, 1, 0) and 5 (1, 1, 0) are both 8 voxels away in x and 16 in
    // y; the tie must break to the lower index.
    assert_eq!(sample.source, SampleSource::Fallback { probe: 4 });
    assert!(volume.is_valid(4));
    assert_eq!(sample.coefficients, volume.probe(4).coefficients());
    assert_eq!(volume.nearest_valid(position), Some(4));

    // The chosen probe is the nearest valid one in the 3×3×3 window.
    let brute_force = nearest_in_window(&volume, position);
    assert_eq!(Some(4), brute_force);

    // A sample in the air above the floor also uses the containing cell; its
    // corners are all valid there, so it blends instead.
    let above = volume.sample([CELL, 2.0 * CELL, CELL / 2.0]);
    assert_eq!(above.source, SampleSource::Blend);
}

/// Independent nearest-valid search over the 3×3×3 window around the sample.
fn nearest_in_window(volume: &ProbeVolume, position: [f64; 3]) -> Option<usize> {
    let grid = volume.grid();
    let edge = grid.probes_per_edge() as i32;
    let cell_voxels = (grid.spacing_blocks() * BLOCK_DIM) as f64;
    let half = (BLOCK_DIM / 2) as f64;
    let mut window = [0i32; 3];
    for axis in 0..3 {
        let raw = (position[axis] - half) / cell_voxels;
        let base = raw.floor().clamp(0.0, (edge - 1) as f64) as i32;
        let fraction = (raw - base as f64).clamp(0.0, 1.0);
        window[axis] = if fraction < 0.5 {
            base
        } else {
            (base + 1).min(edge - 1)
        };
    }
    let mut best: Option<(f64, usize)> = None;

    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let x = (window[0] + dx).clamp(0, edge - 1) as usize;
                let y = (window[1] + dy).clamp(0, edge - 1) as usize;
                let z = (window[2] + dz).clamp(0, edge - 1) as usize;
                let index = linear(x, y, z, edge as usize);
                if !volume.is_valid(index) {
                    continue;
                }
                let centre = grid.centre(index);
                let distance: f64 = (0..3)
                    .map(|axis| (centre[axis] - position[axis]).powi(2))
                    .sum();
                let better = match best {
                    None => true,
                    Some((best_distance, best_index)) => {
                        distance < best_distance
                            || (distance == best_distance && index < best_index)
                    }
                };
                if better {
                    best = Some((distance, index));
                }
            }
        }
    }

    best.map(|(_, index)| index)
}

#[test]
fn probe_fallback_returns_zero_when_every_probe_is_solid() {
    let grid = ProbeGrid::near(2);
    let volume = bake(ChunkPos::new(1, 1, 1), grid, |_, _, _| true);

    assert!((0..volume.len()).all(|index| !volume.is_valid(index)));
    assert_eq!(volume.nearest_valid([CELL, CELL, CELL]), None);

    let sample = volume.sample([CELL, CELL, CELL]);
    assert_eq!(sample.source, SampleSource::Zero);
    assert_eq!(sample.coefficients, [[0.0; 3]; 4]);
    assert_eq!(sample.radiance([0.0, 1.0, 0.0]), [0.0; 3]);
}

#[test]
fn probe_fallback_mid_window_finds_the_nearer_probe() {
    // mid(8) puts four probes per edge at voxels 8, 40, 72 and 104. Only the
    // x = 8 and x = 104 columns are valid, so a sample at x = 63 must pick the
    // x = 104 probe (41 voxels away), not x = 8 (55 away). A window around the
    // containing cell alone would have missed probe 3.
    let grid = ProbeGrid::mid(8);
    let block = BLOCK_DIM as u32;
    let solid = |x: u32, _y: u32, _z: u32| matches!(x / block, 2 | 4);
    let volume = bake(ChunkPos::new(0, 0, 0), grid, solid);

    assert_eq!(grid.probes_per_edge(), 4);
    assert!(volume.is_valid(0), "x = 8 is air");
    assert!(!volume.is_valid(1), "x = 40 is solid");
    assert!(volume.is_valid(3), "x = 104 is air");

    let position = [63.0, 8.0, 8.0];
    assert_eq!(volume.nearest_valid(position), Some(3));
    assert_eq!(
        volume.sample(position).source,
        SampleSource::Fallback { probe: 3 }
    );
}

#[test]
fn probe_fallback_does_not_search_past_the_window() {
    // Every block with all coordinates <= 2 is solid, so all 27 probes in the
    // window around (16, 8, 8) are invalid. Probe 3 at block (3, 0, 0) is valid
    // but outside the window: the sample must contribute zero, not reach for it.
    let block = BLOCK_DIM as u32;
    let solid = |x: u32, y: u32, z: u32| x / block <= 2 && y / block <= 2 && z / block <= 2;
    let grid = ProbeGrid::near(EDGE);
    let volume = bake(ChunkPos::new(0, 0, 0), grid, solid);
    assert!(volume.is_valid(linear(3, 0, 0, 4)));

    let position = [CELL, CELL / 2.0, CELL / 2.0];
    assert_eq!(volume.nearest_valid(position), None);
    assert_eq!(volume.sample(position), ProbeSample::ZERO);
}

#[test]
fn probe_fallback_far_grid_has_no_probes_to_sample() {
    let grid = ProbeGrid::far(EDGE);
    let volume = bake(ChunkPos::new(0, 0, 0), grid, |_, _, _| false);

    assert!(volume.is_empty());
    assert_eq!(volume.nearest_valid([CELL, CELL, CELL]), None);
    assert_eq!(volume.sample([CELL, CELL, CELL]), ProbeSample::ZERO);
}

#[test]
fn probe_fallback_never_returns_a_nan() {
    let volume = bake(
        ChunkPos::new(-2, 3, 0),
        ProbeGrid::near(EDGE),
        terrain_solid,
    );
    let scene = EDGE as f64 * CELL;

    let positions = [
        [0.0, 0.0, 0.0],
        [scene * 0.5, scene * 0.5, scene * 0.5],
        [scene - 0.001, scene - 0.001, scene - 0.001],
        [scene + 5.0, scene + 5.0, scene + 5.0],
        [-5.0, -5.0, -5.0],
        [f64::NAN, CELL, CELL],
        [f64::INFINITY, CELL, CELL],
        [f64::NEG_INFINITY, CELL, CELL],
        [CELL, CELL, f64::NAN],
    ];
    let normals = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, -1.0, 1.0],
        [f64::NAN, 0.0, 0.0],
        [f64::INFINITY, 0.0, 0.0],
        [1e308, 1e308, 1e308],
        [f64::MIN_POSITIVE, 0.0, 0.0],
    ];

    for position in positions {
        let sample = volume.sample(position);
        for coefficient in sample.coefficients {
            for channel in coefficient {
                assert!(channel.is_finite(), "position {position:?} has {channel}");
            }
        }
        for normal in normals {
            let radiance = sample.radiance(normal);
            for channel in radiance {
                assert!(
                    channel.is_finite(),
                    "position {position:?}, normal {normal:?} has {channel}"
                );
            }
        }
    }
}

#[test]
fn probe_fallback_radiance_uses_the_sh4_basis() {
    let volume = bake(ChunkPos::new(0, 0, 0), ProbeGrid::near(2), |_, _, _| false);
    let sample = volume.sample([CELL, CELL, CELL]);
    assert_eq!(sample.source, SampleSource::Blend);

    let normal = [0.0, 0.6, -0.8];
    let basis = [SH_L0, SH_L1 * 0.6, SH_L1 * -0.8, 0.0];
    let mut expected = [0.0f64; 3];
    for (coefficient, weight) in sample.coefficients.iter().zip(basis) {
        for (channel, value) in expected.iter_mut().zip(coefficient) {
            *channel += value * weight;
        }
    }
    let radiance = sample.radiance(normal);
    for (channel, expected) in radiance.iter().zip(expected) {
        assert!(
            (channel - expected).abs() < 1e-12,
            "radiance {channel} missed {expected}"
        );
    }

    // A degenerate or non-finite normal is zero, never a NaN.
    assert_eq!(sample.radiance([0.0, 0.0, 0.0]), [0.0; 3]);
    assert_eq!(sample.radiance([f64::NAN; 3]), [0.0; 3]);
    assert_eq!(sample.radiance([f64::INFINITY; 3]), [0.0; 3]);
}

#[test]
fn probe_fallback_clamps_samples_to_the_chunk_grid() {
    let volume = bake(ChunkPos::new(0, 0, 0), ProbeGrid::near(2), |_, _, _| false);
    let edge = volume.grid().probes_per_edge();
    let scene = volume.grid().scene_edge_voxels() as f64;
    let last = linear(edge - 1, edge - 1, edge - 1, edge);

    // At or beyond the far edge every corner clamps to the last probe. The
    // blend of identical corners differs from the record by at most rounding.
    for (position, expected) in [
        ([scene; 3], last),
        ([scene + 100.0; 3], last),
        ([-100.0; 3], linear(0, 0, 0, edge)),
    ] {
        let sample = volume.sample(position);
        assert_eq!(sample.source, SampleSource::Blend);
        let expected = volume.probe(expected).coefficients();
        for (basis, expected) in sample.coefficients.iter().zip(expected) {
            for (channel, expected) in basis.iter().zip(expected) {
                assert!(
                    (channel - expected).abs() < 1e-12,
                    "position {position:?}: {channel} missed {expected}"
                );
            }
        }
    }
}
