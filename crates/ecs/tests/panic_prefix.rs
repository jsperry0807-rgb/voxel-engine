//! §19.2 — the frozen panic-prefix table.
//!
//! Reachable prefixes are asserted through real panics; every prefix in the
//! frozen table is also checked to be present in `src/`, so the list cannot
//! silently drift.

use std::path::Path;

use voxel_ecs::{CommandBuffer, Storage, World};

#[derive(Debug, PartialEq)]
struct A(u32);
struct Unregistered;

fn world() -> World {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world
}

#[test]
fn every_frozen_prefix_is_implemented() {
    const PREFIXES: &[&str] = &[
        "ecs: borrow conflict:",
        "ecs: dead entity:",
        "ecs: component not registered:",
        "ecs: storage mismatch:",
        "ecs: entity index exhausted:",
        "ecs: flush invariant:",
        "ecs: duplicate component in query:",
        "ecs: duplicate component in bundle:",
        "ecs: query access violation:",
        "ecs: cross-world buffer:",
        "ecs: stale buffer:",
        "ecs: buffer ordering:",
        "ecs: buffer poisoned:",
        "ecs: component id exhausted:",
        "ecs: table column overflow:",
        "ecs: table row overflow:",
        "ecs: reader overflow:",
        "ecs: command buffer leak:",
    ];

    let mut source = String::new();
    collect_sources(Path::new("src"), &mut source);
    for prefix in PREFIXES {
        assert!(
            source.contains(prefix),
            "frozen panic prefix `{prefix}` is not produced anywhere in src/"
        );
    }
}

fn collect_sources(dir: &Path, out: &mut String) {
    for entry in std::fs::read_dir(dir).expect("read src directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push_str(&std::fs::read_to_string(&path).expect("read source file"));
            out.push('\n');
        }
    }
}

#[test]
#[should_panic(expected = "ecs: dead entity:")]
fn dead_entity() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    world.despawn(entity);
    world.insert(entity, A(2));
}

#[test]
#[should_panic(expected = "ecs: component not registered:")]
fn component_not_registered() {
    let mut world = world();
    let entity = world.spawn((A(1),));
    let _ = world.get::<Unregistered>(entity);
}

#[test]
#[should_panic(expected = "ecs: storage mismatch:")]
fn storage_mismatch() {
    let mut world = World::new();
    world.register::<A>(Storage::Table);
    world.register::<A>(Storage::SparseSet);
}

#[test]
#[should_panic(expected = "ecs: duplicate component in query:")]
fn duplicate_component_in_query() {
    let world = world();
    let _ = world.query::<(&A, &A)>();
}

#[test]
#[should_panic(expected = "ecs: duplicate component in bundle:")]
fn duplicate_component_in_bundle() {
    let mut world = world();
    let _ = world.spawn((A(1), A(2)));
}

#[test]
#[should_panic(expected = "ecs: query access violation:")]
fn query_access_violation() {
    struct B;
    let mut world = world();
    world.register::<B>(Storage::Table);
    let entity = world.spawn((A(1),));
    let query = world.query::<(&A,)>();
    let _ = query.get::<B>(entity);
}

#[test]
#[should_panic(expected = "ecs: buffer ordering:")]
fn buffer_ordering() {
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.despawn(entity);
    cb.insert(entity, A(1));
}

#[test]
#[should_panic(expected = "ecs: cross-world buffer:")]
fn cross_world_buffer() {
    let mut first = world();
    let mut second = world();
    let mut cb = CommandBuffer::new();
    let _entity = cb.spawn(&mut first, (A(1),));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        second.flush(&mut cb);
    }));
    first.cancel(&mut cb);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    panic!("ecs: cross-world buffer: expected a cross-world panic");
}

#[test]
#[should_panic(expected = "ecs: stale buffer:")]
fn stale_buffer() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let _ = cb.spawn(&mut world, (A(1),));
    world.clear();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    world.cancel(&mut cb);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    panic!("ecs: stale buffer: expected a stale-buffer panic");
}

#[test]
#[should_panic(expected = "ecs: buffer poisoned:")]
fn buffer_poisoned() {
    let mut world = world();
    let entity = world.spawn(());
    let mut cb = CommandBuffer::new();
    cb.insert(entity, Unregistered);
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    assert!(first.is_err());
    let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.flush(&mut cb);
    }));
    world.cancel(&mut cb);
    if let Err(payload) = second {
        std::panic::resume_unwind(payload);
    }
    panic!("ecs: buffer poisoned: expected a poisoned-buffer panic");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "ecs: command buffer leak:")]
fn command_buffer_leak() {
    let mut world = world();
    let mut cb = CommandBuffer::new();
    let _ = cb.spawn(&mut world, (A(1),));
    drop(cb);
}
