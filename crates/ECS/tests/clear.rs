//! §18 — `World::clear`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use my_ecs::{Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct S(u32);

struct D(Arc<AtomicUsize>);
impl Drop for D {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn clear_despawns_everything_and_retains_registration() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<S>(Storage::SparseSet);

    let old = world.spawn((A(1), S(2)));
    world.spawn((A(3),));
    assert_eq!(world.query::<(Entity,)>().iter().count(), 2);

    world.clear();
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
    assert_eq!(world.get::<A>(old), None);
    assert!(!world.despawn(old));

    // Registration, archetypes, and capacities are retained: spawning again
    // uses the same component types without re-registering.
    let new = world.spawn((A(4), S(5)));
    assert_ne!(new, old);
    assert_eq!(world.get::<A>(new), Some(&A(4)));
    assert_eq!(world.get::<S>(new), Some(&S(5)));
    assert_eq!(world.query::<(&A,)>().iter().count(), 1);
}

#[test]
fn clear_invalidates_cached_plans() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.spawn((A(1),));
    assert_eq!(world.query::<(&A,)>().iter().count(), 1);
    world.clear();
    // The plan cache still exists but is stale; the next query rebuilds it.
    assert_eq!(world.query::<(&A,)>().iter().count(), 0);
    world.spawn((A(2),));
    assert_eq!(world.query::<(&A,)>().iter().count(), 1);
}

#[test]
fn clear_drops_components_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut world = World::new();
    world.register::<D>(Storage::Table);
    for _ in 0..7 {
        world.spawn((D(drops.clone()),));
    }
    world.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 7);
    world.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 7);
}

#[test]
fn clear_drops_sparse_components_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut world = World::new();
    world.register::<D>(Storage::SparseSet);
    for _ in 0..7 {
        world.spawn((D(drops.clone()),));
    }
    world.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 7);
    world.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 7);
}

#[test]
fn clear_survives_destructor_panics() {
    struct PanicTable;
    impl Drop for PanicTable {
        fn drop(&mut self) {
            panic!("intentional table drop panic during clear");
        }
    }
    struct PanicSparse;
    impl Drop for PanicSparse {
        fn drop(&mut self) {
            panic!("intentional sparse drop panic during clear");
        }
    }

    let mut world = World::new();
    world.register::<PanicTable>(Storage::Table);
    world.register::<PanicSparse>(Storage::SparseSet);
    world.spawn((PanicTable, PanicSparse));
    world.clear();
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
    // The world is structurally valid and reusable.
    let entity = world.spawn(());
    assert!(world.despawn(entity));
}

#[test]
fn clear_frees_reserved_slots() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    let mut cb = my_ecs::CommandBuffer::new();
    let reserved = cb.spawn(&mut world, (A(1),));
    world.clear();
    // The reserved slot is gone; the buffer is stale.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = world.flush(&mut cb);
    }));
    assert!(result.is_err());
    world.cancel(&mut cb);
    assert!(!world.despawn(reserved));
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
}
