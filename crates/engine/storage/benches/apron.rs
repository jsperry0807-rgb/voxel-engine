//! M2-02: apron face planes versus a dense padded copy.
//!
//! The mesher can gather the six 1-voxel face planes ([`World::apron`]) or
//! materialise the whole `(CHUNK_DIM + 2)³` padded volume. This benchmark
//! measures both on the same resident neighbourhood and reports the byte ratio
//! that the task's 25% limit is written against.

use std::mem::size_of;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use voxel_storage::gpu::MemSink;
use voxel_storage::world::World;
use voxel_storage::{AIR, Apron, CHUNK_DIM, ChunkPos, local_index};

type FillFn = Box<dyn FnMut(ChunkPos, &mut [u16]) + Send>;

const POS: ChunkPos = ChunkPos::new(0, 0, 0);

fn tmp() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("vstore-apron-bench-{}-{stamp}", std::process::id()))
}

/// Rolling terrain with soil, stone and sea-level fill, so the DAG has both
/// solid and air paths and the apron is not a flat plane.
fn terrain() -> FillFn {
    Box::new(|p, dense| {
        let edge = CHUNK_DIM as i32;
        for z in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                let wx = p.x * edge + x as i32;
                let wz = p.z * edge + z as i32;
                let height = 24.0
                    + (wx as f32 * 0.15).sin() * 5.0
                    + (wz as f32 * 0.11).cos() * 4.0
                    + ((wx + wz) as f32 * 0.07).sin() * 3.0;
                let height = height as i32;
                for y in 0..CHUNK_DIM {
                    let wy = p.y * edge + y as i32;
                    let material = if wy < height - 4 {
                        1
                    } else if wy < height {
                        2
                    } else if wy == height {
                        3
                    } else {
                        AIR
                    };
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
    })
}

fn load_around(world: &mut World<MemSink>) {
    for (dx, dy, dz) in [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ] {
        world
            .load(ChunkPos::new(POS.x + dx, POS.y + dy, POS.z + dz))
            .unwrap();
    }
    world.load(POS).unwrap();
}

/// Materialises the full padded volume the way a dense mesher would: resolve
/// each of the 27 chunks once, then copy its voxels into the padded buffer.
///
/// The earlier version called `World::get` per padded voxel, which measured
/// 287 k hash lookups instead of the copy the apron is meant to avoid.
fn dense_copy(world: &World<MemSink>, out: &mut [u16]) {
    let edge = CHUNK_DIM as i32;
    let padded = CHUNK_DIM + 2;
    let limit = padded as i32;
    out.fill(AIR);

    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos::new(POS.x + dx, POS.y + dy, POS.z + dz);
                let Some(chunk) = world.chunk(pos) else {
                    continue;
                };
                // Chunk-local 0 maps to padded coordinate `d * edge + 1`.
                let base = [dx * edge + 1, dy * edge + 1, dz * edge + 1];
                for pz in base[2].max(0)..(base[2] + edge).min(limit) {
                    for py in base[1].max(0)..(base[1] + edge).min(limit) {
                        for px in base[0].max(0)..(base[0] + edge).min(limit) {
                            let local = local_index(
                                (px - base[0]) as u32,
                                (py - base[1]) as u32,
                                (pz - base[2]) as u32,
                            );
                            let index =
                                px as usize + py as usize * padded + pz as usize * padded * padded;
                            out[index] = chunk.material(local);
                        }
                    }
                }
            }
        }
    }
}

fn face_plane_bytes() -> u64 {
    // Derived from the type, so a plane-geometry change moves the ratio.
    6 * Apron::PLANE as u64 * size_of::<u16>() as u64
}

fn dense_bytes() -> u64 {
    let edge = CHUNK_DIM as u64 + 2;
    edge * edge * edge * 2
}

fn measure_ratio(world: &World<MemSink>) -> (Duration, Duration) {
    // Warm both buffers and the allocator before timing.
    let mut apron = Apron::new();
    world.apron_into(POS, &mut apron);
    let mut dense = vec![0u16; (CHUNK_DIM + 2).pow(3)];
    dense_copy(world, &mut dense);

    let apron_iterations = 200;
    let start = Instant::now();
    for _ in 0..apron_iterations {
        world.apron_into(POS, &mut apron);
        black_box(&apron);
    }
    let apron = start.elapsed() / apron_iterations;

    // The dense copy is two orders of magnitude slower; ten iterations are
    // enough to average out page faults without stalling the bench.
    let dense_iterations = 10;
    let start = Instant::now();
    for _ in 0..dense_iterations {
        dense_copy(world, &mut dense);
        black_box(&dense);
    }
    let dense = start.elapsed() / dense_iterations;
    (apron, dense)
}

fn apron_vs_dense(c: &mut Criterion) {
    let dir = tmp();
    let mut world = World::new(dir.clone(), terrain(), MemSink::default()).unwrap();
    load_around(&mut world);

    let (apron, dense) = measure_ratio(&world);
    let byte_ratio = face_plane_bytes() as f64 / dense_bytes() as f64;
    let time_ratio = apron.as_secs_f64() / dense.as_secs_f64();
    println!(
        "M2-02 edge={CHUNK_DIM}: apron {} B / {:?} vs dense {} B / {:?}; \
         bytes {:.1}%, time {:.1}%",
        face_plane_bytes(),
        apron,
        dense_bytes(),
        dense,
        byte_ratio * 100.0,
        time_ratio * 100.0,
    );

    let mut group = c.benchmark_group("apron");
    group.throughput(Throughput::Bytes(face_plane_bytes()));
    let mut apron = Apron::new();
    group.bench_function("face_planes", |b| {
        b.iter(|| {
            world.apron_into(POS, &mut apron);
            black_box(&apron);
        })
    });
    group.throughput(Throughput::Bytes(dense_bytes()));
    let mut dense = vec![0u16; (CHUNK_DIM + 2).pow(3)];
    group.bench_function("dense_padded", |b| {
        b.iter(|| {
            dense_copy(&world, &mut dense);
            black_box(&dense);
        })
    });
    group.finish();

    // Region files are written under a per-run temp directory; drop the world
    // (and its handles) before removing it, like `benches/store.rs` does.
    drop(world);
    let _ = std::fs::remove_dir_all(&dir);
}

criterion_group!(benches, apron_vs_dense);
criterion_main!(benches);
