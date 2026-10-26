use criterion::{BatchSize, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::PathBuf;
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::{GpuStore, MemSink};
use voxel_storage::overlay::Overlay;
use voxel_storage::world::World;
use voxel_storage::{CHUNK_DIM, CHUNK_VOLUME, ChunkPos, MaterialId, local_index, local_xyz};

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn terrain(ox: i32, oz: i32) -> Vec<u16> {
    let mut d = vec![0u16; CHUNK_VOLUME];
    let mut s = 0x9E3779B97F4A7C15u64 ^ ((ox as u64) << 20) ^ (oz as u64);
    let edge = CHUNK_DIM as i32;
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let (fx, fz) = ((x as i32 + ox * edge) as f32, (z as i32 + oz * edge) as f32);
            let h = 28
                + ((fx * 0.15).sin() * 6.0
                    + (fz * 0.11).cos() * 5.0
                    + ((fx + fz) * 0.07).sin() * 4.0) as i32;
            for y in 0..CHUNK_DIM {
                let yi = y as i32;
                let m = if yi < h - 4 {
                    1
                } else if yi < h {
                    2
                } else if yi == h {
                    3
                } else {
                    0
                };
                let cave = m == 1 && rng(&mut s) % 100 < 3;
                d[local_index(x as u32, y as u32, z as u32) as usize] = if cave { 0 } else { m };
            }
        }
    }
    d
}

fn noisy() -> Vec<u16> {
    let mut s = 12345u64;
    (0..CHUNK_VOLUME)
        .map(|_| {
            if rng(&mut s) % 10 < 3 {
                (rng(&mut s) % 8 + 1) as u16
            } else {
                0
            }
        })
        .collect()
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("vsb-{name}-{}", std::process::id()))
}

/// A world-space voxel inside the 4x4 resident chunk square.
fn world_voxel(r: u64) -> (i32, i32, i32) {
    let span = 4 * CHUNK_DIM as u64;
    let (_, y, _) = local_xyz(r as u32);
    ((r % span) as i32, y as i32, ((r >> 32) % span) as i32)
}

fn bench_dag(c: &mut Criterion) {
    let t = terrain(0, 0);
    let n = noisy();
    let solid = vec![1u16; CHUNK_VOLUME];
    let mut g = c.benchmark_group("dag");
    g.sample_size(20);
    g.throughput(Throughput::Elements(CHUNK_VOLUME as u64));
    g.bench_function("from_dense/terrain", |b| {
        b.iter(|| Svdag::from_dense(black_box(&t)))
    });
    g.bench_function("from_dense/solid", |b| {
        b.iter(|| Svdag::from_dense(black_box(&solid)))
    });
    g.bench_function("from_dense/noise30pct_worst", |b| {
        b.iter(|| Svdag::from_dense(black_box(&n)))
    });
    let dt = Svdag::from_dense(&t);
    let dn = Svdag::from_dense(&n);
    println!(
        "terrain dag: {} words ({} KiB) vs dense {} KiB",
        dt.words(),
        dt.words() * 4 / 1024,
        CHUNK_VOLUME * 2 / 1024
    );
    println!(
        "noise   dag: {} words ({} KiB)",
        dn.words(),
        dn.words() * 4 / 1024
    );
    let mut out = vec![0u16; CHUNK_VOLUME];
    g.bench_function("expand/terrain", |b| {
        b.iter(|| dt.expand(black_box(&mut out)))
    });
    g.throughput(Throughput::Elements(1));
    let mut s = 1u64;
    g.bench_function("get/terrain_random", |b| {
        b.iter(|| {
            let (x, y, z) = local_xyz(rng(&mut s) as u32);
            black_box(dt.get(x, y, z))
        })
    });
    g.bench_function("get/noise_random", |b| {
        b.iter(|| {
            let (x, y, z) = local_xyz(rng(&mut s) as u32);
            black_box(dn.get(x, y, z))
        })
    });
    g.throughput(Throughput::Elements(1));
    for n_edits in [1usize, 100, 1000] {
        let mut s2 = 3u64;
        let edits: Vec<(u32, u16)> = (0..n_edits)
            .map(|_| ((rng(&mut s2) as u32) & (CHUNK_VOLUME as u32 - 1), 4u16))
            .collect();
        g.bench_function(format!("apply_edits/terrain_{n_edits}"), |b| {
            b.iter(|| black_box(dt.apply_edits(edits.iter().copied())))
        });
    }
    g.throughput(Throughput::Bytes(dt.to_bytes().len() as u64));
    g.bench_function("serialize/terrain", |b| b.iter(|| black_box(dt.to_bytes())));
    let bytes = dt.to_bytes();
    g.bench_function("deserialize/terrain", |b| {
        b.iter(|| black_box(Svdag::from_bytes(&bytes)))
    });
    g.finish();
}

fn bench_overlay_world(c: &mut Criterion) {
    let mut g = c.benchmark_group("edit");
    g.throughput(Throughput::Elements(1));
    let mut ov = Overlay::default();
    let mut s = 7u64;
    g.bench_function("overlay/set", |b| {
        b.iter(|| ov.set((rng(&mut s) as u32) & (CHUNK_VOLUME as u32 - 1), 2))
    });
    g.bench_function("overlay/get", |b| {
        b.iter(|| black_box(ov.get((rng(&mut s) as u32) & (CHUNK_VOLUME as u32 - 1))))
    });

    let dir = tmp("edit");
    let mut w = World::new(
        &dir,
        Box::new(|p, d| d.copy_from_slice(&terrain(p.x, p.z))),
        MemSink::default(),
    )
    .unwrap();
    for x in 0..4 {
        for z in 0..4 {
            w.load(ChunkPos::new(x, 0, z)).unwrap();
        }
    }
    g.bench_function("world/set(4x4 chunks resident)", |b| {
        b.iter(|| {
            let r = rng(&mut s);
            let (x, y, z) = world_voxel(r);
            w.set(x, y, z, MaterialId::from_raw(((r >> 32) & 3) as u16))
                .unwrap();
        })
    });
    g.bench_function("world/get(overlay+dag)", |b| {
        b.iter(|| {
            let (x, y, z) = world_voxel(rng(&mut s));
            black_box(w.get(x, y, z))
        })
    });
    g.finish();
    std::fs::remove_dir_all(dir).ok();
}

fn bench_consolidate(c: &mut Criterion) {
    let mut g = c.benchmark_group("consolidate");
    g.sample_size(10);
    for (chunks, edits) in [(1usize, 1usize), (1, 1000), (16, 100), (64, 100)] {
        g.throughput(Throughput::Elements(chunks as u64));
        g.bench_function(format!("{chunks}chunks_x_{edits}edits"), |b| {
            b.iter_batched(
                || {
                    let dir = tmp("cons");
                    let mut w = World::new(
                        &dir,
                        Box::new(|p, d| d.copy_from_slice(&terrain(p.x, p.z))),
                        MemSink::default(),
                    )
                    .unwrap();
                    let mut s = 99u64;
                    for i in 0..chunks {
                        let (cx, cz) = ((i % 8) as i32, (i / 8) as i32);
                        for _ in 0..edits {
                            let r = rng(&mut s);
                            let (lx, ly, lz) = local_xyz(r as u32);
                            w.set(
                                cx * CHUNK_DIM as i32 + lx as i32,
                                ly as i32,
                                cz * CHUNK_DIM as i32 + lz as i32,
                                MaterialId::from_raw(((r >> 32) % 5) as u16),
                            )
                            .unwrap();
                        }
                    }
                    w.flush_gpu();
                    (w, dir)
                },
                |(mut w, dir)| {
                    let r = w.consolidate(chunks).unwrap();
                    w.reclaim(r.epoch);
                    black_box(&w);
                    std::fs::remove_dir_all(dir).ok();
                },
                BatchSize::PerIteration,
            )
        });
    }
    g.finish();
}

fn bench_gpu(c: &mut Criterion) {
    let mut g = c.benchmark_group("gpu_tier");
    g.sample_size(20);
    g.throughput(Whole::one());
    let a = Svdag::from_dense(&terrain(0, 0));
    let mut d2 = terrain(0, 0);
    let mut s = 5u64;
    for _ in 0..100 {
        d2[(rng(&mut s) as usize) % CHUNK_VOLUME] = 4;
    }
    let b2 = Svdag::from_dense(&d2);

    g.bench_function("set_chunk/unique_import", |bn| {
        bn.iter_batched(
            GpuStore::new,
            |mut gs| gs.set_chunk(ChunkPos::new(0, 0, 0), black_box(&a)),
            BatchSize::LargeInput,
        )
    });
    let mut gs = GpuStore::new();
    gs.set_chunk(ChunkPos::new(0, 0, 0), &a);
    let mut i = 1;
    g.bench_function("set_chunk/full_dedup_hit(new pos, same dag)", |bn| {
        bn.iter(|| {
            i += 1;
            gs.set_chunk(ChunkPos::new(i, 0, 0), black_box(&a))
        })
    });
    let mut gs = GpuStore::new();
    let mut flip = false;
    let mut sink = MemSink::default();
    g.bench_function("cow_swap+reclaim+flush (100-edit diff)", |bn| {
        bn.iter(|| {
            flip = !flip;
            let t = gs.consolidate([(ChunkPos::new(0, 0, 0), if flip { &a } else { &b2 })]);
            gs.flush(&mut sink);
            gs.reclaim(t);
        })
    });
    {
        let mut gs = GpuStore::new();
        gs.set_chunk(ChunkPos::new(0, 0, 0), &a);
        let mut sink = MemSink::default();
        gs.flush(&mut sink);
        for n_edits in [1usize, 100] {
            let mut s3 = 11u64;
            let idxs: Vec<u32> = (0..n_edits)
                .map(|_| (rng(&mut s3) as u32) & (CHUNK_VOLUME as u32 - 1))
                .collect();
            let mut flip = false;
            g.bench_function(
                format!("apply_edits+reclaim+flush ({n_edits} edits)"),
                |bn| {
                    bn.iter(|| {
                        flip = !flip;
                        let v = if flip { 4u16 } else { 5 };
                        gs.apply_edits(ChunkPos::new(0, 0, 0), idxs.iter().map(|&i| (i, v)));
                        let t = gs.end_batch();
                        gs.flush(&mut sink);
                        gs.reclaim(t);
                    })
                },
            );
        }
    }
    g.throughput(Whole::one());
    let mut gs = GpuStore::new();
    gs.set_chunk(ChunkPos::new(3, 0, 3), &a);
    g.bench_function("lookup(table probe + dag walk)", |bn| {
        bn.iter(|| {
            let (x, y, z) = local_xyz(rng(&mut s) as u32);
            black_box(gs.lookup(ChunkPos::new(3, 0, 3), x, y, z))
        })
    });
    g.bench_function("table_insert_10k_chunks(grow+rehash)", |bn| {
        bn.iter(|| {
            let mut gs = GpuStore::new();
            for k in 0..10_000 {
                gs.set_chunk(ChunkPos::new(k % 100, 0, k / 100), &a);
            }
            black_box(gs.chunk_count())
        })
    });
    g.finish();
}

struct Whole;
impl Whole {
    fn one() -> Throughput {
        Throughput::Elements(1)
    }
}

/// Run `f` `iters` times, compacting the log every 64 appends (compaction is not timed),
/// so disk use stays bounded no matter how many iterations criterion decides to run.
fn timed<F: FnMut(&mut voxel_storage::disk::RegionStore)>(
    rs: &mut voxel_storage::disk::RegionStore,
    iters: u64,
    mut f: F,
) -> std::time::Duration {
    let mut total = std::time::Duration::ZERO;
    let mut left = iters;
    while left > 0 {
        let n = left.min(64);
        let t = std::time::Instant::now();
        for _ in 0..n {
            f(rs);
        }
        total += t.elapsed();
        rs.compact((0, 0, 0)).unwrap();
        left -= n;
    }
    total
}

fn bench_disk(c: &mut Criterion) {
    let mut g = c.benchmark_group("disk");
    g.sample_size(20);
    let dir = tmp("disk");
    let mut rs = voxel_storage::disk::RegionStore::open(&dir).unwrap();
    let a = Svdag::from_dense(&terrain(0, 0));
    let p = ChunkPos::new(0, 0, 0);
    g.throughput(Throughput::Bytes(a.to_bytes().len() as u64));
    g.bench_function("put_dag/terrain(no fsync)", |b| {
        b.iter_custom(|n| timed(&mut rs, n, |r| r.put_dag(p, black_box(&a)).unwrap()))
    });
    g.bench_function("load/terrain", |b| {
        b.iter(|| black_box(rs.load(p).unwrap()))
    });
    let mut ov = Overlay::default();
    for i in 0..1000u32 {
        ov.set(i * 13, 2);
    }
    g.throughput(Throughput::Elements(1000));
    g.bench_function("put_diff/1000_edits(no fsync)", |b| {
        b.iter_custom(|n| timed(&mut rs, n, |r| r.put_diff(p, black_box(&ov)).unwrap()))
    });
    g.bench_function("fsync_after_put_diff", |b| {
        b.iter_custom(|n| {
            timed(&mut rs, n, |r| {
                r.put_diff(p, &ov).unwrap();
                r.sync().unwrap();
            })
        })
    });
    g.finish();
    drop(rs);
    std::fs::remove_dir_all(dir).ok();
}

criterion_group!(
    benches,
    bench_dag,
    bench_overlay_world,
    bench_consolidate,
    bench_gpu,
    bench_disk
);
criterion_main!(benches);
