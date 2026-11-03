//! Internal invariant checkers.
//!
//! These functions are compiled in tests and debug builds and are called after
//! every flush/clear plus by property tests after every random operation. They
//! are never called during query iteration.

#![cfg_attr(not(any(test, debug_assertions)), allow(unused_imports, dead_code))]

use crate::entity::SlotState;
use crate::location::EntityLocation;
use crate::prelude::*;
use crate::sparse::{SENTINEL, SparseSet};
use crate::table::Table;
use crate::world::World;

/// Checks the S1–S7 sparse-set invariants.
#[cfg(any(test, debug_assertions))]
pub(crate) fn check_sparse_set<T>(set: &SparseSet<T>) {
    let len = set.len();
    let entities = set.entities();
    let sparse = set.sparse_slice();

    // S1: dense.len() == entities.len() == len.
    assert_eq!(entities.len(), len, "ecs: flush invariant: sparse S1");
    assert!(set.capacity() >= len, "ecs: flush invariant: sparse S7");

    // S2: sparse[entities[i].index] == i.
    for (i, entity) in entities.iter().enumerate() {
        assert!(
            (entity.index() as usize) < sparse.len(),
            "ecs: flush invariant: sparse S5 (entity index outside sparse)"
        );
        assert_eq!(
            sparse[entity.index() as usize],
            i as u32,
            "ecs: flush invariant: sparse S2"
        );
    }

    // S3: every non-sentinel sparse entry is a valid dense index that points
    // back to the same entity index.
    for (entity_index, &dense_index) in sparse.iter().enumerate() {
        if dense_index != SENTINEL {
            assert!(
                (dense_index as usize) < len,
                "ecs: flush invariant: sparse S3 (dense index out of range)"
            );
            assert_eq!(
                entities[dense_index as usize].index() as usize,
                entity_index,
                "ecs: flush invariant: sparse S3 (back pointer mismatch)"
            );
        }
    }

    // S4: entity indices are distinct.
    let mut indices: Vec<u32> = entities.iter().map(|entity| entity.index()).collect();
    indices.sort_unstable();
    for pair in indices.windows(2) {
        assert_ne!(pair[0], pair[1], "ecs: flush invariant: sparse S4");
    }

    // S5: sparse is long enough for every dense entity.
    if let Some(max) = indices.last() {
        assert!(
            sparse.len() > *max as usize,
            "ecs: flush invariant: sparse S5 (sparse too short)"
        );
    }

    // S6: no valid dense index is the sentinel.
    assert!(
        len < SENTINEL as usize,
        "ecs: flush invariant: sparse S6 (dense length reached sentinel)"
    );
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn check_table(table: &Table) {
    table.check();
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn check_locations(world: &World) {
    for index in 0..world.allocator.len() {
        let state = world.allocator.state(index as u32);
        let location = world
            .locations
            .get(index)
            .copied()
            .unwrap_or(EntityLocation::INVALID);
        match state {
            SlotState::Alive => {
                let entity = world.allocator.entity(index as u32);
                assert!(
                    location.archetype != u32::MAX,
                    "ecs: flush invariant: alive entity has no archetype"
                );
                assert_eq!(
                    location.generation,
                    entity.generation(),
                    "ecs: flush invariant: location generation mismatch"
                );
                let table = &world.tables[location.archetype as usize];
                assert!(
                    (location.row as usize) < table.len(),
                    "ecs: flush invariant: location row out of bounds"
                );
                assert_eq!(
                    table.entity_rows()[location.row as usize],
                    entity,
                    "ecs: flush invariant: location points at the wrong entity"
                );
            }
            SlotState::Free | SlotState::Reserved | SlotState::Retired => {
                if let Some(location) = world.locations.get(index) {
                    if location.archetype != u32::MAX {
                        let table = &world.tables[location.archetype as usize];
                        if (location.row as usize) < table.len() {
                            assert_ne!(
                                table.entity_rows()[location.row as usize],
                                world.allocator.entity(index as u32),
                                "ecs: flush invariant: dead entity is still in a table"
                            );
                        }
                    }
                }
            }
        }
    }

    for (archetype, table) in world.tables.iter().enumerate() {
        for &entity in table.entity_rows() {
            assert!(
                world.allocator.is_alive(entity),
                "ecs: flush invariant: table holds a dead entity"
            );
            let location = world.locations[entity.index() as usize];
            assert_eq!(location.archetype, archetype as u32);
        }
    }
}

#[cfg(any(test, debug_assertions))]
pub(crate) fn check_generation_bumps(world: &World) {
    world.allocator.check();
}

/// Full world validation.
#[cfg(any(test, debug_assertions))]
pub(crate) fn check_world(world: &World) {
    world.allocator.check();
    for table in &world.tables {
        check_table(table);
    }
    for set in world.sparse_sets.iter().flatten() {
        set.check();
    }
    check_locations(world);
    check_generation_bumps(world);

    for set in world.sparse_sets.iter().flatten() {
        for &entity in set.entities() {
            assert!(
                world.allocator.is_alive(entity),
                "ecs: flush invariant: sparse set holds a dead entity"
            );
        }
    }

    // SAFETY: `World: !Sync`; the checker runs while the world is exclusively
    // borrowed (end of flush/clear), so no guard can hold a borrow.
    let balanced = unsafe { &*world.borrows.get() }.is_balanced();
    assert!(
        balanced,
        "ecs: flush invariant: borrow map is not balanced at check time"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Without;
    use crate::component::Storage;
    use crate::entity::Entity;

    #[allow(dead_code)]
    #[derive(Debug)]
    struct A(u32);
    #[allow(dead_code)]
    #[derive(Debug)]
    struct B(u64);
    #[allow(dead_code)]
    #[derive(Debug)]
    struct S(u8);

    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    #[test]
    #[cfg_attr(miri, ignore = "property fuzzing is too slow under Miri")]
    fn random_operations_preserve_invariants() {
        let mut world = World::new();
        world.register::<A>(Storage::Table);
        world.register::<B>(Storage::Table);
        world.register::<S>(Storage::SparseSet);

        let mut alive: Vec<Entity> = Vec::new();
        let mut next_value = 0u32;
        let mut seed = 0xdead_beef_1234_5678u64;

        for step in 0..800u32 {
            let choice = lcg(&mut seed) % 9;
            match choice {
                0 => {
                    let e = world.spawn((A(next_value), B(u64::from(next_value))));
                    alive.push(e);
                }
                1 => {
                    let e = world.spawn((A(next_value),));
                    alive.push(e);
                }
                2 => {
                    let e = world.spawn((A(next_value), S(next_value as u8)));
                    alive.push(e);
                }
                3 => {
                    let e = world.spawn((B(u64::from(next_value)), S(next_value as u8)));
                    alive.push(e);
                }
                4 if !alive.is_empty() => {
                    let index = (lcg(&mut seed) as usize) % alive.len();
                    let e = alive[index];
                    let _ = world.insert(e, S((step & 0xff) as u8));
                }
                5 if !alive.is_empty() => {
                    let index = (lcg(&mut seed) as usize) % alive.len();
                    let e = alive[index];
                    let _ = world.insert(e, B(u64::from(step)));
                }
                6 if !alive.is_empty() => {
                    let index = (lcg(&mut seed) as usize) % alive.len();
                    let e = alive[index];
                    let _ = world.remove::<A>(e);
                }
                7 if !alive.is_empty() => {
                    let index = (lcg(&mut seed) as usize) % alive.len();
                    let e = alive.swap_remove(index);
                    assert!(world.despawn(e));
                }
                _ => {
                    // Exercise a few query plans; dropping the guards must
                    // leave the borrow map balanced.
                    let mut count = 0usize;
                    world.query::<(&A, &B)>().for_each(|_| count += 1);
                    world.query::<(&S, Without<A>)>().for_each(|_| count += 1);
                    world.query::<(Entity, Option<&A>)>().for_each(|_| {});
                    let _ = count;
                }
            }
            check_world(&world);
            next_value = next_value.wrapping_add(1);
        }

        world.clear();
        check_world(&world);
        // Every slot is free (or retired) and can be reused with a new
        // generation.
        for e in alive {
            assert!(!world.despawn(e));
        }
    }
}
