//! §14 — borrow tracking through the public API.
//!
//! Most conflicts are prevented at compile time by `&World` vs `&mut World`.
//! The internal `BorrowMap` conflict matrix is unit-tested in `src/borrow.rs`;
//! these tests cover the allowed nesting and the release-on-drop protocol.

use my_ecs::{Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::Table);
    world
}

#[test]
fn multiple_read_queries_coexist() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let first = world.query::<(&A,)>();
    let second = world.query::<(&A, &B)>();
    assert!(first.contains(entity));
    assert!(second.contains(entity));
    assert_eq!(first.get::<A>(entity), Some(&A(1)));
    assert_eq!(second.get::<B>(entity), Some(&B(2)));
}

#[test]
fn entity_ref_coexists_with_read_query() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let view = world.entity_ref(entity);
    let query = world.query::<(&A, &B)>();
    assert_eq!(view.get::<A>(), Some(&A(1)));
    assert_eq!(query.get::<A>(entity), Some(&A(1)));
}

#[test]
fn guard_drop_releases_borrows() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    {
        let query = world.query::<(&A, &B)>();
        assert!(query.contains(entity));
    }
    {
        let view = world.entity_ref(entity);
        assert!(view.get::<A>().is_some());
    }
    // A mutable guard can be created once all shared borrows are gone.
    {
        let mut query = world.query_mut::<(&mut A,)>();
        query.for_each(|(a,)| a.0 += 1);
    }
    assert_eq!(world.get::<A>(entity), Some(&A(2)));
}

#[test]
fn query_get_does_not_acquire_a_second_borrow() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let query = world.query::<(&A,)>();
    // `World::get` performs a short-lived conflict check; the query's read
    // borrow is compatible.
    assert_eq!(query.get::<A>(entity), Some(&A(1)));
    assert_eq!(world.get::<A>(entity), Some(&A(1)));
}

#[test]
fn nested_read_queries_with_filters() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let without = world.query::<(&A, my_ecs::Without<B>)>();
    let with = world.query::<(&A, my_ecs::With<B>)>();
    assert!(!without.contains(entity));
    assert!(with.contains(entity));
}
