//! §13 — query forms, filters, anchors, access sets, and mutation.

use hashbrown::HashMap;

use my_ecs::{Entity, Storage, With, Without, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);
#[derive(Debug, PartialEq)]
struct M(u32);
#[derive(Debug, PartialEq)]
struct S(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::Table);
    world.register::<M>(Storage::Table);
    world.register::<S>(Storage::SparseSet);
    world
}

#[test]
fn pure_table_query() {
    let mut world = world();
    world.spawn((A(1), B(10)));
    world.spawn((A(2), B(20)));
    world.spawn((A(3),));

    let mut pairs = Vec::new();
    world
        .query::<(&A, &B)>()
        .for_each(|(a, b)| pairs.push((a.0, b.0)));
    pairs.sort_unstable();
    assert_eq!(pairs, vec![(1, 10), (2, 20)]);
}

#[test]
fn with_and_without_table_filters() {
    let mut world = world();
    let ab = world.spawn((A(1), B(1)));
    let a_only = world.spawn((A(2),));

    let mut with = Vec::new();
    world
        .query::<(Entity, With<B>)>()
        .for_each(|(entity, ())| with.push(entity));
    assert_eq!(with, vec![ab]);

    let mut without = Vec::new();
    world
        .query::<(Entity, Without<B>)>()
        .for_each(|(entity, ())| without.push(entity));
    assert_eq!(without, vec![a_only]);
}

#[test]
fn optional_table_and_sparse_terms_yield_none() {
    let mut world = world();
    let both = world.spawn((A(1), B(1), S(1)));
    let a_only = world.spawn((A(2),));

    let mut seen = HashMap::new();
    world
        .query::<(Entity, Option<&B>, Option<&S>)>()
        .for_each(|(entity, b, s)| {
            seen.insert(entity, (b.map(|b| b.0), s.map(|s| s.0)));
        });
    assert_eq!(seen[&both], (Some(1), Some(1)));
    assert_eq!(seen[&a_only], (None, None));
}

#[test]
fn hybrid_query_checks_sparse_presence_per_row() {
    let mut world = world();
    let with = world.spawn((A(1), S(10)));
    let without = world.spawn((A(2),));

    let mut seen = Vec::new();
    world
        .query::<(Entity, &A, &S)>()
        .for_each(|(entity, a, s)| seen.push((entity, a.0, s.0)));
    assert_eq!(seen, vec![(with, 1, 10)]);
    assert!(seen.iter().all(|(entity, _, _)| *entity != without));
}

#[test]
fn sparse_filters_are_row_level() {
    let mut world = world();
    let with = world.spawn((S(1),));
    let without = world.spawn(());

    let mut seen = Vec::new();
    world
        .query::<(Entity, With<S>)>()
        .for_each(|(entity, ())| seen.push(entity));
    assert_eq!(seen, vec![with]);

    let mut seen = Vec::new();
    world
        .query::<(Entity, Without<S>)>()
        .for_each(|(entity, ())| seen.push(entity));
    assert_eq!(seen, vec![without]);
}

#[test]
fn entity_only_query_scans_all_live_entities() {
    let mut world = world();
    for index in 0..10 {
        world.spawn((A(index),));
    }
    world.spawn(());
    let count = world.query::<(Entity,)>().iter().count();
    assert_eq!(count, 11);
    // A despawn is reflected immediately.
    let victim = world.query::<(Entity,)>().iter().next().unwrap().0;
    world.despawn(victim);
    assert_eq!(world.query::<(Entity,)>().iter().count(), 10);
}

#[test]
fn empty_query_yields_every_live_entity_once() {
    let mut world = world();
    for _ in 0..5 {
        world.spawn(());
    }
    let mut seen = Vec::new();
    world.query::<()>().for_each(|()| seen.push(()));
    assert_eq!(seen.len(), 5);
}

#[test]
fn try_for_each_stops_at_break() {
    let mut world = world();
    for index in 0..10 {
        world.spawn((A(index),));
    }
    let mut count = 0;
    let result = world.query::<(&A,)>().try_for_each(|(a,)| {
        count += 1;
        if a.0 == 2 {
            std::ops::ControlFlow::Break(a.0)
        } else {
            std::ops::ControlFlow::Continue(())
        }
    });
    assert_eq!(result, std::ops::ControlFlow::Break(2));
    assert!(count <= 10);
}

#[test]
fn query_get_and_contains_respect_the_plan() {
    let mut world = world();
    let ab = world.spawn((A(1), B(10)));
    let a_only = world.spawn((A(2),));
    let dead = world.spawn((A(3), B(30)));
    world.despawn(dead);

    let query = world.query::<(&A, &B)>();
    assert!(query.contains(ab));
    assert!(!query.contains(a_only));
    assert_eq!(query.get::<A>(ab), Some(&A(1)));
    assert_eq!(query.get::<B>(ab), Some(&B(10)));
    // `a_only` does not match the query, even though it has A.
    assert_eq!(query.get::<A>(a_only), None);
    // Dead entities return None.
    assert_eq!(query.get::<A>(dead), None);
}

#[test]
fn read_only_query_does_not_apply_writes() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let mut query = world.query_mut::<(&mut A,)>();
    query.for_each(|(a,)| a.0 += 10);
    drop(query);
    assert_eq!(world.get::<A>(entity), Some(&A(11)));
}

#[test]
fn query_mut_get_and_get_mut() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));

    let mut query = world.query_mut::<(&mut A, &B)>();
    assert_eq!(query.get::<A>(entity), Some(&A(1)));
    assert_eq!(query.get::<B>(entity), Some(&B(2)));
    if let Some(a) = query.get_mut::<A>(entity) {
        a.0 = 5;
    }
    assert_eq!(query.get::<A>(entity), Some(&A(5)));
}

#[test]
fn query_mut_optional_mutation() {
    let mut world = world();
    let with = world.spawn((A(1), B(1)));
    let without = world.spawn((A(2),));

    world
        .query_mut::<(&mut A, Option<&mut B>)>()
        .for_each(|(a, b)| {
            a.0 += 100;
            if let Some(b) = b {
                b.0 += 100;
            }
        });
    assert_eq!(world.get::<A>(with), Some(&A(101)));
    assert_eq!(world.get::<B>(with), Some(&B(101)));
    assert_eq!(world.get::<A>(without), Some(&A(102)));
    assert_eq!(world.get::<B>(without), None);
}

#[test]
fn query_mut_sees_migrations_in_the_next_query() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    world.insert(entity, M(9));
    let mut query = world.query_mut::<(&mut A, &M)>();
    assert!(query.contains(entity));
    query.for_each(|(a, m)| {
        a.0 += m.0;
    });
    drop(query);
    assert_eq!(world.get::<A>(entity), Some(&A(10)));
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn duplicate_table_component_panics() {
    let world = world();
    let _ = world.query::<(&A, &A)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn duplicate_entity_term_panics() {
    let world = world();
    let _ = world.query::<(Entity, Entity)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn duplicate_optional_component_panics() {
    let world = world();
    let _ = world.query::<(&A, Option<&A>)>();
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn unregistered_query_term_panics() {
    struct Unregistered;
    let world = world();
    let _ = world.query::<(&Unregistered,)>();
}

#[test]
#[should_panic(expected = "ecs: query access violation:")]
fn query_get_outside_read_set_panics() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let query = world.query::<(&A,)>();
    let _ = query.get::<B>(entity);
}

#[test]
#[should_panic(expected = "ecs: query access violation:")]
fn query_mut_get_mut_outside_write_set_panics() {
    let mut world = world();
    let entity = world.spawn((A(1), B(2)));
    let mut query = world.query_mut::<(&A,)>();
    let _ = query.get_mut::<B>(entity);
}

#[test]
fn filters_do_not_count_as_duplicate_fetch_terms() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let mut count = 0;
    world.query::<(&A, With<A>)>().for_each(|(a, ())| {
        assert_eq!(a.0, 1);
        count += 1;
    });
    assert_eq!(count, 1);
    assert!(!world.query::<(&A, Without<A>)>().contains(entity));
}

#[test]
fn plans_survive_and_rebuild_across_new_archetypes() {
    let mut world = world();
    world.spawn((A(1),));
    assert_eq!(world.query::<(&A,)>().iter().count(), 1);
    // Creating a new archetype invalidates cached plans; the next query must
    // see it.
    world.spawn((A(2), B(2)));
    assert_eq!(world.query::<(&A,)>().iter().count(), 2);
    assert_eq!(world.query::<(&B,)>().iter().count(), 1);
}

#[test]
fn plans_rebuild_after_a_new_registration() {
    struct Late(u8);
    let mut world = world();
    let entity = world.spawn((A(1),));
    assert_eq!(world.query::<(&A,)>().iter().count(), 1);

    world.register::<Late>(Storage::Table);
    world.insert(entity, Late(2));

    let mut seen = 0;
    world.query::<(&A, &Late)>().for_each(|(a, late)| {
        assert_eq!(a.0, 1);
        assert_eq!(late.0, 2);
        seen += 1;
    });
    assert_eq!(seen, 1);
}

#[test]
fn sparse_and_table_mix_in_optional_position() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    world.insert(entity, S(3));
    let mut seen = 0;
    world.query::<(&A, Option<&S>)>().for_each(|(a, s)| {
        assert_eq!(a.0, 1);
        assert_eq!(s.map(|s| s.0), Some(3));
        seen += 1;
    });
    assert_eq!(seen, 1);
}
