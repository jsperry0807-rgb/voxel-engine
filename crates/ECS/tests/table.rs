//! §10 — columnar tables: lockstep, swap-remove, alignment, ZSTs.

use hashbrown::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use my_ecs::{Entity, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);

#[repr(align(64))]
#[derive(Debug, PartialEq)]
struct OverAligned(u8);

#[repr(align(64))]
struct OverAlignedZst;

static ZST_DROPS: AtomicUsize = AtomicUsize::new(0);

#[repr(align(64))]
struct OverAlignedZstDrop;

impl Drop for OverAlignedZstDrop {
    fn drop(&mut self) {
        ZST_DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Zst;

#[test]
fn swap_remove_preserves_entity_component_mapping() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);

    let mut model: HashMap<Entity, u32> = HashMap::new();
    for index in 0..200 {
        let entity = world.spawn((A(index),));
        model.insert(entity, index);
    }

    // Despawn every third entity to force swap-removes from the middle.
    let removed: Vec<Entity> = model.keys().copied().step_by(3).collect();
    for entity in &removed {
        assert!(world.despawn(*entity));
        model.remove(entity);
    }

    for (entity, value) in &model {
        assert_eq!(world.get::<A>(*entity), Some(&A(*value)));
    }

    let mut seen = 0;
    world.query::<(Entity, &A)>().for_each(|(entity, value)| {
        assert_eq!(Some(&value.0), model.get(&entity));
        seen += 1;
    });
    assert_eq!(seen, model.len());
}

#[test]
fn over_aligned_columns_are_aligned() {
    let mut world = World::new();
    world.register::<OverAligned>(Storage::Table);
    for index in 0..32u8 {
        world.spawn((OverAligned(index),));
    }
    let mut seen = 0;
    world.query::<(&OverAligned,)>().for_each(|(value,)| {
        let address = value as *const OverAligned as usize;
        assert_eq!(address % 64, 0, "over-aligned component is misaligned");
        seen += 1;
    });
    assert_eq!(seen, 32);
}

#[test]
fn over_aligned_zst_columns_are_aligned_and_dropped() {
    ZST_DROPS.store(0, Ordering::SeqCst);
    let mut world = World::new();
    world.register::<OverAlignedZst>(Storage::Table);
    world.register::<OverAlignedZstDrop>(Storage::Table);

    let a = world.spawn((OverAlignedZst, OverAlignedZstDrop));
    let _b = world.spawn((OverAlignedZst, OverAlignedZstDrop));

    let mut seen = 0;
    world
        .query::<(&OverAlignedZst, &OverAlignedZstDrop)>()
        .for_each(|(marker, bomb)| {
            assert_eq!(marker as *const OverAlignedZst as usize % 64, 0);
            assert_eq!(bomb as *const OverAlignedZstDrop as usize % 64, 0);
            seen += 1;
        });
    assert_eq!(seen, 2);

    assert!(world.despawn(a));
    assert_eq!(ZST_DROPS.load(Ordering::SeqCst), 1);
    world.clear();
    assert_eq!(ZST_DROPS.load(Ordering::SeqCst), 2);
}

#[test]
fn zst_columns_have_presence_without_bytes() {
    let mut world = World::new();
    world.register::<Zst>(Storage::Table);
    let a = world.spawn((Zst,));
    let b = world.spawn((Zst,));
    let mut count = 0;
    world.query::<(Entity, &Zst)>().for_each(|_| count += 1);
    assert_eq!(count, 2);
    assert!(world.despawn(a));
    assert!(world.despawn(b));
    assert_eq!(world.query::<(&Zst,)>().iter().count(), 0);
}

#[test]
fn tables_grow_beyond_initial_capacity() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    for index in 0..5_000 {
        world.spawn((A(index),));
    }
    let mut count = 0;
    world.query::<(&A,)>().for_each(|_| count += 1);
    assert_eq!(count, 5_000);
}

#[test]
fn sparse_set_handles_large_entity_indices() {
    let mut world = World::new();
    world.register::<A>(Storage::SparseSet);
    let mut entities = Vec::new();
    for _ in 0..100 {
        entities.push(world.spawn(()));
    }
    let last = *entities.last().unwrap();
    assert!(world.insert(last, A(7)).is_none());
    assert_eq!(world.get::<A>(last), Some(&A(7)));
    // Despawn everything except the last entity, then ensure the sparse value
    // still resolves through the (reused) allocator slot table.
    for entity in &entities[..entities.len() - 1] {
        assert!(world.despawn(*entity));
    }
    assert_eq!(world.get::<A>(last), Some(&A(7)));
}
