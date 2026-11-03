//! §12 — location bookkeeping after migration and swap-remove.
//!
//! `EntityLocation` itself is internal; this test observes it through the
//! public mapping between entities and their components.

use hashbrown::HashMap;

use voxel_ecs::{Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);
#[derive(Debug, PartialEq)]
struct C(u32);

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::Table);
    world.register::<C>(Storage::Table);
    world
}

#[test]
fn migration_updates_locations_for_the_moved_entity() {
    let mut world = world();
    let entities: Vec<Entity> = (0..50).map(|i| world.spawn((A(i),))).collect();

    // Migrate every second entity, which moves rows in both archetypes.
    for (i, &entity) in entities.iter().enumerate() {
        if i % 2 == 0 {
            world.insert(entity, B(i as u32));
        }
    }

    for (i, &entity) in entities.iter().enumerate() {
        assert_eq!(world.get::<A>(entity), Some(&A(i as u32)));
        if i % 2 == 0 {
            assert_eq!(world.get::<B>(entity), Some(&B(i as u32)));
        } else {
            assert_eq!(world.get::<B>(entity), None);
        }
    }

    let mut seen = 0;
    world.query::<(Entity, &A)>().for_each(|(entity, a)| {
        assert_eq!(a.0 as usize, entity_query_index(&entities, entity));
        seen += 1;
    });
    assert_eq!(seen, entities.len());
}

fn entity_query_index(entities: &[Entity], entity: Entity) -> usize {
    entities
        .iter()
        .position(|candidate| *candidate == entity)
        .expect("query yielded a known entity")
}

#[test]
fn removing_the_last_row_does_not_disturb_other_locations() {
    let mut world = world();
    let entities: Vec<Entity> = (0..30).map(|i| world.spawn((A(i), B(i)))).collect();

    // Remove from the middle repeatedly; the last row is moved each time.
    for index in [5usize, 10, 0, 20] {
        let entity = entities[index];
        assert_eq!(world.remove::<B>(entity), Some(B(index as u32)));
    }

    for (index, &entity) in entities.iter().enumerate() {
        assert_eq!(world.get::<A>(entity), Some(&A(index as u32)));
        if [5, 10, 0, 20].contains(&index) {
            assert_eq!(world.get::<B>(entity), None);
        } else {
            assert_eq!(world.get::<B>(entity), Some(&B(index as u32)));
        }
    }
}

#[test]
fn despawn_swap_remove_updates_survivor_locations() {
    let mut world = world();
    let entities: Vec<Entity> = (0..40).map(|i| world.spawn((A(i), B(i), C(i)))).collect();

    for index in (0..40).step_by(4) {
        assert!(world.despawn(entities[index]));
    }

    let mut expected = HashMap::new();
    for (index, &entity) in entities.iter().enumerate() {
        if index % 4 != 0 {
            expected.insert(entity, index as u32);
        }
    }

    let mut seen = 0;
    world
        .query::<(Entity, &A, &B, &C)>()
        .for_each(|(entity, a, b, c)| {
            let index = expected.get(&entity).expect("unknown entity");
            assert_eq!(a.0, *index);
            assert_eq!(b.0, *index);
            assert_eq!(c.0, *index);
            seen += 1;
        });
    assert_eq!(seen, expected.len());
}
