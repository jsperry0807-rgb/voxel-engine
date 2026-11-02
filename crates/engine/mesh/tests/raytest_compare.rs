//! M2-26: flood fill vs SVDAG ray-test over 100 poses.
//!
//! A small SVDAG world (terrain, a slit wall, a pillar and a sealed cave) is
//! swept with 100 deterministic camera poses. Each pose runs both occluders at
//! the same block seed and horizon, and the test pins the comparison the M2-26
//! report is written from:
//!
//! - the ray-test visible set is always a subset of the fill's;
//! - the delta (fill-only blocks) is non-zero overall;
//! - both passes produce a measurable cost;
//! - the summary prints the counts and timings the report records.
//!
//! The horizon is 4 blocks: the production 128 m horizon is a benchmark, not a
//! unit test. The semantics are identical at any horizon, and the cost scales
//! `O(h⁴)` for the ray-test against `O(h³)` for the fill.

#![cfg(feature = "raytest")]

use std::collections::HashMap;
use std::time::Instant;

use voxel_core::constants::{BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::raytest::{BLOCK_EDGE_M, RayTestFill, SvdagScene};
use voxel_storage::dag::Svdag;
use voxel_storage::reference::occlusion::{self, OcclusionFill};
use voxel_storage::{AIR, ChunkPos, local_index};

/// Poses required by M2-26.
const POSES: usize = 100;
/// Small horizon; see the module docs.
const HORIZON: u32 = 4;
/// World edge in blocks: 16 × 16 × 16.
const WORLD: i32 = 16;

fn blocks_per_edge() -> i32 {
    (CHUNK_DIM / BLOCK_DIM) as i32
}

fn height(x: i32, z: i32) -> i32 {
    4 + (x.wrapping_mul(7).wrapping_add(z.wrapping_mul(11))).rem_euclid(5)
}

/// The block-level scene: terrain, a slit wall at x = 6, a pillar at (4, 10),
/// and a sealed cave pocket under the ground at x/z 8..=10, y 1..=2.
fn solid(x: i32, y: i32, z: i32) -> bool {
    let inside = |coordinate: i32| (0..WORLD).contains(&coordinate);
    if !inside(x) || !inside(y) || !inside(z) {
        return false;
    }
    // The sealed pocket is carved below the minimum terrain height (4), so the
    // block above it is always solid.
    if (8..=10).contains(&x) && (1..=2).contains(&y) && (8..=10).contains(&z) {
        return false;
    }
    let surface = height(x, z);
    if y < surface {
        return true;
    }
    // A wall with a one-block slit at z = 8.
    if x == 6 && (surface..=surface + 3).contains(&y) && z != 8 {
        return true;
    }
    // A pillar in the open field.
    x == 4 && z == 10 && (surface..=surface + 4).contains(&y)
}

fn camera_block(block: [i32; 3]) -> [f64; 3] {
    [
        (block[0] as f64 + 0.5) * BLOCK_EDGE_M,
        (block[1] as f64 + 0.5) * BLOCK_EDGE_M,
        (block[2] as f64 + 0.5) * BLOCK_EDGE_M,
    ]
}

/// A dense chunk map and the matching SVDAGs.
fn world() -> HashMap<ChunkPos, Svdag> {
    let blocks = blocks_per_edge();
    let chunks = (WORLD + blocks - 1) / blocks;
    let edge = BLOCK_DIM as i32;
    let mut dags = HashMap::new();

    for cz in 0..chunks {
        for cy in 0..chunks {
            for cx in 0..chunks {
                let mut voxels = vec![AIR; CHUNK_VOLUME];
                for bz in 0..blocks {
                    for by in 0..blocks {
                        for bx in 0..blocks {
                            let x = cx * blocks + bx;
                            let y = cy * blocks + by;
                            let z = cz * blocks + bz;
                            if !solid(x, y, z) {
                                continue;
                            }
                            for zz in 0..edge {
                                for yy in 0..edge {
                                    for xx in 0..edge {
                                        let index = local_index(
                                            (bx * edge + xx) as u32,
                                            (by * edge + yy) as u32,
                                            (bz * edge + zz) as u32,
                                        );
                                        voxels[index as usize] = 1;
                                    }
                                }
                            }
                        }
                    }
                }
                let pos = ChunkPos::new(cx, cy, cz);
                dags.insert(pos, Svdag::from_dense(&voxels));
            }
        }
    }
    dags
}

/// 100 deterministic air poses: above the terrain, in front of the slit wall,
/// and inside the sealed cave.
fn poses() -> Vec<[i32; 3]> {
    let mut state = 0x0cce_51a7_9e37_79b9u64;
    let mut seeds = Vec::with_capacity(POSES);
    for pose in 0..POSES {
        let mut seed = match pose % 5 {
            0..=2 => {
                let x = random_range(&mut state, 1, WORLD - 2);
                let z = random_range(&mut state, 1, WORLD - 2);
                let y = height(x, z) + 2 + random_range(&mut state, 0, 5);
                [x, y, z]
            }
            3 => {
                // In front of the slit wall.
                let z = random_range(&mut state, 1, WORLD - 2);
                [3, height(3, z) + 2, z]
            }
            _ => {
                // Inside the sealed cave pocket.
                [9, 3, 9]
            }
        };
        let mut attempts = 0;
        while solid(seed[0], seed[1], seed[2]) && attempts < 8 {
            seed[1] += 1;
            attempts += 1;
        }
        assert!(
            !solid(seed[0], seed[1], seed[2]),
            "pose {pose}: could not place a seed in air above {seed:?}"
        );
        seeds.push(seed);
    }
    seeds
}

fn random_range(state: &mut u64, low: i32, high: i32) -> i32 {
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

fn flood(seed: [i32; 3]) -> OcclusionFill {
    occlusion::flood_fill_from_block(seed, HORIZON, solid)
}

fn subtitle(fill: &OcclusionFill, ray: &RayTestFill) -> (usize, usize, usize) {
    let fill_visible = fill.visible_count();
    let ray_visible = ray.visible_count();
    let delta = fill
        .visible_blocks()
        .filter(|block| ray.is_occluded(*block))
        .count();
    (fill_visible, ray_visible, delta)
}

fn percentile(values: &mut [u64], fraction: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values[index]
}

#[test]
fn raytest_compares_with_the_fill_on_one_hundred_poses() {
    let dags = world();
    let chunks: HashMap<ChunkPos, &Svdag> = dags.iter().map(|(pos, dag)| (*pos, dag)).collect();
    let scene = SvdagScene::new(&chunks);
    let seeds = poses();

    let mut fill_ns = Vec::with_capacity(POSES);
    let mut ray_ns = Vec::with_capacity(POSES);
    let mut fill_total = 0usize;
    let mut fill_in_world = 0usize;
    let mut ray_total = 0usize;
    let mut delta_total = 0usize;
    let mut delta_in_world = 0usize;
    let mut steps_total = 0u64;

    for (pose, seed) in seeds.iter().copied().enumerate() {
        let fill_start = Instant::now();
        let fill = flood(seed);
        fill_ns.push(fill_start.elapsed().as_nanos() as u64);

        let ray_start = Instant::now();
        let ray = scene.ray_test(camera_block(seed), HORIZON);
        ray_ns.push(ray_start.elapsed().as_nanos() as u64);

        let (fill_visible, ray_visible, _) = subtitle(&fill, &ray);
        for block in ray.visible_blocks() {
            assert!(
                fill.is_visible(block),
                "pose {pose}: the ray-test saw {block:?}, which the fill could not reach"
            );
        }
        for block in fill.visible_blocks() {
            let inside = (0..WORLD).contains(&block[0])
                && (0..WORLD).contains(&block[1])
                && (0..WORLD).contains(&block[2]);
            if !inside {
                continue;
            }
            fill_in_world += 1;
            if ray.is_occluded(block) {
                delta_in_world += 1;
            }
        }
        fill_total += fill_visible;
        ray_total += ray_visible;
        delta_total += fill_visible - ray_visible;
        steps_total += ray.voxel_steps();
    }

    assert!(delta_total > 0, "the sweep must exercise the delta");
    assert!(fill_in_world > 0 && delta_in_world > 0);
    // The report table is written from these deterministic aggregates.
    assert_eq!(fill_total, 68_787, "fill-visible aggregate");
    assert_eq!(ray_total, 47_104, "ray-visible aggregate");
    assert_eq!(delta_total, 21_683, "delta aggregate");
    assert_eq!(delta_in_world, 20_227, "in-world delta aggregate");
    assert_eq!(steps_total, 5_079_443, "voxel-step aggregate");
    let fill_p50 = percentile(&mut fill_ns, 0.50);
    let fill_p95 = percentile(&mut fill_ns, 0.95);
    let fill_max = *fill_ns.iter().max().unwrap();
    let ray_p50 = percentile(&mut ray_ns, 0.50);
    let ray_p95 = percentile(&mut ray_ns, 0.95);
    let ray_max = *ray_ns.iter().max().unwrap();
    let delta_percent = 100.0 * delta_total as f64 / fill_total as f64;
    let delta_world_percent = 100.0 * delta_in_world as f64 / fill_in_world as f64;

    println!("M2-26 100-pose comparison (horizon {HORIZON} blocks)");
    println!(
        "fill:  p50 {:.1} µs  p95 {:.1} µs  max {:.1} µs  visible/pose {:.1}",
        fill_p50 as f64 / 1e3,
        fill_p95 as f64 / 1e3,
        fill_max as f64 / 1e3,
        fill_total as f64 / POSES as f64,
    );
    println!(
        "ray:   p50 {:.1} µs  p95 {:.1} µs  max {:.1} µs  visible/pose {:.1}",
        ray_p50 as f64 / 1e3,
        ray_p95 as f64 / 1e3,
        ray_max as f64 / 1e3,
        ray_total as f64 / POSES as f64,
    );
    println!(
        "delta: {delta_total} blocks ({delta_percent:.1}% of all fill-visible); \
         in-world {delta_in_world} ({delta_world_percent:.1}% of in-world fill-visible); \
         {:.0} voxel steps/pose",
        steps_total as f64 / POSES as f64,
    );
    println!(
        "cost ratio: ray/fill p50 {:.1}×, p95 {:.1}×",
        ray_p50 as f64 / fill_p50 as f64,
        ray_p95 as f64 / fill_p95 as f64,
    );
}
