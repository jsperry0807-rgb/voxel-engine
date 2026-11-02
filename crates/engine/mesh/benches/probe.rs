//! M2-18: production SH4 probe-bake cost and frame-budget granularity.
//!
//! Two things are measured:
//!
//! - **full bake** at the 64³ (4-block) and 128³ (8-block) near grids, which is
//!   the cost a streaming job has to spread over frames;
//! - **one frame** of [`ProbeBake::advance`] at a 256-ray budget, which is the
//!   unit the content scheduler can afford per frame.
//!
//! The fixture is a deterministic block-scale terrain with a floating block, so
//! every probe is not simply open sky and the directional coefficients vary.
//!
//! Measured 2026-10-30 (reference host, release): recorded in
//! `docs/specs/task-list.md` under M2-18. Set `PROBE_PROFILE=1` and pass
//! `--profile-time 10` to write a pprof flamegraph.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
#[cfg(unix)]
use pprof::criterion::{Output, PProfProfiler};
use voxel_mesh::probe::{ProbeBake, ProbeGrid, bake};
use voxel_storage::ChunkPos;

/// Rays a content frame may spend on a probe bake.
const RAYS_PER_FRAME: usize = 256;

/// Chunk position used for every measurement; the seed depends on it.
const CHUNK: ChunkPos = ChunkPos::new(3, -1, 5);

/// A deterministic terrain fixture with a hill and a floating block.
fn terrain(x: u32, y: u32, z: u32) -> bool {
    let block = voxel_core::constants::BLOCK_DIM as u32;
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

fn probe_cost(c: &mut Criterion) {
    for edge_blocks in [4usize, 8] {
        let grid = ProbeGrid::near(edge_blocks);
        let voxels = grid.scene_edge_voxels();
        let mut group = c.benchmark_group(format!("probe/{voxels}cubed"));
        group.bench_function("full_bake", |b| {
            b.iter(|| black_box(bake(CHUNK, grid, terrain)))
        });
        group.finish();
    }

    let grid = ProbeGrid::near(8);
    let mut job = ProbeBake::new(CHUNK, grid, terrain);
    let mut group = c.benchmark_group("probe/frame");
    group.bench_function(format!("advance_{RAYS_PER_FRAME}_rays"), |b| {
        b.iter_custom(|iterations| {
            let mut total = Duration::ZERO;
            for _ in 0..iterations {
                // Restart outside the timed region so a finished bake does not
                // charge its first frame.
                if job.is_done() {
                    job = ProbeBake::new(CHUNK, grid, terrain);
                }
                let start = Instant::now();
                let step = job.advance(RAYS_PER_FRAME);
                total += start.elapsed();
                black_box(step);
            }
            total
        });
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = probe_cost
}
criterion_main!(benches);

/// Attaches the pprof sampler only when a flamegraph is requested, so the
/// recorded timing runs are not perturbed by SIGPROF.
fn criterion_config() -> Criterion {
    let config = Criterion::default();
    #[cfg(unix)]
    if std::env::var_os("PROBE_PROFILE").is_some() {
        return config.with_profiler(PProfProfiler::new(997, Output::Flamegraph(None)));
    }
    config
}
