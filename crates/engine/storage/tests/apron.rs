//! M2-01: the six face planes equal a direct read of the padded volume.

use std::path::PathBuf;

use voxel_block::MaterialId;
use voxel_storage::gpu::MemSink;
use voxel_storage::world::World;
use voxel_storage::{AIR, Apron, CHUNK_DIM, ChunkPos, Face, local_index};

type FillFn = Box<dyn FnMut(ChunkPos, &mut [u16]) + Send>;

fn id(raw: u16) -> MaterialId {
    MaterialId::from_raw(raw)
}

fn tmp(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "vstore-apron-{name}-{}-{stamp}",
        std::process::id()
    ))
}

/// Integer hash with no platform-dependent state.
fn hash(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ (z as u32).wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^ (h >> 16)
}

/// A solid world whose material changes with height, so neighbours differ.
fn layered() -> FillFn {
    Box::new(|p, dense| {
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let world_y = p.y * CHUNK_DIM as i32 + y as i32;
                    let material = 1 + (world_y.div_euclid(3).rem_euclid(5) as u16);
                    dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
                }
            }
        }
    })
}

/// A sparse, three-dimensional pattern with air and several materials.
fn noisy() -> FillFn {
    Box::new(|p, dense| {
        for z in 0..CHUNK_DIM {
            for y in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    let world_x = p.x * CHUNK_DIM as i32 + x as i32;
                    let world_y = p.y * CHUNK_DIM as i32 + y as i32;
                    let world_z = p.z * CHUNK_DIM as i32 + z as i32;
                    let h = hash(world_x, world_y, world_z);
                    dense[local_index(x as u32, y as u32, z as u32) as usize] =
                        if h % 7 < 2 { AIR } else { (h % 11) as u16 + 1 };
                }
            }
        }
    })
}

/// Loads `p` and its six face neighbours.
fn load_around(world: &mut World<MemSink>, p: ChunkPos) {
    for (dx, dy, dz) in [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ] {
        world
            .load(ChunkPos::new(p.x + dx, p.y + dy, p.z + dz))
            .unwrap();
    }
    world.load(p).unwrap();
}

fn assert_matches_padded_read(world: &World<MemSink>, p: ChunkPos) {
    let apron = world.apron(p);
    let origin = (
        p.x * CHUNK_DIM as i32,
        p.y * CHUNK_DIM as i32,
        p.z * CHUNK_DIM as i32,
    );
    for face in Face::ALL {
        for v in 0..Apron::SIDE {
            for u in 0..Apron::SIDE {
                let (dx, dy, dz) = face.offset(u, v);
                let expected = world
                    .get(origin.0 + dx, origin.1 + dy, origin.2 + dz)
                    .map(MaterialId::as_raw)
                    .unwrap_or(AIR);
                assert_eq!(
                    apron.get(face, u, v),
                    expected,
                    "{face:?} texel ({u}, {v}) around {p:?}"
                );
            }
        }
    }
}

#[test]
fn apron_matches_dense_read_layered() {
    let dir = tmp("layered");
    let mut world = World::new(&dir, layered(), MemSink::default()).unwrap();
    for p in [ChunkPos::new(0, 0, 0), ChunkPos::new(-1, 2, 1)] {
        load_around(&mut world, p);
        assert_matches_padded_read(&world, p);
    }
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_matches_dense_read_noise() {
    let dir = tmp("noise");
    let mut world = World::new(&dir, noisy(), MemSink::default()).unwrap();
    for p in [ChunkPos::new(2, -1, 3), ChunkPos::new(-2, 0, -3)] {
        load_around(&mut world, p);
        assert_matches_padded_read(&world, p);
    }
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_matches_dense_read_after_edits() {
    let dir = tmp("edits");
    let mut world = World::new(&dir, noisy(), MemSink::default()).unwrap();
    let p = ChunkPos::new(0, 0, 0);
    load_around(&mut world, p);
    let edge = CHUNK_DIM as i32;

    // Overlay writes in the target and in every face neighbour, including a
    // carve back to air so the overlay must shadow a solid DAG value.
    world.set(0, 0, 0, id(15)).unwrap();
    world.set(-1, 0, 0, id(16)).unwrap();
    world.set(edge, 0, 0, id(0)).unwrap();
    world.set(0, -1, 0, id(17)).unwrap();
    world.set(0, edge, 0, id(18)).unwrap();
    world.set(0, 0, -1, id(19)).unwrap();
    world.set(0, 0, edge, id(20)).unwrap();

    assert_matches_padded_read(&world, p);
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_face_axes_match_the_documented_mapping() {
    let dir = tmp("axes");
    let mut world = World::new(&dir, noisy(), MemSink::default()).unwrap();
    let p = ChunkPos::new(0, 0, 0);
    load_around(&mut world, p);
    let apron = world.apron(p);
    let edge = CHUNK_DIM as i32;
    let read = |x: i32, y: i32, z: i32| world.get(x, y, z).map(MaterialId::as_raw).unwrap_or(AIR);

    for v in 0..Apron::SIDE {
        for u in 0..Apron::SIDE {
            let (u, v) = (u as i32, v as i32);
            assert_eq!(apron.get(Face::NegX, u as u32, v as u32), read(-1, v, u));
            assert_eq!(apron.get(Face::PosX, u as u32, v as u32), read(edge, v, u));
            assert_eq!(apron.get(Face::NegY, u as u32, v as u32), read(v, -1, u));
            assert_eq!(apron.get(Face::PosY, u as u32, v as u32), read(v, edge, u));
            assert_eq!(apron.get(Face::NegZ, u as u32, v as u32), read(u, v, -1));
            assert_eq!(apron.get(Face::PosZ, u as u32, v as u32), read(u, v, edge));
            assert_eq!(
                apron.material(Face::PosZ, u as u32, v as u32),
                MaterialId::from_raw(read(u, v, edge))
            );
        }
    }
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_reads_air_when_neighbours_are_missing() {
    let dir = tmp("missing");
    let mut world = World::new(&dir, noisy(), MemSink::default()).unwrap();
    world.load(ChunkPos::new(0, 0, 0)).unwrap();
    let apron = world.apron(ChunkPos::new(0, 0, 0));
    for face in Face::ALL {
        for v in 0..Apron::SIDE {
            for u in 0..Apron::SIDE {
                assert_eq!(apron.get(face, u, v), AIR, "{face:?} ({u}, {v})");
            }
        }
    }
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_into_reuses_one_buffer_and_resets_it() {
    let dir = tmp("reuse");
    let mut world = World::new(&dir, noisy(), MemSink::default()).unwrap();
    let p = ChunkPos::new(0, 0, 0);
    load_around(&mut world, p);

    let mut buffer = Apron::new();
    world.apron_into(p, &mut buffer);
    let fresh = world.apron(p);
    for face in Face::ALL {
        assert_eq!(buffer.face(face), fresh.face(face), "{face:?}");
    }

    // A chunk with no resident neighbours must overwrite every texel with AIR
    // rather than leave the previous chunk's border behind.
    let empty = ChunkPos::new(40, 0, 0);
    world.apron_into(empty, &mut buffer);
    for face in Face::ALL {
        assert!(
            buffer.face(face).iter().all(|&material| material == AIR),
            "{face:?} kept a stale texel"
        );
    }
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn apron_reads_neighbours_when_the_target_chunk_is_absent() {
    let dir = tmp("absent-target");
    let mut world = World::new(&dir, layered(), MemSink::default()).unwrap();
    let p = ChunkPos::new(3, 0, -2);
    for (dx, dy, dz) in [
        (-1, 0, 0),
        (1, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
    ] {
        world
            .load(ChunkPos::new(p.x + dx, p.y + dy, p.z + dz))
            .unwrap();
    }
    assert!(
        world.chunk(p).is_none(),
        "the target itself is not resident"
    );

    // The apron only reads neighbours, so it still matches a direct read.
    assert_matches_padded_read(&world, p);
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}
