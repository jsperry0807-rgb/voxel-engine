//! §19.5 — representation and auto-trait limits that are observable publicly.

use my_ecs::{CommandBuffer, ComponentId, Entity, FlushOutcome, Storage, World};

fn assert_send<T: Send>() {}

#[test]
fn entity_representation_is_eight_bytes() {
    assert_eq!(core::mem::size_of::<Entity>(), 8);
    assert_eq!(core::mem::align_of::<Entity>(), 4);
}

#[test]
fn send_contracts() {
    assert_send::<Entity>();
    assert_send::<ComponentId>();
    assert_send::<Storage>();
    assert_send::<World>();
    assert_send::<CommandBuffer>();
    assert_send::<FlushOutcome>();
}

#[test]
fn storage_is_copy_and_equality_is_structural() {
    let storage = Storage::Table;
    let copy = storage;
    assert_eq!(storage, copy);
    assert_ne!(Storage::Table, Storage::SparseSet);
}

#[test]
fn component_ids_are_unique_per_type() {
    let mut world = World::new();
    let a = world.register::<u32>(Storage::Table);
    let b = world.register::<u64>(Storage::Table);
    let c = world.register::<String>(Storage::SparseSet);
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
}

#[test]
fn empty_world_queries_are_valid() {
    let mut world = World::new();
    assert_eq!(world.query::<(Entity,)>().iter().count(), 0);
    assert_eq!(world.query::<()>().iter().count(), 0);
    let mut cb = CommandBuffer::new();
    let entity = cb.spawn(&mut world, ());
    world.cancel(&mut cb);
    assert!(!world.despawn(entity));
}

#[test]
fn unchecked_access_under_a_valid_contract() {
    let mut world = World::new();
    world.register::<u32>(Storage::Table);
    let entity = world.spawn((7u32,));
    // SAFETY: the entity is alive, has a `u32`, and there is no other
    // reference to it.
    unsafe {
        assert_eq!(*world.get_unchecked::<u32>(entity), 7);
        *world.get_unchecked_mut::<u32>(entity) = 8;
    }
    assert_eq!(world.get::<u32>(entity), Some(&8));
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn unchecked_access_to_unregistered_type_panics_safely() {
    struct Unregistered;
    let mut world = World::new();
    world.register::<u32>(Storage::Table);
    let entity = world.spawn((7u32,));
    // SAFETY: this call is expected to panic on registration before any UB
    // contract is consulted.
    unsafe {
        let _ = world.get_unchecked::<Unregistered>(entity);
    }
}
