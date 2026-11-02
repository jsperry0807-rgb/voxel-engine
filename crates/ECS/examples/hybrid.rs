//! Table + sparse-set storage and hybrid queries.
//!
//! Run with `cargo run --example hybrid`.

use my_ecs::{Entity, Storage, Without, World};

#[derive(Debug)]
struct Position(f32, f32);

#[derive(Debug)]
struct Health(f32);

#[derive(Debug)]
struct Stamina(f32);

#[derive(Debug)]
struct Frozen;

fn main() {
    let mut world = World::new();
    world.register::<Position>(Storage::Table);
    world.register::<Health>(Storage::SparseSet);
    world.register::<Stamina>(Storage::SparseSet);
    world.register::<Frozen>(Storage::SparseSet);

    let orc = world.spawn((Position(1.0, 1.0), Health(100.0), Stamina(50.0)));
    let ghost = world.spawn((Position(5.0, 5.0), Frozen));
    let rock = world.spawn((Position(9.0, 9.0),));

    // Hybrid: table anchor, sparse presence checked per row.
    println!("Living entities:");
    world
        .query::<(Entity, &Position, &Health)>()
        .for_each(|(entity, position, health)| {
            println!(
                "  {entity:?} pos=({}, {}) hp={}",
                position.0, position.1, health.0
            );
        });

    // Sparse-only query: anchored on the smallest required sparse set.
    println!("Stamina:");
    world
        .query::<(Entity, &Stamina)>()
        .for_each(|(entity, stamina)| {
            println!("  {entity:?} stamina={}", stamina.0);
        });

    // `Without` works for both table and sparse components.
    println!("Entities without Health:");
    world
        .query::<(Entity, Without<Health>)>()
        .for_each(|(entity, ())| println!("  {entity:?}"));

    // Adding/removing a sparse component never migrates the entity.
    assert!(world.insert(orc, Frozen).is_none());
    assert!(world.remove::<Frozen>(orc).is_some());

    // Re-registering the same sparse component is idempotent.
    world.register::<Stamina>(Storage::SparseSet);
    assert!(world.get::<Position>(rock).is_some());
    assert!(world.get::<Health>(rock).is_none());

    assert!(world.despawn(ghost));
}
