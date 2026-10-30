//! §11 — sparse sets through the public API (internal S1–S7 are unit-tested).

use hashbrown::HashMap;

use my_ecs::{Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct T(i32);
#[derive(Debug, PartialEq)]
struct S(i32);

#[test]
fn sparse_insert_get_remove() {
    let mut world = World::new();
    world.register::<S>(Storage::SparseSet);
    let entity = world.spawn(());
    assert_eq!(world.get::<S>(entity), None);
    assert!(world.insert(entity, S(1)).is_none());
    assert_eq!(world.get::<S>(entity), Some(&S(1)));
    assert_eq!(world.insert(entity, S(2)), Some(S(1)));
    assert_eq!(world.get::<S>(entity), Some(&S(2)));
    assert_eq!(world.remove::<S>(entity), Some(S(2)));
    assert_eq!(world.remove::<S>(entity), None);
}

#[test]
fn sparse_swap_remove_keeps_every_other_entity_correct() {
    let mut world = World::new();
    world.register::<T>(Storage::Table);
    world.register::<S>(Storage::SparseSet);

    let mut model: HashMap<Entity, i32> = HashMap::new();
    let mut entities = Vec::new();
    for index in 0..100 {
        let entity = world.spawn((T(index),));
        if index % 2 == 0 {
            world.insert(entity, S(index * 10));
            model.insert(entity, index * 10);
        }
        entities.push(entity);
    }

    // Remove sparse entries from the middle, then despawn some entities.
    for (index, entity) in entities.iter().enumerate() {
        if index % 6 == 0 {
            let expected = model.remove(entity);
            assert_eq!(world.remove::<S>(*entity).map(|s| s.0), expected);
        }
    }
    for (index, entity) in entities.iter().enumerate() {
        if index % 5 == 0 {
            assert!(world.despawn(*entity));
            model.remove(entity);
        }
    }

    let mut seen = 0;
    world.query::<(Entity, &S)>().for_each(|(entity, value)| {
        assert_eq!(model.get(&entity), Some(&value.0));
        seen += 1;
    });
    assert_eq!(seen, model.len());

    for entity in &entities {
        if world.query::<(Entity,)>().contains(*entity) {
            assert_eq!(
                world.get::<S>(*entity).map(|s| s.0),
                model.get(entity).copied()
            );
        }
    }
}

#[test]
fn despawn_removes_sparse_presence_so_reused_slots_do_not_resurrect() {
    let mut world = World::new();
    world.register::<S>(Storage::SparseSet);
    let old = world.spawn((S(1),));
    assert!(world.despawn(old));
    // Reuse the slot without adding S.
    let new = world.spawn(());
    assert_eq!(world.get::<S>(new), None);
    assert_eq!(world.query::<(&S,)>().iter().count(), 0);
    assert!(!world.query::<(&S,)>().contains(new));
}

#[test]
fn sparse_only_query_uses_smallest_set_anchor() {
    let mut world = World::new();
    world.register::<S>(Storage::SparseSet);
    world.register::<T>(Storage::SparseSet);

    let a = world.spawn((S(1), T(1)));
    let b = world.spawn((S(2),));
    let c = world.spawn((T(3),));

    let mut seen = 0;
    world.query::<(&S, &T)>().for_each(|(s, t)| {
        assert_eq!(s.0, 1);
        assert_eq!(t.0, 1);
        seen += 1;
    });
    assert_eq!(seen, 1);

    let mut entities = Vec::new();
    world
        .query::<(Entity, Option<&S>, Option<&T>)>()
        .for_each(|(entity, s, t)| {
            assert!(entity == a || entity == b || entity == c);
            if entity == a {
                assert!(s.is_some() && t.is_some());
            }
            if entity == b {
                assert!(s.is_some() && t.is_none());
            }
            if entity == c {
                assert!(s.is_none() && t.is_some());
            }
            entities.push(entity);
        });
    assert_eq!(entities.len(), 3);
}
