//! Block-light flood-fill reference tests (M1-29).
//!
//! Three hand-computed fixtures pin the two channels and the attenuation rule:
//!
//! 1. **open sky** — a floor under an open column, where every exposed air cell
//!    seeds skylight 15 and the floor stays dark;
//! 2. **cave** — a roofed side arm lit only through a shaft, so skylight falls
//!    by one level per air step and cannot cross the opaque roof;
//! 3. **torch** — an isolated emitter in open air, whose block channel is
//!    exactly `15 - Manhattan distance`.
//!
//! The remaining tests cover the apron on all six faces, opaque blocking, the
//! current compiled chunk edge, deterministic repeatability, and a fixed-seed
//! comparison against an independent relaxation solver.

use std::mem::size_of;

use voxel_core::content_constants::{
    LIGHT_BYTES_PER_CHUNK, LIGHT_CELLS_PER_CHUNK, LIGHT_CELLS_PER_EDGE,
};
use voxel_storage::reference::light::{
    APRON_CELLS, BlockLight, Channel, LightCell, MAX_LEVEL, NEIGHBOURS, full_pass,
};

/// Edge used by the literal hand-computed fixtures.
///
/// It matches the 64³ chunk (four block cells per edge) regardless of the
/// `chunk128` feature, so the literal tables stay stable. A separate test
/// covers the compiled edge.
const HAND_EDGE: usize = 4;

/// Hand-computed block levels for one torch at `(1, 1, 1)` in open air.
///
/// Indexed `[z][y][x]`; each value is `15 - |x - 1| - |y - 1| - |z - 1|`.
const TORCH_LEVELS: [[[u8; HAND_EDGE]; HAND_EDGE]; HAND_EDGE] = [
    // z = 0
    [
        [12, 13, 12, 11],
        [13, 14, 13, 12],
        [12, 13, 12, 11],
        [11, 12, 11, 10],
    ],
    // z = 1
    [
        [13, 14, 13, 12],
        [14, 15, 14, 13],
        [13, 14, 13, 12],
        [12, 13, 12, 11],
    ],
    // z = 2
    [
        [12, 13, 12, 11],
        [13, 14, 13, 12],
        [12, 13, 12, 11],
        [11, 12, 11, 10],
    ],
    // z = 3
    [
        [11, 12, 11, 10],
        [12, 13, 12, 11],
        [11, 12, 11, 10],
        [10, 11, 10, 9],
    ],
];

#[test]
fn hand_computed_open_sky() {
    let fill = full_pass(
        HAND_EDGE,
        |_, y, _| {
            if y <= 0 {
                BlockLight::OPAQUE
            } else {
                BlockLight::AIR
            }
        },
        |_, y, _| y > 0,
    );

    for (x, y, z) in local_coords(HAND_EDGE) {
        let cell = fill.local(x, y, z);
        let expected = if y == 0 { 0 } else { MAX_LEVEL };
        assert_eq!(cell.sky_level(), expected, "open sky at ({x}, {y}, {z})");
        assert_eq!(
            cell.block_level(),
            0,
            "no emissive block at ({x}, {y}, {z})"
        );
    }

    // The apron above the chunk is exposed and seeds 15; the floor apron is
    // opaque and stays dark.
    assert_eq!(fill.with_apron(0, 4, 0).sky_level(), MAX_LEVEL);
    assert_eq!(fill.with_apron(0, -1, 0).sky_level(), 0);
}

#[test]
fn hand_computed_cave() {
    // A vertical shaft at x = z = 0 from y = 1..=3, and a roofed side arm at
    // y = 1, z = 0, x = 1..=3. Every other cell is opaque.
    let fill = full_pass(
        HAND_EDGE,
        |x, y, z| {
            let shaft = x == 0 && z == 0 && (1..=3).contains(&y);
            let arm = y == 1 && z == 0 && (1..=3).contains(&x);
            if shaft || arm {
                BlockLight::AIR
            } else {
                BlockLight::OPAQUE
            }
        },
        |x, y, z| x == 0 && z == 0 && (1..=4).contains(&y),
    );

    // Expected sky levels in the shaft and its side arm. The arm loses one
    // level per air step; the opaque roof above it admits nothing.
    let expected = [
        ((0, 1, 0), 15),
        ((0, 2, 0), 15),
        ((0, 3, 0), 15),
        ((1, 1, 0), 14),
        ((2, 1, 0), 13),
        ((3, 1, 0), 12),
    ];

    for (x, y, z) in local_coords(HAND_EDGE) {
        let cell = fill.local(x, y, z);
        let sky = expected
            .iter()
            .find_map(|&((ex, ey, ez), level)| (x == ex && y == ey && z == ez).then_some(level))
            .unwrap_or(0);
        assert_eq!(cell.sky_level(), sky, "cave sky at ({x}, {y}, {z})");
        assert_eq!(
            cell.block_level(),
            0,
            "no emitter in the cave at ({x}, {y}, {z})"
        );
    }
}

#[test]
fn hand_computed_torch() {
    let fill = full_pass(
        HAND_EDGE,
        |x, y, z| {
            if (x, y, z) == (1, 1, 1) {
                BlockLight::emitter(MAX_LEVEL)
            } else {
                BlockLight::AIR
            }
        },
        |_, _, _| false,
    );

    for (x, y, z) in local_coords(HAND_EDGE) {
        let cell = fill.local(x, y, z);
        assert_eq!(
            cell.block_level(),
            TORCH_LEVELS[z][y][x],
            "torch block light at ({x}, {y}, {z})"
        );
        assert_eq!(cell.sky_level(), 0, "no sky source at ({x}, {y}, {z})");
    }
}

#[test]
fn apron_reads_one_block_on_every_face() {
    let last = HAND_EDGE as i32 - 1;
    let cases = [
        ((-1, 1, 1), (0, 1, 1)),
        ((HAND_EDGE as i32, 1, 1), (last as usize, 1, 1)),
        ((1, -1, 1), (1, 0, 1)),
        ((1, HAND_EDGE as i32, 1), (1, last as usize, 1)),
        ((1, 1, -1), (1, 1, 0)),
        ((1, 1, HAND_EDGE as i32), (1, 1, last as usize)),
    ];

    for (source, target) in cases {
        let fill = full_pass(
            HAND_EDGE,
            |x, y, z| {
                if (x, y, z) == source {
                    BlockLight::emitter(MAX_LEVEL)
                } else {
                    BlockLight::AIR
                }
            },
            |_, _, _| false,
        );
        assert_eq!(
            fill.local(target.0, target.1, target.2).block_level(),
            MAX_LEVEL - 1,
            "light from apron cell {source:?} reaches {target:?}"
        );
    }
}

#[test]
fn apron_is_exactly_one_block_wide() {
    for source in [(-2, 1, 1), (HAND_EDGE as i32 + 1, 1, 1)] {
        let fill = full_pass(
            HAND_EDGE,
            |x, y, z| {
                if (x, y, z) == source {
                    BlockLight::emitter(MAX_LEVEL)
                } else {
                    BlockLight::AIR
                }
            },
            |_, _, _| false,
        );
        assert_eq!(
            fill.local(0, 1, 1).block_level(),
            0,
            "a source at {source:?} is outside the one-block apron"
        );
    }
}

#[test]
fn apron_imports_sky_from_a_neighbour() {
    let fill = full_pass(
        HAND_EDGE,
        |x, y, z| {
            if (x, y, z) == (-1, 1, 1) || (x, y, z) == (0, 1, 1) {
                BlockLight::AIR
            } else {
                BlockLight::OPAQUE
            }
        },
        |x, y, z| (x, y, z) == (-1, 1, 1),
    );

    assert_eq!(fill.local(0, 1, 1).sky_level(), MAX_LEVEL - 1);
    assert_eq!(fill.local(1, 1, 1).sky_level(), 0);
}

#[test]
fn opaque_blocks_stop_the_fill() {
    // A torch, a full opaque wall at x = 1, and open air behind the wall. The
    // wall spans the padded volume, so there is no path around it. The wall
    // cell and everything beyond it stay dark; the air beside the torch is
    // lit.
    let fill = full_pass(
        HAND_EDGE,
        |x, y, z| match (x, y, z) {
            (0, 1, 1) => BlockLight::emitter(MAX_LEVEL),
            _ if x == 1 => BlockLight::OPAQUE,
            _ => BlockLight::AIR,
        },
        |_, _, _| false,
    );

    assert_eq!(fill.local(0, 0, 1).block_level(), MAX_LEVEL - 1);
    assert_eq!(fill.local(1, 1, 1).block_level(), 0);
    assert_eq!(fill.local(2, 1, 1).block_level(), 0);
    assert_eq!(fill.local(3, 1, 1).block_level(), 0);
}

#[test]
fn cell_layout_is_the_sixteen_byte_contract() {
    assert_eq!(size_of::<Channel>(), 8);
    assert_eq!(size_of::<LightCell>(), 16);
    assert_eq!(
        size_of::<LightCell>() * LIGHT_CELLS_PER_CHUNK,
        LIGHT_BYTES_PER_CHUNK
    );
    assert_eq!(APRON_CELLS, 1);
}

#[test]
fn fill_covers_the_compiled_chunk_edge() {
    let edge = LIGHT_CELLS_PER_EDGE;
    assert_eq!(edge * edge * edge, LIGHT_CELLS_PER_CHUNK);

    let fill = full_pass(edge, |_, _, _| BlockLight::AIR, |_, _, _| true);
    assert_eq!(fill.edge_cells(), edge);
    assert_eq!(fill.padded_edge_cells(), edge + 2);
    assert_eq!(fill.padded_cells().len(), (edge + 2).pow(3));

    for (x, y, z) in local_coords(edge) {
        assert_eq!(
            fill.local(x, y, z).sky_level(),
            MAX_LEVEL,
            "all-open sky at ({x}, {y}, {z})"
        );
    }
}

#[test]
fn repeated_fills_are_byte_identical() {
    let block_at = |x: i32, y: i32, z: i32| match (x, y, z) {
        (1, 2, 1) => BlockLight::OPAQUE,
        (2, 1, 2) => BlockLight::OPAQUE,
        (3, 1, 3) => BlockLight::emitter(9),
        (0, 2, 1) => BlockLight::new(0, 4),
        _ if y <= 0 => BlockLight::OPAQUE,
        _ => BlockLight::AIR,
    };
    let sky_at = |_, y: i32, z: i32| y > 0 && z != 2;

    let first = full_pass(HAND_EDGE, block_at, sky_at);
    for run in 1..100 {
        let next = full_pass(HAND_EDGE, block_at, sky_at);
        assert_eq!(next, first, "fill {run} differs from the first run");
    }
}

#[test]
fn matches_fixed_point_relaxation() {
    // A fixed-seed matrix of awkward cases: opaque cells, translucent cells,
    // multiple emitters, and sky seeds that do and do not reach the chunk.
    // The relaxation solver updates every padded cell in place until it stops
    // changing, visiting neighbours in the reverse of the reference order.
    let padded = HAND_EDGE + 2;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;

    for case in 0..32 {
        let mut blocks = vec![BlockLight::AIR; padded.pow(3)];
        let mut sky_seed = vec![false; padded.pow(3)];

        for index in 0..blocks.len() {
            let draw = next(&mut state);
            let opacity = if draw & 0x3 == 0 {
                MAX_LEVEL
            } else {
                ((draw >> 2) % 4) as u8
            };
            let emission = if (draw >> 8).is_multiple_of(13) {
                1 + ((draw >> 16) % u64::from(MAX_LEVEL)) as u8
            } else {
                0
            };
            blocks[index] = BlockLight::new(emission, opacity);
            sky_seed[index] = (draw >> 24).is_multiple_of(5);
        }

        let sample = |x: i32, y: i32, z: i32| blocks[padded_index(x, y, z, padded)];
        let exposed = |x: i32, y: i32, z: i32| sky_seed[padded_index(x, y, z, padded)];

        let fill = full_pass(HAND_EDGE, sample, exposed);
        let relaxed = relax(HAND_EDGE, &blocks, &sky_seed);
        assert_eq!(
            fill.padded_cells(),
            relaxed.as_slice(),
            "random case {case} differs from relaxation"
        );
    }
}

/// Every chunk-local coordinate in `z, y, x` order.
fn local_coords(edge: usize) -> impl Iterator<Item = (usize, usize, usize)> {
    (0..edge).flat_map(move |z| (0..edge).flat_map(move |y| (0..edge).map(move |x| (x, y, z))))
}

fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn padded_index(x: i32, y: i32, z: i32, padded: usize) -> usize {
    let px = (x + APRON_CELLS) as usize;
    let py = (y + APRON_CELLS) as usize;
    let pz = (z + APRON_CELLS) as usize;
    px + py * padded + pz * padded * padded
}

fn padded_coords(index: usize, padded: usize) -> (i32, i32, i32) {
    let x = (index % padded) as i32 - APRON_CELLS;
    let y = ((index / padded) % padded) as i32 - APRON_CELLS;
    let z = (index / (padded * padded)) as i32 - APRON_CELLS;
    (x, y, z)
}

/// Independent solver: seed both channels, then relax every cell to the best
/// value any already-lit neighbour can give it until nothing changes.
fn relax(edge: usize, blocks: &[BlockLight], sky_seed: &[bool]) -> Vec<LightCell> {
    let padded = edge + 2;
    let mut sky = vec![0u8; blocks.len()];
    let mut block = vec![0u8; blocks.len()];

    for index in 0..blocks.len() {
        if blocks[index].emission > 0 {
            block[index] = blocks[index].emission;
        }
        if !blocks[index].is_fully_opaque() && sky_seed[index] {
            sky[index] = MAX_LEVEL;
        }
    }

    relax_channel(blocks, &mut sky, padded);
    relax_channel(blocks, &mut block, padded);

    sky.into_iter()
        .zip(block)
        .map(|(sky, block)| LightCell {
            sky: Channel::from_level(sky),
            block: Channel::from_level(block),
        })
        .collect()
}

fn relax_channel(blocks: &[BlockLight], levels: &mut [u8], padded: usize) {
    let lower = -APRON_CELLS;
    let upper = padded as i32 - APRON_CELLS - 1;

    loop {
        let mut changed = false;
        for index in 0..levels.len() {
            let (x, y, z) = padded_coords(index, padded);
            let mut best = levels[index];

            // Reverse order on purpose: a correct fixed point cannot depend on
            // the order in which neighbours are visited.
            for &(dx, dy, dz) in NEIGHBOURS.iter().rev() {
                let nx = x + dx;
                let ny = y + dy;
                let nz = z + dz;
                if nx < lower || nx > upper || ny < lower || ny > upper || nz < lower || nz > upper
                {
                    continue;
                }
                let neighbour = padded_index(nx, ny, nz, padded);
                let candidate = levels[neighbour].saturating_sub(1 + blocks[index].opacity);
                best = best.max(candidate);
            }

            if best > levels[index] {
                levels[index] = best;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}
