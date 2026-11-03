//! Duplicate-component detection at the query and bundle boundaries (§13.9).

use voxel_ecs::{Entity, Storage, With, Without, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::SparseSet);
    world
}

#[test]
#[should_panic(expected = "ecs: duplicate component in bundle:")]
fn bundle_duplicate_table() {
    let mut world = world();
    let _ = world.spawn((A(1), A(2)));
}

#[test]
#[should_panic(expected = "ecs: duplicate component in bundle:")]
fn bundle_duplicate_sparse() {
    let mut world = world();
    let _ = world.spawn((B(1), B(2)));
}

#[test]
#[should_panic(expected = "ecs: duplicate component in bundle:")]
fn bundle_duplicate_across_storage_queries_still_duplicates() {
    struct Mixed;
    let mut world = World::new();
    world.register::<Mixed>(Storage::Table);
    let _ = world.spawn((Mixed, Mixed));
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn query_duplicate_read() {
    let world = world();
    let _ = world.query::<(&A, &A)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn query_duplicate_mut() {
    let mut world = world();
    let _ = world.query_mut::<(&mut A, &mut A)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn query_duplicate_read_mut_mix() {
    let mut world = world();
    let _ = world.query_mut::<(&A, &mut A)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn query_duplicate_entity() {
    let world = world();
    let _ = world.query::<(Entity, Entity)>();
}

#[test]
fn filters_are_not_fetch_terms() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let mut count = 0;
    world.query::<(&A, With<A>)>().for_each(|_| count += 1);
    assert_eq!(count, 1);
    assert!(!world.query::<(&A, Without<A>)>().contains(entity));
}
