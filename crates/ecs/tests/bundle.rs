//! §8 — tuple bundles up to arity 16.

use voxel_ecs::{Bundle, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
#[derive(Debug, PartialEq)]
struct B(u32);

macro_rules! define_components {
    ($($name:ident),+) => {
        $(
            #[derive(Debug, PartialEq)]
            struct $name(u8);
        )+
    };
}

define_components!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9, C10, C11, C12, C13, C14, C15);

#[test]
fn empty_bundle_spawns_an_entity() {
    let mut world = World::new();
    let entity = world.spawn(());
    assert!(world.query::<(voxel_ecs::Entity,)>().contains(entity));
    assert!(world.despawn(entity));
}

#[test]
fn sixteen_tuple_bundle_round_trips() {
    let mut world = World::new();
    world.register::<C0>(Storage::Table);
    world.register::<C1>(Storage::Table);
    world.register::<C2>(Storage::Table);
    world.register::<C3>(Storage::Table);
    world.register::<C4>(Storage::Table);
    world.register::<C5>(Storage::Table);
    world.register::<C6>(Storage::Table);
    world.register::<C7>(Storage::Table);
    world.register::<C8>(Storage::Table);
    world.register::<C9>(Storage::Table);
    world.register::<C10>(Storage::Table);
    world.register::<C11>(Storage::Table);
    world.register::<C12>(Storage::Table);
    world.register::<C13>(Storage::Table);
    world.register::<C14>(Storage::Table);
    world.register::<C15>(Storage::Table);

    let entity = world.spawn((
        C0(0),
        C1(1),
        C2(2),
        C3(3),
        C4(4),
        C5(5),
        C6(6),
        C7(7),
        C8(8),
        C9(9),
        C10(10),
        C11(11),
        C12(12),
        C13(13),
        C14(14),
        C15(15),
    ));

    let mut seen = 0;
    world
        .query::<(
            &C0,
            &C1,
            &C2,
            &C3,
            &C4,
            &C5,
            &C6,
            &C7,
            &C8,
            &C9,
            &C10,
            &C11,
            &C12,
            &C13,
            &C14,
            &C15,
        )>()
        .for_each(|values| {
            assert_eq!(values.0 .0, 0);
            assert_eq!(values.15 .0, 15);
            seen += 1;
        });
    assert_eq!(seen, 1);
    assert!(world.despawn(entity));
}

#[test]
fn mixed_storage_bundle() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<B>(Storage::SparseSet);

    let entity = world.spawn((A(1), B(2)));
    assert_eq!(world.get::<A>(entity), Some(&A(1)));
    assert_eq!(world.get::<B>(entity), Some(&B(2)));
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn unregistered_bundle_panics() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    let _ = world.spawn((A(1), B(2)));
}

#[test]
#[should_panic(expected = "ecs: duplicate component in bundle:")]
fn duplicate_bundle_panics() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    let _ = world.spawn((A(1), A(2)));
}

#[test]
fn bundle_is_publicly_bound_but_sealed() {
    fn accepts_bundle<B: Bundle>(_b: B) {}
    accepts_bundle((A(1),));
    accepts_bundle(());
}
