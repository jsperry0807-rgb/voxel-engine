//! §9 — archetype graph and migration.

use my_ecs::{CommandBuffer, Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct P(i32);
#[derive(Debug, PartialEq)]
struct V(i32);
#[derive(Debug, PartialEq)]
struct W(i32);
#[derive(Debug, PartialEq)]
struct S(i32);

struct PanicDrop;
impl Drop for PanicDrop {
    fn drop(&mut self) {
        panic!("intentional drop panic");
    }
}

fn world() -> World {
    let mut world = World::new();
    world.register::<P>(Storage::Table);
    world.register::<V>(Storage::Table);
    world.register::<W>(Storage::Table);
    world.register::<S>(Storage::SparseSet);
    world
}

#[test]
fn table_insert_migrates_and_preserves_data() {
    let mut world = world();
    let entity = world.spawn((P(1), V(2)));
    assert!(world.insert(entity, W(3)).is_none());
    assert_eq!(world.get::<P>(entity), Some(&P(1)));
    assert_eq!(world.get::<V>(entity), Some(&V(2)));
    assert_eq!(world.get::<W>(entity), Some(&W(3)));
}

#[test]
fn table_remove_migrates_and_preserves_data() {
    let mut world = world();
    let entity = world.spawn((P(1), V(2), W(3)));
    assert_eq!(world.remove::<V>(entity), Some(V(2)));
    assert_eq!(world.get::<P>(entity), Some(&P(1)));
    assert_eq!(world.get::<W>(entity), Some(&W(3)));
    assert_eq!(world.get::<V>(entity), None);
    assert_eq!(world.remove::<V>(entity), None);
}

#[test]
fn sparse_changes_never_migrate() {
    let mut world = world();
    let entity = world.spawn((P(1),));
    // Before/after identity is not observable directly, but the entity and
    // table data must survive, and sparse components never filter archetypes.
    assert!(world.insert(entity, S(7)).is_none());
    assert_eq!(world.get::<S>(entity), Some(&S(7)));
    assert_eq!(world.remove::<S>(entity), Some(S(7)));
    assert_eq!(world.get::<P>(entity), Some(&P(1)));
    // An entity without P and one with P but no S both match `(Entity,
    // Without<S>)`.
    let other = world.spawn((V(2),));
    let mut seen = Vec::new();
    world
        .query::<(Entity, my_ecs::Without<S>)>()
        .for_each(|(e, ())| seen.push(e));
    assert!(seen.contains(&entity));
    assert!(seen.contains(&other));
}

#[test]
fn migration_leaves_other_entities_intact() {
    let mut world = world();
    let entities: Vec<Entity> = (0..64).map(|i| world.spawn((P(i), V(-i)))).collect();
    // Migrate every other entity by adding W.
    for (i, &entity) in entities.iter().enumerate() {
        if i % 2 == 0 {
            assert!(world.insert(entity, W(i as i32)).is_none());
        }
    }
    // Remove P from a third of them (also migrates).
    for (i, &entity) in entities.iter().enumerate() {
        if i % 3 == 0 {
            assert_eq!(world.remove::<P>(entity), Some(P(i as i32)));
        }
    }
    for (i, &entity) in entities.iter().enumerate() {
        assert_eq!(world.get::<V>(entity), Some(&V(-(i as i32))));
        if i % 3 == 0 {
            assert_eq!(world.get::<P>(entity), None);
        } else {
            assert_eq!(world.get::<P>(entity), Some(&P(i as i32)));
        }
        if i % 2 == 0 {
            assert_eq!(world.get::<W>(entity), Some(&W(i as i32)));
        } else {
            assert_eq!(world.get::<W>(entity), None);
        }
    }
}

#[test]
fn removed_value_is_returned_not_dropped_immediately() {
    let mut world = World::new();
    world.register::<PanicDrop>(Storage::Table);
    let entity = world.spawn((PanicDrop,));
    let value = world.remove::<PanicDrop>(entity).expect("present");
    std::mem::forget(value);
    assert!(world.get::<PanicDrop>(entity).is_none());
}

#[test]
fn drop_panic_during_flush_is_collected() {
    let mut world = World::new();
    world.register::<PanicDrop>(Storage::Table);
    let entity = world.spawn((PanicDrop,));

    let mut cb = CommandBuffer::new();
    cb.remove::<PanicDrop>(entity);
    let outcome = world.flush(&mut cb);
    assert_eq!(outcome.drop_panics.len(), 1);
    // The entity remains alive and the component is gone.
    assert!(world.query::<(Entity,)>().contains(entity));
    assert!(world.get::<PanicDrop>(entity).is_none());
}

#[test]
fn immediate_despawn_re_raises_drop_panic_after_structural_removal() {
    let mut world = World::new();
    world.register::<PanicDrop>(Storage::Table);
    let entity = world.spawn((PanicDrop,));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.despawn(entity);
    }));
    assert!(result.is_err());
    // Structural removal happened before the destructor ran.
    assert!(!world.query::<(Entity,)>().contains(entity));
    let replacement = world.spawn(());
    assert!(world.despawn(replacement));
}

#[test]
fn drop_panic_during_flush_despawn_is_collected() {
    let mut world = World::new();
    world.register::<PanicDrop>(Storage::Table);
    let entity = world.spawn((PanicDrop,));

    let mut cb = CommandBuffer::new();
    cb.despawn(entity);
    let outcome = world.flush(&mut cb);
    assert_eq!(outcome.drop_panics.len(), 1);
    assert!(!world.query::<(Entity,)>().contains(entity));
    // The world is still usable.
    let replacement = world.spawn(());
    assert!(world.despawn(replacement));
}
