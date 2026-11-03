//! §6 — entity allocator, exercised through the public API.

use voxel_ecs::{Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world
}

#[test]
fn entity_is_eight_bytes_and_copy() {
    assert_eq!(core::mem::size_of::<Entity>(), 8);
    fn assert_copy<T: Copy + Eq + core::hash::Hash>() {}
    assert_copy::<Entity>();
}

#[test]
fn despawn_invalidates_old_handle() {
    let mut world = world();
    let old = world.spawn((A(1),));
    assert!(world.despawn(old));
    let new = world.spawn((A(2),));
    assert_ne!(old, new);
    assert_eq!(world.get::<A>(old), None);
    assert!(!world.despawn(old));
}

#[test]
fn spawn_reuses_slot_with_new_generation() {
    let mut world = world();
    let first = world.spawn((A(1),));
    world.despawn(first);
    let second = world.spawn((A(2),));
    // The allocator is LIFO, so the index is reused, but the generation is
    // bumped and the old handle stays dead.
    assert_ne!(first, second);
    assert_eq!(world.get::<A>(first), None);
    assert_eq!(world.get::<A>(second), Some(&A(2)));
}

#[test]
fn dead_entity_insert_and_remove_panic() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    world.despawn(entity);

    let insert = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.insert(entity, A(2));
    }));
    assert!(insert.is_err());

    let remove = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.remove::<A>(entity);
    }));
    assert!(remove.is_err());
}

#[test]
fn reserved_entities_are_dead_until_flush() {
    let mut world = world();
    let mut cb = voxel_ecs::CommandBuffer::new();
    let reserved = cb.spawn(&mut world, (A(9),));
    assert_eq!(world.get::<A>(reserved), None);
    assert!(!world.despawn(reserved));

    let view = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = world.entity_ref(reserved);
    }));
    assert!(view.is_err());

    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
    assert_eq!(world.get::<A>(reserved), Some(&A(9)));
}
