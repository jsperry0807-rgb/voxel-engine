//! §20 — property-style fuzzing through the public API.
//!
//! Every random operation is checked against a simple model; the internal
//! `debug_assert`-driven invariant checker also runs at the end of every flush
//! and clear in debug builds.

use hashbrown::HashMap;

use voxel_ecs::{Entity, Storage, World};

#[derive(Debug, Clone, Copy, PartialEq)]
struct A(u32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct B(u64);
#[derive(Debug, Clone, Copy, PartialEq)]
struct S(u8);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Model {
    a: Option<u32>,
    b: Option<u64>,
    s: Option<u8>,
}

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

fn verify(world: &World, model: &HashMap<Entity, Model>) {
    let mut seen = 0usize;
    world
        .query::<(Entity, Option<&A>, Option<&B>, Option<&S>)>()
        .for_each(|(entity, a, b, s)| {
            let expected = model.get(&entity).expect("query yielded an unknown entity");
            assert_eq!(expected.a, a.map(|value| value.0));
            assert_eq!(expected.b, b.map(|value| value.0));
            assert_eq!(expected.s, s.map(|value| value.0));
            seen += 1;
        });
    assert_eq!(seen, model.len());

    for (entity, expected) in model {
        assert_eq!(world.get::<A>(*entity).map(|value| value.0), expected.a);
        assert_eq!(world.get::<B>(*entity).map(|value| value.0), expected.b);
        assert_eq!(world.get::<S>(*entity).map(|value| value.0), expected.s);
    }
}

#[test]
#[cfg_attr(miri, ignore = "property fuzzing is too slow under Miri")]
fn random_public_operations_match_model() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::Table);
    world.register::<S>(Storage::SparseSet);

    let mut model: HashMap<Entity, Model> = HashMap::new();
    let mut alive: Vec<Entity> = Vec::new();
    let mut seed = 0x0123_4567_89ab_cdefu64;

    for step in 0..600u32 {
        let value = step.wrapping_mul(7) & 0xffff;
        match lcg(&mut seed) % 10 {
            0 => {
                let entity = world.spawn((A(value),));
                model.insert(
                    entity,
                    Model {
                        a: Some(value),
                        ..Model::default()
                    },
                );
                alive.push(entity);
            }
            1 => {
                let entity = world.spawn((A(value), B(u64::from(value))));
                model.insert(
                    entity,
                    Model {
                        a: Some(value),
                        b: Some(u64::from(value)),
                        s: None,
                    },
                );
                alive.push(entity);
            }
            2 => {
                let entity = world.spawn((A(value), S((value & 0xff) as u8)));
                model.insert(
                    entity,
                    Model {
                        a: Some(value),
                        b: None,
                        s: Some((value & 0xff) as u8),
                    },
                );
                alive.push(entity);
            }
            3 if !alive.is_empty() => {
                let entity = alive[(lcg(&mut seed) as usize) % alive.len()];
                let old = world.insert(entity, B(u64::from(value)));
                let expected = model.get_mut(&entity).unwrap();
                assert_eq!(old.map(|item| item.0), expected.b);
                expected.b = Some(u64::from(value));
            }
            4 if !alive.is_empty() => {
                let entity = alive[(lcg(&mut seed) as usize) % alive.len()];
                let old = world.insert(entity, S((value & 0xff) as u8));
                let expected = model.get_mut(&entity).unwrap();
                assert_eq!(old.map(|item| item.0), expected.s);
                expected.s = Some((value & 0xff) as u8);
            }
            5 if !alive.is_empty() => {
                let entity = alive[(lcg(&mut seed) as usize) % alive.len()];
                let old = world.remove::<A>(entity);
                let expected = model.get_mut(&entity).unwrap();
                assert_eq!(old.map(|item| item.0), expected.a);
                expected.a = None;
            }
            6 if !alive.is_empty() => {
                let entity = alive[(lcg(&mut seed) as usize) % alive.len()];
                let old = world.remove::<B>(entity);
                let expected = model.get_mut(&entity).unwrap();
                assert_eq!(old.map(|item| item.0), expected.b);
                expected.b = None;
            }
            7 if !alive.is_empty() => {
                let entity = alive[(lcg(&mut seed) as usize) % alive.len()];
                let old = world.remove::<S>(entity);
                let expected = model.get_mut(&entity).unwrap();
                assert_eq!(old.map(|item| item.0), expected.s);
                expected.s = None;
            }
            8 if !alive.is_empty() => {
                let index = (lcg(&mut seed) as usize) % alive.len();
                let entity = alive.swap_remove(index);
                assert!(world.despawn(entity));
                model.remove(&entity);
            }
            _ => {}
        }
        verify(&world, &model);
    }

    world.clear();
    model.clear();
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
    // Old handles are dead after clear.
    for entity in alive {
        assert!(!world.despawn(entity));
    }
    // The world remains usable.
    let fresh = world.spawn((A(1),));
    assert!(world.despawn(fresh));
}
