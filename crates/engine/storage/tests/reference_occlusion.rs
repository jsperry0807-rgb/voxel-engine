//! Block-occlusion flood-fill reference tests (M1-33).
//!
//! The acceptance criterion is narrow and asymmetric: an enclosed cave is
//! occluded, an open field is not, and on **100 camera poses** the fill never
//! marks a block occluded when the camera can actually see it. The last point
//! is checked against line of sight, which is strictly stronger than the
//! connectivity the fill promises; a block visible along any clear segment must
//! therefore be in the fill's visible set.
//!
//! The remaining tests pin the mechanics the invariant rests on:
//!
//! 1. the camera block is the floor of the world position, and the seed is
//!    always visible — even inside solid;
//! 2. opaque blocks adjacent to reached space stay visible, while enclosed
//!    air behind them is culled;
//! 3. a one-block gap carries the fill around a wall even when line of sight is
//!    blocked (connectivity, not line of sight);
//! 4. blocks beyond the horizon are conservatively visible;
//! 5. one bit per block packs into `BLOCK_OCCLUSION_BYTES` per chunk.
//!
//! The pose test also compares the fill against an independent relaxation
//! solver, so an over-eager or missing expansion is caught exactly, not just by
//! the weaker line-of-sight oracle.

use voxel_core::constants::{BLOCK_DIM, BLOCKS_PER_CHUNK, CHUNK_DIM};
use voxel_core::content_constants::{BLOCK_OCCLUSION_BYTES, OCCLUSION_HORIZON_M};
use voxel_storage::ChunkPos;
use voxel_storage::reference::cull::Vec3;
use voxel_storage::reference::occlusion::{
    BLOCK_EDGE_M, DEFAULT_HORIZON_BLOCKS, NEIGHBOURS, camera_block, flood_fill,
    flood_fill_from_block,
};

/// Poses required by M1-33.
const POSES: usize = 100;

/// Small horizon used by the pose tests.
///
/// The production horizon is 128 blocks (a 257³ cube); a sweep of 100 poses
/// over that volume belongs in a benchmark, not a unit test. The semantics are
/// identical at any horizon.
const TEST_HORIZON: u32 = 8;

#[test]
fn camera_block_is_the_floor_of_the_world_position() {
    assert_eq!(BLOCK_EDGE_M, 1.0, "a block is one metre");
    assert_eq!(DEFAULT_HORIZON_BLOCKS, OCCLUSION_HORIZON_M);
    assert_eq!(DEFAULT_HORIZON_BLOCKS, 128);

    assert_eq!(camera_block(Vec3::new(0.0, 0.0, 0.0)), [0, 0, 0]);
    assert_eq!(camera_block(Vec3::new(0.25, 0.999, 1.0)), [0, 0, 1]);
    assert_eq!(camera_block(Vec3::new(-0.001, -1.0, -1.5)), [-1, -1, -2]);
    assert_eq!(camera_block(Vec3::new(15.9, -16.0, 31.99)), [15, -16, 31]);

    let fill = flood_fill(Vec3::new(2.5, -3.25, 7.75), 2, |_, _, _| false);
    assert_eq!(fill.seed(), [2, -4, 7]);
    assert_eq!(fill.horizon_blocks(), 2);
    assert_eq!(fill.side(), 5);
    assert_eq!(fill.origin(), [0, -6, 5]);
}

#[test]
fn enclosed_cave_is_occluded_but_its_outer_wall_is_drawn() {
    // A solid 7³ shell with a sealed 3³ cave in the middle. The camera sits in
    // open air outside the shell, so the cave has no non-opaque path to it.
    let scene = |x: i32, y: i32, z: i32| {
        let shell = x.abs() <= 3 && y.abs() <= 3 && z.abs() <= 3;
        let cave = x.abs() <= 1 && y.abs() <= 1 && z.abs() <= 1;
        shell && !cave
    };

    let fill = flood_fill_from_block([-8, 0, 0], 12, scene);

    for cave_block in [[0, 0, 0], [1, 1, 1], [-1, 0, 1]] {
        assert!(
            fill.is_occluded(cave_block),
            "enclosed cave block {cave_block:?} must be occluded"
        );
    }

    // The shell's outer face is adjacent to reached air and must survive.
    for wall_block in [[-3, 0, 0], [3, 0, 0], [0, 3, 0], [0, 0, -3]] {
        assert!(
            fill.is_visible(wall_block),
            "outer wall block {wall_block:?} must be visible"
        );
    }

    // The inner shell only touches other solids and the unreachable cave.
    assert!(fill.is_occluded([-2, 0, 0]));
    assert!(fill.is_occluded([2, 0, 0]));

    // The camera's own block and the air around the shell are visible.
    assert!(fill.is_visible([-8, 0, 0]));
    assert!(fill.is_visible([-4, 0, 0]));

    // Beyond the horizon everything resolves visible, even with no path.
    assert!(fill.is_visible([-8 + 13, 0, 0]));
}

#[test]
fn open_field_is_not_occluded() {
    let fill = flood_fill_from_block([0, 0, 0], 5, |_, _, _| false);
    let side = fill.side();
    assert_eq!(side, 11);
    assert_eq!(fill.reached_blocks(), side.pow(3));
    assert_eq!(fill.visible_count(), side.pow(3));
    assert_eq!(fill.occluded_count(), 0);

    for block in fill.visible_blocks() {
        assert!(fill.is_visible(block));
    }
    assert_eq!(fill.occluded_blocks().count(), 0);

    // Outside the horizon the query is deliberately conservative.
    assert!(fill.is_visible([6, 0, 0]));
    assert!(fill.is_visible([-100, 50, 200]));

    // Same input, same bits: no map/thread order participates.
    let again = flood_fill_from_block([0, 0, 0], 5, |_, _, _| false);
    assert_eq!(again, fill);
}

#[test]
fn one_block_gap_carries_visibility_around_a_wall() {
    // A wall at x = 2 with one gap at (2, 3, 0). The target at (4, 0, 0) is
    // hidden from the camera by line of sight, but a 6-connected path goes
    // through the gap. The fill must keep it visible: connectivity, not
    // line-of-sight, is the ADR-006 decision.
    let scene = |x: i32, y: i32, z: i32| {
        x == 2 && (-5..=5).contains(&y) && (-5..=5).contains(&z) && !(y == 3 && z == 0)
    };

    let fill = flood_fill_from_block([0, 0, 0], 8, scene);
    assert!(
        fill.is_visible([4, 0, 0]),
        "the fill must reach the far side through the gap"
    );
    assert!(
        !line_of_sight_clear([0, 0, 0], [4, 0, 0], scene),
        "the fixture must actually block the straight segment"
    );

    // The wall face is visible even though it is opaque.
    assert!(fill.is_visible([2, 0, 0]));
    assert!(!fill.is_occluded([4, 0, 0]));
}

#[test]
fn horizon_is_inclusive_and_outside_is_conservatively_visible() {
    // All solid, camera in one solid block: only the seed and its six face
    // neighbours can be reached. Everything else inside the cube is occluded.
    let fill = flood_fill_from_block([0, 0, 0], 4, |_, _, _| true);

    assert!(fill.is_visible([0, 0, 0]));
    assert_eq!(fill.reached_blocks(), 1);
    for (dx, dy, dz) in NEIGHBOURS {
        assert!(fill.is_visible([dx, dy, dz]));
    }

    assert!(fill.is_within_horizon([4, 4, 4]));
    assert!(fill.is_occluded([2, 0, 0]));
    assert!(fill.is_occluded([4, 4, 4]));
    assert_eq!(fill.occluded_count(), 9usize.pow(3) - 7);

    // Horizon + 1 on an axis is outside the cube.
    assert!(!fill.is_within_horizon([5, 0, 0]));
    assert!(fill.is_visible([5, 0, 0]));
    assert!(fill.is_visible([-5, 0, 0]));
    assert!(fill.is_visible([i32::MAX, i32::MIN, 0]));
}

#[test]
fn opaque_seed_is_still_visible_and_seeds_its_neighbourhood() {
    // A camera embedded in solid rock is the ambiguous seed policy. The seed
    // and its face neighbourhood resolve visible; the rest stays occluded.
    let fill = flood_fill_from_block([10, -2, 5], 3, |_, _, _| true);

    assert_eq!(fill.reached_blocks(), 1);
    assert!(fill.is_visible([10, -2, 5]));
    for (dx, dy, dz) in NEIGHBOURS {
        assert!(fill.is_visible([10 + dx, -2 + dy, 5 + dz]));
    }
    assert!(fill.is_occluded([12, -2, 5]));
}

#[test]
fn chunk_bits_pack_one_bit_per_block_in_the_block_table_order() {
    let fill = flood_fill_from_block([0, 0, 0], 16, |_, _, _| true);
    let chunk = ChunkPos::new(0, 0, 0);
    let bytes = fill.chunk_bits(chunk);
    assert_eq!(bytes.len(), BLOCK_OCCLUSION_BYTES);

    let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
    let bit = |x: usize, y: usize, z: usize| {
        assert!(x < blocks_per_edge && y < blocks_per_edge && z < blocks_per_edge);
        let index = x + y * blocks_per_edge + z * blocks_per_edge * blocks_per_edge;
        bytes[index / 8] & (1 << (index % 8)) != 0
    };

    // The seed is local block (0, 0, 0); the surface neighbourhood follows.
    assert!(bit(0, 0, 0));
    assert!(bit(1, 0, 0));
    // Inside the horizon but solid and unreached.
    assert!(!bit(2, 2, 2));

    // A chunk entirely beyond the horizon is conservatively all-visible.
    let far = fill.chunk_bits(ChunkPos::new(100, 100, 100));
    assert!(far.iter().all(|byte| *byte == 0xFF));

    // The packed chunk must agree with the global query on every block.
    let mut expected = 0usize;
    for lz in 0..blocks_per_edge {
        for ly in 0..blocks_per_edge {
            for lx in 0..blocks_per_edge {
                let global = [lx as i32, ly as i32, lz as i32];
                let visible = fill.is_visible(global);
                assert_eq!(bit(lx, ly, lz), visible, "block {global:?}");
                expected += usize::from(visible);
            }
        }
    }
    let packed: usize = bytes.iter().map(|byte| byte.count_ones() as usize).sum();
    assert_eq!(packed, expected);
    assert!(
        packed < BLOCKS_PER_CHUNK,
        "the fixture must include occluded blocks"
    );
}

/// On 100 poses, the fill's visible set exactly equals an independent
/// relaxation solver over the same cube.
#[test]
fn fill_equals_an_independent_relaxation_on_one_hundred_poses() {
    let mut occluded_total = 0usize;

    for (pose, seed) in pose_seeds().into_iter().enumerate() {
        assert!(
            !scene_opaque(seed[0], seed[1], seed[2]),
            "pose {pose} must start in air"
        );

        let fill = flood_fill_from_block(seed, TEST_HORIZON, scene_opaque);
        let (expected, expected_reached) = relaxation(seed, TEST_HORIZON, scene_opaque);
        let side = fill.side();

        for lz in 0..side {
            for ly in 0..side {
                for lx in 0..side {
                    let block = [
                        fill.origin()[0] + lx as i32,
                        fill.origin()[1] + ly as i32,
                        fill.origin()[2] + lz as i32,
                    ];
                    let index = lx + ly * side + lz * side * side;
                    assert_eq!(
                        fill.is_visible(block),
                        expected[index],
                        "pose {pose}, block {block:?}"
                    );
                }
            }
        }

        assert_eq!(
            fill.reached_blocks(),
            expected_reached,
            "pose {pose}: traversed volume differs"
        );
        occluded_total += fill.occluded_count();
    }

    assert!(occluded_total > 0, "the scene never produced an occlusion");
    println!("reference_occlusion: {POSES} poses, {occluded_total} occluded blocks");
}

/// On 100 poses, every block with a clear line of sight is visible to the fill.
#[test]
fn line_of_sight_visible_blocks_are_never_occluded_on_one_hundred_poses() {
    let mut visible_total = 0usize;
    let mut occluded_total = 0usize;

    for (pose, seed) in pose_seeds().into_iter().enumerate() {
        let fill = flood_fill_from_block(seed, TEST_HORIZON, scene_opaque);
        let side = fill.side();

        for lz in 0..side {
            for ly in 0..side {
                for lx in 0..side {
                    let block = [
                        fill.origin()[0] + lx as i32,
                        fill.origin()[1] + ly as i32,
                        fill.origin()[2] + lz as i32,
                    ];
                    if fill.is_visible(block) {
                        visible_total += 1;
                        continue;
                    }
                    occluded_total += 1;
                    assert!(
                        !line_of_sight_clear(seed, block, scene_opaque),
                        "pose {pose} marked the line-of-sight-visible block {block:?} occluded"
                    );
                }
            }
        }
    }

    assert!(visible_total > 0, "no pose saw any block");
    assert!(occluded_total > 0, "no pose exercised an occlusion");
    println!(
        "reference_occlusion: {POSES} poses, {visible_total} visible / \
         {occluded_total} occluded block samples"
    );
}

/// Builds the visible set by relaxation, with no queue and no flood order.
///
/// The solver repeatedly sweeps the cube until the reachable non-opaque set
/// stops growing, then adds the face neighbours of every reached cell. That is
/// the same fixed point as the fill but reached by a different algorithm.
fn relaxation(
    seed: [i32; 3],
    horizon: u32,
    is_opaque: impl Fn(i32, i32, i32) -> bool,
) -> (Vec<bool>, usize) {
    let side = (2 * horizon + 1) as usize;
    let origin = [
        seed[0] - horizon as i32,
        seed[1] - horizon as i32,
        seed[2] - horizon as i32,
    ];
    let mut reachable = vec![false; side * side * side];
    reachable[index_in_cube(seed, origin, side)] = true;

    loop {
        let mut changed = false;
        for lz in 0..side {
            for ly in 0..side {
                for lx in 0..side {
                    let cell = [
                        origin[0] + lx as i32,
                        origin[1] + ly as i32,
                        origin[2] + lz as i32,
                    ];
                    let index = lx + ly * side + lz * side * side;
                    if reachable[index] || is_opaque(cell[0], cell[1], cell[2]) {
                        continue;
                    }
                    if NEIGHBOURS.iter().any(|&(dx, dy, dz)| {
                        let neighbour = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                        contains(neighbour, origin, side)
                            && reachable[index_in_cube(neighbour, origin, side)]
                    }) {
                        reachable[index] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut visible = reachable.clone();
    let mut reached = 0usize;
    for lz in 0..side {
        for ly in 0..side {
            for lx in 0..side {
                let index = lx + ly * side + lz * side * side;
                if !reachable[index] {
                    continue;
                }
                reached += 1;
                let cell = [
                    origin[0] + lx as i32,
                    origin[1] + ly as i32,
                    origin[2] + lz as i32,
                ];
                for (dx, dy, dz) in NEIGHBOURS {
                    let neighbour = [cell[0] + dx, cell[1] + dy, cell[2] + dz];
                    if contains(neighbour, origin, side) {
                        visible[index_in_cube(neighbour, origin, side)] = true;
                    }
                }
            }
        }
    }
    (visible, reached)
}

fn contains(block: [i32; 3], origin: [i32; 3], side: usize) -> bool {
    let side = side as i64;
    (0..3).all(|axis| {
        let local = i64::from(block[axis]) - i64::from(origin[axis]);
        (0..side).contains(&local)
    })
}

fn index_in_cube(block: [i32; 3], origin: [i32; 3], side: usize) -> usize {
    let x = (block[0] - origin[0]) as usize;
    let y = (block[1] - origin[1]) as usize;
    let z = (block[2] - origin[2]) as usize;
    x + y * side + z * side * side
}

/// Voxel DDA at block centres: true when no block before `end` is opaque.
///
/// The target itself may be opaque: a wall face touching open space is visible.
/// Ties step one axis at a time, which can only introduce an extra conservative
/// opaque test, never a false clear segment.
fn line_of_sight_clear(
    start: [i32; 3],
    end: [i32; 3],
    is_opaque: impl Fn(i32, i32, i32) -> bool,
) -> bool {
    assert!(
        !is_opaque(start[0], start[1], start[2]),
        "line of sight must start in non-opaque space"
    );
    if start == end {
        return true;
    }

    let direction = [
        f64::from(end[0] - start[0]),
        f64::from(end[1] - start[1]),
        f64::from(end[2] - start[2]),
    ];
    let step = [
        (end[0] - start[0]).signum(),
        (end[1] - start[1]).signum(),
        (end[2] - start[2]).signum(),
    ];
    let delta = direction.map(|component| {
        if component == 0.0 {
            f64::INFINITY
        } else {
            1.0 / component.abs()
        }
    });
    let mut next = [
        boundary_parameter(direction[0], step[0]),
        boundary_parameter(direction[1], step[1]),
        boundary_parameter(direction[2], step[2]),
    ];

    let mut cell = start;
    // The ray crosses at most `|dx| + |dy| + |dz|` boundaries.
    let mut guard = 0usize;
    loop {
        guard += 1;
        assert!(guard < 100_000, "line-of-sight traversal did not terminate");

        let axis = (0..3)
            .min_by(|&a, &b| next[a].total_cmp(&next[b]))
            .expect("three axes");
        cell[axis] += step[axis];
        if cell == end {
            return true;
        }
        if is_opaque(cell[0], cell[1], cell[2]) {
            return false;
        }
        next[axis] += delta[axis];
    }
}

/// Parameter at which a ray starting at a block centre crosses its first face.
fn boundary_parameter(direction: f64, step: i32) -> f64 {
    if step > 0 {
        0.5 / direction
    } else if step < 0 {
        -0.5 / direction
    } else {
        f64::INFINITY
    }
}

/// A terrain scene with sealed caves, an overhang, a slit wall and a pillar.
///
/// The caves give the fill interiors it must cull; the wall and overhang give
/// line-of-sight cases where connectivity and visibility disagree. The shape is
/// a pure function of world block coordinates, so the fill and the oracles read
/// the same world without a chunk store.
fn scene_opaque(x: i32, y: i32, z: i32) -> bool {
    let surface = terrain_height(x, z);

    if y <= surface {
        let cave_a = (x - 8).pow(2) + (y - 10).pow(2) + (z - 8).pow(2) <= 9;
        let cave_b = (x + 4).pow(2) + (y - 12).pow(2) + (z - 6).pow(2) <= 4;
        return !(cave_a || cave_b);
    }

    // A floating platform: opaque roof, open air beneath it.
    if (4..=12).contains(&x) && (4..=12).contains(&z) && (30..=31).contains(&y) {
        return true;
    }

    // A wall with one-block gap at z = 10.
    if x == 18 && (-5..=15).contains(&z) && (surface + 1..=surface + 8).contains(&y) && z != 10 {
        return true;
    }

    // A pillar in the open field.
    if x == -8 && z == -8 && (surface + 1..=surface + 6).contains(&y) {
        return true;
    }

    false
}

fn terrain_height(x: i32, z: i32) -> i32 {
    20 + (x.wrapping_mul(7).wrapping_add(z.wrapping_mul(11))).rem_euclid(9) - 4
}

/// Deterministic camera seeds: above ground, inside both caves, and beside the
/// slit wall. Every seed is nudged until it starts in non-opaque space.
fn pose_seeds() -> Vec<[i32; 3]> {
    let mut state = 0x0cce_51a7_9e37_79b9u64;
    let mut seeds = Vec::with_capacity(POSES);

    for pose in 0..POSES {
        let mut seed = match pose % 5 {
            0..=2 => {
                let x = random_range(&mut state, -16, 32);
                let z = random_range(&mut state, -16, 32);
                let y = terrain_height(x, z) + 2 + random_range(&mut state, 0, 8);
                [x, y, z]
            }
            3 => {
                // Inside the larger sealed cave.
                [
                    8 + random_range(&mut state, -1, 1),
                    10 + random_range(&mut state, -1, 1),
                    8 + random_range(&mut state, -1, 1),
                ]
            }
            _ => {
                // In front of the slit wall.
                let x = 14;
                let z = random_range(&mut state, -8, 18);
                let y = terrain_height(x, z) + 2 + random_range(&mut state, 0, 6);
                [x, y, z]
            }
        };

        let mut attempts = 0;
        while scene_opaque(seed[0], seed[1], seed[2]) && attempts < 32 {
            seed[1] += 1;
            attempts += 1;
        }
        assert!(
            !scene_opaque(seed[0], seed[1], seed[2]),
            "pose {pose}: could not place a seed in air above {seed:?}"
        );
        seeds.push(seed);
    }

    seeds
}

fn random_range(state: &mut u64, low: i32, high: i32) -> i32 {
    assert!(low <= high);
    let span = (i64::from(high) - i64::from(low) + 1) as u64;
    low + (next(state) % span) as i32
}

fn next(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}
