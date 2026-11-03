//! Deferred structural edits with `CommandBuffer`.
//!
//! Run with `cargo run --example deferred`.
//!
//! # WARNING
//!
//! A `CommandBuffer` deliberately has no `World` reference (the D1 design).
//! Dropping a buffer with un-flushed reservations **leaks those entity slots**
//! in release builds. Always call `World::flush` or `World::cancel` before
//! dropping a buffer. `#[must_use]` and a debug-only `Drop` assertion catch the
//! most common mistakes, but a buffer created, populated, and dropped through
//! a variable is only caught in debug builds.

use voxel_ecs::{CommandBuffer, Entity, Storage, World};

#[derive(Debug)]
struct Name(&'static str);
#[derive(Debug)]
struct Health(f32);
#[derive(Debug)]
struct Position(f32);

fn main() {
    let mut world = World::new();
    world.register::<Name>(Storage::Table);
    world.register::<Health>(Storage::SparseSet);
    world.register::<Position>(Storage::Table);

    // ── Spawn and insert, then flush ──────────────────────────────────────
    let mut cb = CommandBuffer::new();
    let orc = cb.spawn(&mut world, (Name("orc"), Health(100.0)));
    let elf = cb.spawn(&mut world, (Name("elf"), Health(80.0)));
    cb.insert(orc, Position(1.0));
    cb.insert(elf, Position(2.0));

    // Reserved entities are dead until materialization.
    println!(
        "before flush: orc alive = {}",
        world.query::<(Entity,)>().contains(orc)
    );

    let outcome = world.flush(&mut cb);
    println!(
        "after flush: orc alive = {}, drop panics = {}",
        world.query::<(Entity,)>().contains(orc),
        outcome.drop_panics.len()
    );

    // ── Deferred changes recorded while iterating ─────────────────────────
    // `insert`/`remove`/`despawn` do not borrow the world, so they can be
    // recorded from inside a query. (Spawning new entities cannot: reservation
    // requires `&mut World`, which a live query guard already holds.)
    let mut damage = CommandBuffer::new();
    world
        .query::<(Entity, &Name, &Health)>()
        .for_each(|(entity, name, health)| {
            println!("  applying damage to {}", name.0);
            if health.0 <= 90.0 {
                damage.despawn(entity);
            } else {
                damage.remove::<Health>(entity);
            }
        });
    world.flush(&mut damage);

    println!("survivors:");
    world.query::<(Entity, &Name)>().for_each(|(entity, name)| {
        println!("  {entity:?} {}", name.0);
    });

    // ── Last write wins per component ─────────────────────────────────────
    let mut changes = CommandBuffer::new();
    changes.insert(orc, Position(3.0));
    changes.insert(orc, Position(4.0));
    let outcome = world.flush(&mut changes);
    assert!(outcome.drop_panics.is_empty());
    println!("orc x = {}", world.get::<Position>(orc).unwrap().0);

    // ── Abort path: cancel releases the reserved slots ────────────────────
    let mut aborted = CommandBuffer::new();
    let ghost = aborted.spawn(&mut world, (Name("ghost"),));
    world.cancel(&mut aborted);
    println!(
        "cancelled ghost alive = {}",
        world.query::<(Entity,)>().contains(ghost)
    );

    // Always flush or cancel before the buffer goes out of scope.
}
