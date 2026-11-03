//! Criterion benchmarks for the §22 workload table.
//!
//! Run with `cargo bench`. These are informational and non-gating. For a fair
//! comparison against `bevy_ecs`/`hecs`, see `benches/README.md`.

#![allow(missing_docs)] // criterion's generated harness has no rustdoc surface

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use voxel_ecs::{Storage, World};

#[derive(Clone, Copy)]
struct Position(f32);
#[derive(Clone, Copy)]
struct Velocity(f32);
#[derive(Clone, Copy)]
struct Mass(f32);

#[derive(Clone, Copy)]
struct Health(f32);
#[derive(Clone, Copy)]
struct Stamina(f32);

const ENTITIES: usize = 10_000;

fn build_world() -> World {
    let mut world = World::new();
    world.register::<Position>(Storage::Table);
    world.register::<Velocity>(Storage::Table);
    world.register::<Mass>(Storage::Table);
    world.register::<Health>(Storage::SparseSet);
    world.register::<Stamina>(Storage::SparseSet);

    for index in 0..ENTITIES {
        let position = Position(index as f32);
        let velocity = Velocity(1.0);
        let mass = Mass(2.0);
        if index % 2 == 0 {
            world.spawn((position, velocity, mass, Health(100.0)));
        } else {
            world.spawn((position, velocity, mass));
        }
    }
    world
}

fn bench_spawn(c: &mut Criterion) {
    let mut group = c.benchmark_group("lifecycle");
    group.bench_function("spawn", |b| {
        b.iter(|| {
            let mut world = World::new();
            world.register::<Position>(Storage::Table);
            let entity = world.spawn((Position(1.0),));
            black_box(entity);
        });
    });
    group.bench_function("spawn_batch", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                world
            },
            |mut world| {
                for index in 0..1_000 {
                    let entity = world.spawn((Position(index as f32),));
                    black_box(entity);
                }
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("despawn", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                let entity = world.spawn((Position(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.despawn(entity));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_structural(c: &mut Criterion) {
    let mut group = c.benchmark_group("structural");
    group.bench_function("insert_existing_table", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                let entity = world.spawn((Position(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.insert(entity, Position(2.0)));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("insert_new_table", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                world.register::<Velocity>(Storage::Table);
                let entity = world.spawn((Position(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.insert(entity, Velocity(2.0)));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("insert_sparse", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                world.register::<Health>(Storage::SparseSet);
                let entity = world.spawn((Position(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.insert(entity, Health(2.0)));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("remove_table", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Position>(Storage::Table);
                let entity = world.spawn((Position(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.remove::<Position>(entity));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.bench_function("remove_sparse", |b| {
        b.iter_batched(
            || {
                let mut world = World::new();
                world.register::<Health>(Storage::SparseSet);
                let entity = world.spawn((Health(1.0),));
                (world, entity)
            },
            |(mut world, entity)| {
                black_box(world.remove::<Health>(entity));
                world
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_queries(c: &mut Criterion) {
    let world = build_world();
    let mut group = c.benchmark_group("query");

    group.bench_function("table_position", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world.query::<(&Position,)>().for_each(|(p,)| sum += p.0);
            black_box(sum);
        });
    });
    group.bench_function("table_position_velocity", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world
                .query::<(&Position, &Velocity)>()
                .for_each(|(p, v)| sum += p.0 + v.0);
            black_box(sum);
        });
    });
    group.bench_function("table_position_velocity_mass", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world
                .query::<(&Position, &Velocity, &Mass)>()
                .for_each(|(p, v, m)| sum += p.0 + v.0 + m.0);
            black_box(sum);
        });
    });
    group.bench_function("hybrid_position_health", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world
                .query::<(&Position, &Health)>()
                .for_each(|(p, health)| sum += p.0 + health.0);
            black_box(sum);
        });
    });
    group.bench_function("sparse_health", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world.query::<(&Health,)>().for_each(|(h,)| sum += h.0);
            black_box(sum);
        });
    });
    group.bench_function("sparse_health_stamina", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            world
                .query::<(&Health, &Stamina)>()
                .for_each(|(h, s)| sum += h.0 + s.0);
            black_box(sum);
        });
    });
    group.bench_function("all_live_without_health", |b| {
        b.iter(|| {
            let mut count = 0usize;
            world
                .query::<(voxel_ecs::Entity, voxel_ecs::Without<Health>)>()
                .for_each(|_| count += 1);
            black_box(count);
        });
    });
    group.bench_function("fragment_iter", |b| {
        b.iter(|| {
            let query = world.query::<(&Position, &Velocity)>();
            let mut sum = 0.0f32;
            for (p, v) in query.iter() {
                sum += p.0 + v.0;
            }
            black_box(sum);
        });
    });
    group.finish();
}

fn bench_command_buffer(c: &mut Criterion) {
    let mut world = build_world();
    let mut group = c.benchmark_group("command_buffer");
    group.bench_function("flush_inserts", |b| {
        b.iter(|| {
            let mut cb = voxel_ecs::CommandBuffer::new();
            let entity = cb.spawn(&mut world, (Position(1.0), Velocity(2.0)));
            cb.insert(entity, Mass(3.0));
            let outcome = world.flush(&mut cb);
            black_box(outcome.drop_panics.len());
        });
    });
    group.finish();
}

fn bench_views(c: &mut Criterion) {
    let mut world = build_world();
    let entities: Vec<voxel_ecs::Entity> = world
        .query::<(voxel_ecs::Entity,)>()
        .iter()
        .map(|(entity,)| entity)
        .take(64)
        .collect();

    let mut group = c.benchmark_group("views");
    group.bench_function("entity_ref", |b| {
        b.iter(|| {
            for &entity in &entities {
                let view = world.entity_ref(entity);
                black_box(view.get::<Position>().map(|p| p.0));
            }
        });
    });
    group.bench_function("entity_mut", |b| {
        b.iter(|| {
            for &entity in &entities {
                let mut view = world.entity_mut(entity);
                if let Some(position) = view.get_mut::<Position>() {
                    position.0 += 1.0;
                }
            }
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_spawn,
    bench_structural,
    bench_queries,
    bench_command_buffer,
    bench_views
);
criterion_main!(benches);
