//! §15 — `EntityRef` / `EntityMut`.

use voxel_ecs::{Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);
#[derive(Debug, PartialEq)]
struct S(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::Table);
    world.register::<S>(Storage::SparseSet);
    world
}

#[test]
fn entity_ref_reads_table_and_sparse_components() {
    let mut world = world();
    let entity = world.spawn((A(1), S(3)));

    let view = world.entity_ref(entity);
    assert_eq!(view.entity(), entity);
    assert_eq!(view.get::<A>(), Some(&A(1)));
    assert_eq!(view.get::<B>(), None);
    assert_eq!(view.get::<S>(), Some(&S(3)));
    assert!(view.get::<Unregistered>().is_none());
}

struct Unregistered;

#[test]
fn entity_mut_reads_and_writes() {
    let mut world = world();
    let entity = world.spawn((A(1), S(3)));

    {
        let mut view = world.entity_mut(entity);
        assert_eq!(view.get::<A>(), Some(&A(1)));
        assert_eq!(view.get::<S>(), Some(&S(3)));
        if let Some(a) = view.get_mut::<A>() {
            a.0 = 42;
        }
        if let Some(s) = view.get_mut::<S>() {
            s.0 = 43;
        }
        assert_eq!(view.get::<A>(), Some(&A(42)));
        assert_eq!(view.get::<B>(), None);
    }

    assert_eq!(world.get::<A>(entity), Some(&A(42)));
    assert_eq!(world.get::<S>(entity), Some(&S(43)));
}

#[test]
fn entity_mut_sequential_mutable_borrows() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let mut view = world.entity_mut(entity);
    {
        let a = view.get_mut::<A>().unwrap();
        a.0 += 1;
    }
    {
        let a = view.get_mut::<A>().unwrap();
        a.0 += 1;
    }
    drop(view);
    assert_eq!(world.get::<A>(entity), Some(&A(3)));
}

#[test]
fn dead_entities_panic_for_views() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    world.despawn(entity);

    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = world.entity_ref(entity);
    }));
    assert!(read.is_err());

    let write = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = world.entity_mut(entity);
    }));
    assert!(write.is_err());
}

#[test]
fn view_borrows_are_released_on_drop() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    {
        let view = world.entity_ref(entity);
        assert!(view.get::<A>().is_some());
    }
    // The read borrows are gone, so a mutable view can be acquired.
    {
        let mut view = world.entity_mut(entity);
        view.get_mut::<A>().unwrap().0 = 9;
    }
    assert_eq!(world.get::<A>(entity), Some(&A(9)));
}
