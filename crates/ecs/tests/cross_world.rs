//! §17.2/§17.13 — cross-world binding, staleness, and poisoning.

use voxel_ecs::{CommandBuffer, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
struct Unregistered;

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world
}

#[test]
fn spawn_binds_the_world() {
    let mut first = world();
    let mut second = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut first, (A(1),));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        second.flush(&mut cb);
    }));
    assert!(result.is_err());

    first.cancel(&mut cb);
    assert!(!first.despawn(entity));
}

#[test]
fn insert_only_buffer_binds_at_first_flush() {
    let mut first = world();
    let mut second = world();
    let entity = first.spawn(());

    let mut cb = CommandBuffer::new();
    cb.insert(entity, A(1));
    first.flush(&mut cb);
    assert_eq!(first.get::<A>(entity), Some(&A(1)));

    // Now bound to `first`; a flush into another world is rejected.
    let other = second.spawn(());
    cb.insert(other, A(2));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        second.flush(&mut cb);
    }));
    assert!(result.is_err());
}

#[test]
fn binding_persists_across_successful_flushes() {
    let mut first = world();
    let entity = first.spawn(());
    let mut cb = CommandBuffer::new();
    cb.insert(entity, A(1));
    first.flush(&mut cb);
    cb.insert(first.spawn(()), A(2));
    first.flush(&mut cb);
    assert_eq!(first.get::<A>(entity), Some(&A(1)));
}

#[test]
fn clear_makes_bound_buffers_stale() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, (A(3),));
    world.clear();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    assert!(result.is_err());
    world.cancel(&mut cb);
    assert!(!world.despawn(entity));
}

#[test]
fn panic_poisons_until_cancel() {
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.insert(entity, Unregistered);

    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    assert!(first.is_err());

    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    assert!(second.is_err());

    world.cancel(&mut cb);
    let outcome = world.flush(&mut cb);
    assert!(outcome.drop_panics.is_empty());
}

#[test]
fn successful_flush_does_not_unbind() {
    let mut first = world();
    let mut cb = CommandBuffer::new();
    let spawned = cb.spawn(&mut first, (A(1),));
    first.flush(&mut cb);
    assert_eq!(first.get::<A>(spawned), Some(&A(1)));
}
