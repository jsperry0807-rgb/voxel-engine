//! Criterion lens for the block registry and its hot/cold lookup paths.
//!
//! The memory and CPU lenses report bytes and process time; this one reports
//! statistically stable wall-clock timings and throughput, which is what
//! catches regressions in the per-voxel path.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use voxel_bench::fixtures;

const LOOKUPS: usize = 1 << 16;

fn bench_registry(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry");
    for blocks in [1_024usize, 65_536] {
        group.throughput(Throughput::Elements(blocks as u64));
        group.bench_function(format!("build+freeze/{blocks}"), |b| {
            b.iter(|| fixtures::registry(black_box(blocks)))
        });
    }
    group.finish();
}

fn bench_lookups(c: &mut Criterion) {
    let registry = fixtures::registry(4_096);
    let random = fixtures::random_ids(&registry, LOOKUPS, 0xC0FF_EE01);
    let coherent = fixtures::coherent_ids(&registry, LOOKUPS, 0xC0FF_EE02);
    let keys: Vec<_> = registry.keys().map(|(_, key)| key.clone()).collect();

    let mut group = c.benchmark_group("lookup");
    group.throughput(Throughput::Elements(LOOKUPS as u64));

    group.bench_function("hot/random", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            for &id in &random {
                acc = acc.wrapping_add(registry.hot(id).to_words()[0]);
            }
            black_box(acc)
        })
    });
    group.bench_function("hot/coherent", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            for &id in &coherent {
                acc = acc.wrapping_add(registry.hot(id).to_words()[0]);
            }
            black_box(acc)
        })
    });
    group.bench_function("resolved/random", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            for &id in &random {
                acc = acc.wrapping_add(registry.resolved(id).light_emission as u32);
            }
            black_box(acc)
        })
    });
    group.bench_function("key/random", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            for key in &keys {
                acc = acc.wrapping_add(registry.id(key).map_or(0, |id| id.as_raw() as u32));
            }
            black_box(acc)
        })
    });
    group.finish();
}

criterion_group!(benches, bench_registry, bench_lookups);
criterion_main!(benches);
