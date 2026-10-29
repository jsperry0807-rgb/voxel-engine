//! M2-02: apron face planes versus a dense padded copy.
//!
//! The mesher can gather the six 1-voxel face planes ([`World::apron`]) or
//! materialise the whole `(CHUNK_DIM + 2)³` padded volume. This benchmark
//! measures both on the same resident neighbourhood and reports the byte ratio
//! that the task's 25% limit is written against.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use voxel_storage::gpu::MemSink;
use voxel_storage::world::World;
use voxel_storage::{AIR, CHUNK_DIM, ChunkPos, local_index};

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

/// Reads the full padded volume one voxel at a time.
fn dense_read(world: &World<MemSink>, out: &mut [u16]) {
    let edge = CHUNK_DIM as i32;
    let padded = CHUNK_DIM + 2;
    let origin = (POS.x * edge, POS.y * edge, POS.z * edge);
    for z in -1..=edge {
        for y in -1..=edge {
            for x in -1..=edge {
                let value = world
                    .get(origin.0 + x, origin.1 + y, origin.2 + z)
                    .map(|material| material.as_raw())
                    .unwrap_or(AIR);
                let index = (x + 1) as usize
                    + (y + 1) as usize * padded
                    + (z + 1) as usize * padded * padded;
                out[index] = value;
            }
        }
    }
}

fn face_plane_bytes() -> u64 {
    6 * CHUNK_DIM as u64 * CHUNK_DIM as u64 * 2
}

fn dense_bytes() -> u64 {
    let edge = CHUNK_DIM as u64 + 2;
    edge * edge * edge * 2
}

fn measure_ratio(world: &World<MemSink>) -> (Duration, Duration) {
    let apron_iterations = 200;
    let start = Instant::now();
    for _ in 0..apron_iterations {
        black_box(world.apron(POS));
    }
    let apron = start.elapsed() / apron_iterations;

    let mut dense = vec![0u16; (CHUNK_DIM + 2).pow(3)];
    let dense_iterations = 3;
    let start = Instant::now();
    for _ in 0..dense_iterations {
        dense_read(world, &mut dense);
        black_box(&dense);
    }
    let dense = start.elapsed() / dense_iterations;
    (apron, dense)
}

fn apron_vs_dense(c: &mut Criterion) {
    let mut world = World::new(tmp(), terrain(), MemSink::default()).unwrap();
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
    group.bench_function("face_planes", |b| b.iter(|| black_box(world.apron(POS))));
    group.throughput(Throughput::Bytes(dense_bytes()));
    let mut dense = vec![0u16; (CHUNK_DIM + 2).pow(3)];
    group.bench_function("dense_padded", |b| {
        b.iter(|| {
            dense_read(&world, &mut dense);
            black_box(&dense);
        })
    });
    group.finish();
}

criterion_group!(benches, apron_vs_dense);
criterion_main!(benches);
