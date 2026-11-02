//! Basic lifecycle: register, spawn, query, mutate, despawn.
//!
//! Run with `cargo run --example basic`.

use my_ecs::{Entity, Storage, World};

#[derive(Debug)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Debug)]
struct Velocity {
    dx: f32,
    dy: f32,
}

#[derive(Debug)]
struct Name(&'static str);

fn main() {
    let mut world = World::new();
    world.register::<Position>(Storage::Table);
    world.register::<Velocity>(Storage::Table);
    world.register::<Name>(Storage::Table);

    let player = world.spawn((
        Name("player"),
        Position { x: 0.0, y: 0.0 },
        Velocity { dx: 1.0, dy: 0.5 },
    ));
    let rock = world.spawn((Name("rock"), Position { x: 3.0, y: 2.0 }));

    println!("Before: {player:?} {rock:?}");
    world
        .query::<(Entity, &Name, &Position)>()
        .for_each(|(entity, name, position)| {
            println!(
                "  {entity:?} {} at ({}, {})",
                name.0, position.x, position.y
            );
        });

    // A mutable query updates every entity that has both components.
    world
        .query_mut::<(&mut Position, &Velocity)>()
        .for_each(|(position, velocity)| {
            position.x += velocity.dx;
            position.y += velocity.dy;
        });

    println!("After one tick:");
    world
        .query::<(Entity, &Name, &Position)>()
        .for_each(|(entity, name, position)| {
            println!(
                "  {entity:?} {} at ({}, {})",
                name.0, position.x, position.y
            );
        });

    // `With` / `Without` filters express presence without yielding the value.
    let movers = world
        .query::<(&Name, my_ecs::With<Velocity>)>()
        .iter()
        .count();
    println!("movers: {movers}");

    assert!(world.despawn(rock));
    assert!(!world.despawn(rock), "second despawn is a no-op");
    println!(
        "After despawn: {} entities",
        world.query::<(Entity,)>().iter().count()
    );
}
