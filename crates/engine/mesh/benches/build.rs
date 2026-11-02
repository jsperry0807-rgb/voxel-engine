//! M2-28: single-threaded terrain chunk-meshing throughput.
//!
//! One build is the production path for one chunk: every 16³ block is filled
//! into the reusable 18³ scratch and meshed by `greedy_block` (AO in the merge
//! key) + `mesh_block` into the ADR-010 stream. 100 deterministic terrain
//! chunks are meshed in a row on one thread; the criterion group reports
//! chunks/s and the pre-run summary asserts the ≥ 20 chunks/s budget.
//!
//! The chunk volumes are generated once before timing, so the measurement is
//! meshing and not terrain generation. The apron is empty: this is the
//! single-chunk build the M2 exit names, not a border-seam benchmark.
//!
//! Measured 2026-10-31 (reference host, release): recorded in
//! `docs/specs/task-list.md` under M2-28. Set `BUILD_PROFILE=1` and pass
//! `--profile-time 10` to write a pprof flamegraph.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
#[cfg(unix)]
use pprof::criterion::{Output, PProfProfiler};
use voxel_core::constants::{CHUNK_DIM, CHUNK_VOLUME};
use voxel_mesh::{GreedyQuad, for_each_block, greedy_block, mesh_block};
use voxel_storage::Apron;
use voxel_storage::local_index;

/// Chunks in one measured run.
const CHUNKS: usize = 100;

/// M2-28 budget: at least 20 terrain chunks per second on one thread.
const BUDGET_PER_SECOND: f64 = 20.0;

/// Totals over one run, kept so the optimizer cannot elide the work.
#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    quads: usize,
    vertices: usize,
    indices: usize,
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Deterministic terrain height for one column: broad hills plus a fine
/// ripple, with a gentle rise toward the chunk edges.
fn height(seed: u64, x: u32, z: u32) -> usize {
    let broad = (mix(seed
        ^ (u64::from(x / 16)).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (u64::from(z / 16)).wrapping_mul(0xc2b2_ae3d_27d4_eb4f))
        % 9) as usize;
    let fine = (mix(seed
        ^ (u64::from(x / 4)).wrapping_mul(0xd6e8_feb8_6659_fd93)
        ^ (u64::from(z / 4)).wrapping_mul(0xa24b_aed4_963e_e407))
        % 5) as usize;
    let centre = CHUNK_DIM as u32 / 2;
    let rise = ((x as i32 - centre as i32).unsigned_abs()
        + (z as i32 - centre as i32).unsigned_abs()) as usize
        / 16;
    12 + broad + fine + rise.min(6)
}

/// One terrain chunk: a height field, two ellipsoid caves and one floating
/// shelf, so surfaces, overhangs and AO corners are all exercised without
/// turning the volume into per-voxel noise.
fn generate(seed: u64) -> Vec<u16> {
    let edge = CHUNK_DIM as u32;
    let mut volume = vec![0u16; CHUNK_VOLUME];
    for z in 0..edge {
        for x in 0..edge {
            let top = height(seed, x, z).min(CHUNK_DIM - 4);
            for y in 0..top {
                let material = if y + 1 == top {
                    3
                } else if y + 4 >= top {
                    2
                } else {
                    1
                };
                volume[local_index(x, y as u32, z) as usize] = material;
            }
        }
    }

    // Two caves: modest ellipsoids in the middle of the ground.
    for cave in 0..2u64 {
        let pick = |salt: u64, span: u64| (mix(seed ^ (cave << 32) ^ salt) % span) as i32;
        let radius = 5 + pick(1, 5);
        let cx = radius + pick(2, (edge as i32 - 2 * radius) as u64);
        let cy = 4 + pick(3, 10);
        let cz = radius + pick(4, (edge as i32 - 2 * radius) as u64);
        let rx = radius;
        let ry = 3 + pick(5, 3);
        let rz = radius;
        for y in (cy - ry).max(0)..=(cy + ry).min(CHUNK_DIM as i32 - 1) {
            for z in (cz - rz).max(0)..=(cz + rz).min(edge as i32 - 1) {
                for x in (cx - rx).max(0)..=(cx + rx).min(edge as i32 - 1) {
                    let dx = (x - cx) as f32 / rx as f32;
                    let dy = (y - cy) as f32 / ry as f32;
                    let dz = (z - cz) as f32 / rz as f32;
                    if dx * dx + dy * dy + dz * dz <= 1.0 {
                        volume[local_index(x as u32, y as u32, z as u32) as usize] = 0;
                    }
                }
            }
        }
    }

    // A floating shelf: a solid slab above the surface with air beneath it.
    let sx = (mix(seed ^ 0x51ed) % (u64::from(edge) - 14)) as u32 + 2;
    let sz = (mix(seed ^ 0xa17e) % (u64::from(edge) - 14)) as u32 + 2;
    for z in sz..sz + 12 {
        for x in sx..sx + 12 {
            let top = height(seed, x, z).min(CHUNK_DIM - 4);
            let base = (top + 4).min(CHUNK_DIM - 3) as u32;
            for y in base..base + 2 {
                volume[local_index(x, y, z) as usize] = 2;
            }
        }
    }

    volume
}

fn build(volume: &[u16], apron: &Apron, quads: &mut Vec<GreedyQuad>, stats: &mut Stats) {
    for_each_block(volume, apron, |block, scratch| {
        greedy_block(scratch, quads);
        let mesh = mesh_block(quads, block);
        stats.quads += mesh.quad_count();
        stats.vertices += mesh.vertices.len();
        stats.indices += mesh.indices.len();
        black_box(&mesh);
    });
}

/// Meshes 100 chunks and returns the throughput plus the work totals.
fn run(volumes: &[Vec<u16>]) -> (f64, Stats) {
    let apron = Apron::new();
    let mut quads = Vec::new();
    let mut stats = Stats::default();
    let start = Instant::now();
    for volume in volumes {
        build(volume, &apron, &mut quads, &mut stats);
    }
    let elapsed = start.elapsed();
    (volumes.len() as f64 / elapsed.as_secs_f64(), stats)
}

fn throughput(c: &mut Criterion) {
    let volumes: Vec<Vec<u16>> = (0..CHUNKS as u64).map(generate).collect();
    let (rate, stats) = run(&volumes);
    println!(
        "M2-28: {rate:.1} chunks/s single-threaded over {CHUNKS} chunks \
         (budget {BUDGET_PER_SECOND:.0}/s, {} quads, {} vertices, {} indices)",
        stats.quads, stats.vertices, stats.indices,
    );
    assert!(
        rate >= BUDGET_PER_SECOND,
        "M2-28 throughput {rate:.1} chunks/s is below the {BUDGET_PER_SECOND:.0}/s budget"
    );

    let mut group = c.benchmark_group("terrain");
    group.throughput(Throughput::Elements(CHUNKS as u64));
    group.bench_function("chunks_100", |b| {
        let apron = Apron::new();
        let mut quads = Vec::new();
        b.iter_custom(|iterations| {
            let mut total = Duration::ZERO;
            for _ in 0..iterations {
                let mut stats = Stats::default();
                for volume in &volumes {
                    let start = Instant::now();
                    build(volume, &apron, &mut quads, &mut stats);
                    total += start.elapsed();
                }
                black_box(&stats);
            }
            total
        });
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = throughput
}
criterion_main!(benches);

fn criterion_config() -> Criterion {
    let config = Criterion::default();
    #[cfg(unix)]
    if std::env::var_os("BUILD_PROFILE").is_some() {
        return config.with_profiler(PProfProfiler::new(997, Output::Flamegraph(None)));
    }
    config
}
