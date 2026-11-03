//! §7 — component registry, ZSTs, and drop glue.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use voxel_ecs::{Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);

struct D(Arc<AtomicUsize>);
impl Drop for D {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Zst;

#[test]
fn registration_is_idempotent() {
    let mut world = World::new();
    let a = world.register::<A>(Storage::Table);
    let b = world.register::<A>(Storage::Table);
    assert_eq!(a, b);
}

#[test]
#[should_panic(expected = "ecs: storage mismatch:")]
fn storage_mismatch_panics() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<A>(Storage::SparseSet);
}

#[test]
fn distinct_types_get_distinct_ids() {
    let mut world = World::new();
    assert_ne!(
        world.register::<A>(Storage::Table),
        world.register::<Zst>(Storage::Table)
    );
}

#[test]
fn table_zst_presence_and_drop() {
    let drops = Arc::new(AtomicUsize::new(0));
    struct ZD(Arc<AtomicUsize>);
    impl Drop for ZD {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut world = World::new();
    world.register::<Zst>(Storage::Table);
    world.register::<ZD>(Storage::Table);

    let entity = world.spawn((Zst, ZD(drops.clone())));
    let mut count = 0;
    world.query::<(&Zst,)>().for_each(|_| count += 1);
    assert_eq!(count, 1);

    // Replacing the ZST does not change presence.
    assert!(world.insert(entity, Zst).is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert!(world.despawn(entity));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(world.query::<(&Zst,)>().iter().next().is_none());
}

#[test]
fn sparse_zst_presence_and_drop() {
    let drops = Arc::new(AtomicUsize::new(0));
    struct ZD(Arc<AtomicUsize>);
    impl Drop for ZD {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut world = World::new();
    world.register::<Zst>(Storage::SparseSet);
    world.register::<ZD>(Storage::SparseSet);

    let entity = world.spawn((Zst, ZD(drops.clone())));
    assert!(world.get::<Zst>(entity).is_some());
    assert!(world.remove::<Zst>(entity).is_some());
    assert!(!world.query::<(&Zst,)>().contains(entity));
    assert!(world.despawn(entity));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn components_are_dropped_exactly_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut world = World::new();
    world.register::<D>(Storage::Table);
    for _ in 0..10 {
        world.spawn((D(drops.clone()),));
    }
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    world.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 10);
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn get_unregistered_panics() {
    struct Unregistered;
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    let entity = world.spawn((A(1),));
    let _ = world.get::<Unregistered>(entity);
}
