//! Deterministic hardware lens through Callgrind.
//!
//! Criterion reports wall clock, which moves with frequency scaling, turbo and
//! neighbours on the machine. These benchmarks report instruction counts and
//! simulated cache/branch behavior instead, which is stable enough for CI.
//!
//! Run with `cargo bench -p voxel-bench --features iai --bench iai`.
//! Requires Valgrind (`valgrind --version`).

use std::hint::black_box;

use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use voxel_bench::{fixtures, patterns};
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::GpuStore;
use voxel_storage::{BLOCK_DIM, local_index, local_xyz};

fn lookup_scene() -> (voxel_block::BlockRegistry, Vec<voxel_block::MaterialId>) {
    let registry = fixtures::registry(1_024);
    let ids = fixtures::random_ids(&registry, 4_096, 0xC0FF_EE01);
    (registry, ids)
}

fn dag_scene() -> (Svdag, Vec<(u32, u32, u32)>) {
    let dag = Svdag::from_dense(&patterns::terrain(0, 0));
    let mut state = 0xDEAD_BEEFu64;
    let coords = (0..4_096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            local_xyz(state as u32)
        })
        .collect();
    (dag, coords)
}

fn gpu_scene() -> (GpuStore, Vec<(u32, u32, u32)>) {
    let dag = Svdag::from_dense(&patterns::terrain(0, 0));
    let mut store = GpuStore::new();
    store.set_chunk(ChunkPos::new(0, 0, 0), &dag);
    let (_, coords) = dag_scene();
    (store, coords)
}

fn gpu_overlay_scene() -> (GpuStore, Vec<(u32, u32, u32)>) {
    let (mut store, coords) = gpu_scene();
    let pos = ChunkPos::new(0, 0, 0);
    // One full BLOCK_DIM^3 block at the chunk origin, so the deterministic run
    // crosses both bitmap states.
    let block_volume = (BLOCK_DIM * BLOCK_DIM * BLOCK_DIM) as u32;
    for within in 0..block_volume {
        let x = within % BLOCK_DIM as u32;
        let y = (within / BLOCK_DIM as u32) % BLOCK_DIM as u32;
        let z = within / (BLOCK_DIM * BLOCK_DIM) as u32;
        store.set_overlay(pos, local_index(x, y, z), 4);
    }
    (store, coords)
}

fn material_scene() -> voxel_block::BlockRegistry {
    fixtures::registry(65_536)
}

#[library_benchmark]
#[bench::hot_lookup(lookup_scene())]
fn hot_lookup((registry, ids): (voxel_block::BlockRegistry, Vec<voxel_block::MaterialId>)) -> u32 {
    let mut acc = 0u32;
    for id in ids {
        acc = acc.wrapping_add(registry.hot(id).to_words()[0]);
    }
    black_box(acc)
}

#[library_benchmark]
#[bench::resolved_lookup(lookup_scene())]
fn resolved_lookup(
    (registry, ids): (voxel_block::BlockRegistry, Vec<voxel_block::MaterialId>),
) -> u32 {
    let mut acc = 0u32;
    for id in ids {
        acc = acc.wrapping_add(registry.resolved(id).light_emission as u32);
    }
    black_box(acc)
}

#[library_benchmark]
#[bench::dag_get(dag_scene())]
fn dag_get((dag, coords): (Svdag, Vec<(u32, u32, u32)>)) -> u32 {
    let mut acc = 0u32;
    for (x, y, z) in coords {
        acc = acc.wrapping_add(dag.get(x, y, z) as u32);
    }
    black_box(acc)
}

#[library_benchmark]
#[bench::dag_build(patterns::terrain(0, 0))]
fn dag_build(dense: Vec<u16>) -> usize {
    black_box(Svdag::from_dense(&dense).words())
}

#[library_benchmark]
#[bench::gpu_lookup(gpu_scene())]
fn gpu_lookup((store, coords): (GpuStore, Vec<(u32, u32, u32)>)) -> u32 {
    let pos = ChunkPos::new(0, 0, 0);
    let mut acc = 0u32;
    for (x, y, z) in coords {
        acc = acc.wrapping_add(store.lookup(pos, x, y, z) as u32);
    }
    black_box(acc)
}

#[library_benchmark]
#[bench::gpu_lookup_overlay(gpu_overlay_scene())]
fn gpu_lookup_overlay((store, coords): (GpuStore, Vec<(u32, u32, u32)>)) -> u32 {
    let pos = ChunkPos::new(0, 0, 0);
    let mut acc = 0u32;
    for (x, y, z) in coords {
        acc = acc.wrapping_add(store.lookup(pos, x, y, z) as u32);
    }
    black_box(acc)
}

#[library_benchmark]
#[bench::material_words(material_scene())]
fn material_words(registry: voxel_block::BlockRegistry) -> usize {
    black_box(registry.gpu_hot_words().len() + registry.gpu_texture_set_words().len())
}

library_benchmark_group!(
    name = voxel;
    benchmarks = hot_lookup, resolved_lookup, dag_get, dag_build, gpu_lookup, gpu_lookup_overlay, material_words
);

main!(library_benchmark_groups = voxel);
