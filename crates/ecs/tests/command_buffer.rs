//! §17 — command buffer lifecycle, coalescing, poisoning, and D1 leaks.

use voxel_ecs::{CommandBuffer, Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct S(u32);

struct PanicDrop;
impl Drop for PanicDrop {
    fn drop(&mut self) {
        panic!("intentional deferred drop panic");
    }
}

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<S>(Storage::SparseSet);
    world
}

fn catch_flush(world: &mut World, cb: &mut CommandBuffer) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = world.flush(cb);
    }))
    .is_err()
}

#[test]
fn spawn_is_reserved_then_materialized_on_flush() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, (A(1),));

    assert!(!world.query::<(Entity,)>().contains(entity));
    assert_eq!(world.get::<A>(entity), None);

    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert!(world.query::<(Entity,)>().contains(entity));
    assert_eq!(world.get::<A>(entity), Some(&A(1)));
}

#[test]
fn deferred_insert_remove_despawn_apply_in_order() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let victim = world.spawn((A(2),));

    let mut cb = CommandBuffer::new();
    let spawned = cb.spawn(&mut world, (A(3),));
    cb.insert(entity, A(10));
    cb.remove::<A>(victim);
    cb.despawn(victim);
    cb.insert(spawned, S(4));
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());

    assert_eq!(world.get::<A>(entity), Some(&A(10)));
    assert!(!world.query::<(Entity,)>().contains(victim));
    assert_eq!(world.get::<A>(spawned), Some(&A(3)));
    assert_eq!(world.get::<S>(spawned), Some(&S(4)));
}

#[test]
fn last_write_wins_per_component() {
    let mut world = world();
    let entity = world.spawn(());

    let mut cb = CommandBuffer::new();
    cb.insert(entity, A(1));
    cb.insert(entity, A(2));
    world.flush(&mut cb);
    assert_eq!(world.get::<A>(entity), Some(&A(2)));

    let mut cb = CommandBuffer::new();
    cb.remove::<A>(entity);
    cb.insert(entity, A(3));
    world.flush(&mut cb);
    assert_eq!(world.get::<A>(entity), Some(&A(3)));

    let mut cb = CommandBuffer::new();
    cb.insert(entity, A(4));
    cb.remove::<A>(entity);
    world.flush(&mut cb);
    assert_eq!(world.get::<A>(entity), None);
}

#[test]
fn duplicate_despawn_is_coalesced() {
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.despawn(entity);
    cb.despawn(entity);
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert!(!world.query::<(Entity,)>().contains(entity));
}

#[test]
fn despawn_of_already_dead_entity_is_a_no_op() {
    let mut world = world();
    let entity = world.spawn(());
    world.despawn(entity);

    let mut cb = CommandBuffer::new();
    cb.despawn(entity);
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert!(world.query::<(Entity,)>().iter().count() == 0);
}

#[test]
fn spawn_and_despawn_in_the_same_buffer_cancel_out_and_free_the_slot() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, (A(1),));
    cb.despawn(entity);
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert!(!world.query::<(Entity,)>().contains(entity));
    assert!(!world.despawn(entity));
    // The reservation was freed, so further spawns still work.
    let other = world.spawn((A(2),));
    assert!(world.query::<(Entity,)>().contains(other));
}

#[test]
fn cancel_is_idempotent_and_releases_reservations() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, (A(1),));
    cb.insert(entity, A(2));

    world.cancel(&mut cb);
    world.cancel(&mut cb);

    assert!(!world.despawn(entity));
    assert_eq!(world.get::<A>(entity), None);
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
}

#[test]
#[should_panic(expected = "ecs: buffer ordering:")]
fn insert_after_despawn_panics_at_record_time() {
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.despawn(entity);
    cb.insert(entity, A(1));
}

#[test]
fn unregistered_insert_panics_at_flush_and_poisons() {
    struct Unregistered;
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.insert(entity, Unregistered);

    assert!(catch_flush(&mut world, &mut cb));
    // Poisoned buffers refuse to flush again.
    assert!(catch_flush(&mut world, &mut cb));
    world.cancel(&mut cb);
    // After cancel the buffer is reusable.
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn unregistered_spawn_bundle_panics_at_record_time() {
    struct Unregistered;
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let _ = cb.spawn(&mut world, (Unregistered,));
}

#[test]
fn drop_panic_of_replaced_value_is_collected() {
    let mut world = World::new();
    world.register::<PanicDrop>(Storage::Table);
    let entity = world.spawn((PanicDrop,));

    let mut cb = CommandBuffer::new();
    cb.insert(entity, PanicDrop);
    let outcome = world.flush(&mut cb);
    assert_eq!(outcome.drop_panics.len(), 1);

    // Take the new value out so dropping the world does not panic again.
    let value = world.remove::<PanicDrop>(entity).expect("present");
    std::mem::forget(value);
}

#[test]
fn cross_world_flush_panics() {
    let mut first = world();
    let mut second = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut first, (A(1),));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = second.flush(&mut cb);
    }));
    assert!(result.is_err());

    // The reservation still belongs to `first`; cancel it there.
    first.cancel(&mut cb);
    assert!(!first.despawn(entity));
}

#[test]
fn insert_only_buffer_binds_on_first_flush_and_sticks() {
    let mut first = world();
    let mut second = world();
    let e1 = first.spawn(());
    let e2 = second.spawn(());

    let mut cb = CommandBuffer::new();
    cb.insert(e1, A(1));
    first.flush(&mut cb);

    cb.insert(e2, A(2));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = second.flush(&mut cb);
    }));
    assert!(result.is_err());
}

#[test]
fn clear_invalidates_bound_buffers() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, (A(1),));
    world.clear();

    assert!(catch_flush(&mut world, &mut cb));
    // `clear` already reclaimed the reserved slot; cancel must not double-free.
    world.cancel(&mut cb);
    assert!(!world.despawn(entity));
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
}

#[test]
fn empty_buffer_flush_is_harmless() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "ecs: command buffer leak:")]
fn dropping_unflushed_buffer_asserts_in_debug() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let _entity = cb.spawn(&mut world, (A(1),));
    drop(cb);
}
