//! M2-12: mesh determinism across worker counts and processing order.
//!
//! Four chunks are generated from one world-space height field. Each worker
//! meshes a disjoint slice, results are reassembled by chunk index, and the
//! aggregate hash must match for 1, 2, 4 and 8 workers over ten repeats. A
//! reversed single-thread order must also produce the same hash.

use std::thread;

use voxel_mesh::{for_each_block, greedy_block, mesh_block};
use voxel_storage::{Apron, CHUNK_DIM, CHUNK_VOLUME, local_index};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[test]
fn determinism() {
    let volumes = {
        let edge = if CHUNK_DIM == 64 { 2 } else { 1 };
        let mut volumes = Vec::new();
        for cz in 0..edge {
            for cx in 0..edge {
                volumes.push(chunk_volume(cx, cz));
            }
        }
        volumes
    };
    let order: Vec<usize> = (0..volumes.len()).collect();
    let baseline = hash_with_order(&volumes, &order, 1);

    for workers in [1, 2, 4, 8] {
        for repeat in 0..10 {
            let hash = hash_with_order(&volumes, &order, workers);
            assert_eq!(
                hash, baseline,
                "worker count {workers}, repeat {repeat} changed the mesh hash"
            );
        }
    }

    let reverse: Vec<usize> = (0..volumes.len()).rev().collect();
    assert_eq!(hash_with_order(&volumes, &reverse, 1), baseline);
    assert_eq!(hash_with_order(&volumes, &reverse, 4), baseline);
}

fn hash_with_order(volumes: &[Vec<u16>], order: &[usize], workers: usize) -> u64 {
    let chunk_size = order.len().div_ceil(workers).max(1);
    let mut per_chunk = vec![0u64; volumes.len()];

    thread::scope(|scope| {
        let handles: Vec<_> = order
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|&index| (index, mesh_chunk(&volumes[index])))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            for (index, hash) in handle.join().expect("worker panicked") {
                per_chunk[index] = hash;
            }
        }
    });

    let mut hash = FNV_OFFSET;
    for chunk_hash in per_chunk {
        fnv_bytes(&mut hash, &chunk_hash.to_le_bytes());
    }
    hash
}

fn mesh_chunk(voxels: &[u16]) -> u64 {
    let apron = Apron::new();
    let mut hash = FNV_OFFSET;
    for_each_block(voxels, &apron, |block, scratch| {
        let mut quads = Vec::new();
        greedy_block(scratch, &mut quads);
        let mesh = mesh_block(&quads, block);
        fnv_bytes(&mut hash, &block.x.to_le_bytes());
        fnv_bytes(&mut hash, &block.y.to_le_bytes());
        fnv_bytes(&mut hash, &block.z.to_le_bytes());
        for vertex in &mesh.vertices {
            fnv_bytes(&mut hash, &vertex.encode());
        }
        for index in &mesh.indices {
            fnv_bytes(&mut hash, &index.to_le_bytes());
        }
    });
    hash
}

fn fnv_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn chunk_volume(cx: i32, cz: i32) -> Vec<u16> {
    let edge = CHUNK_DIM as i32;
    let mut voxels = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let wx = cx * edge + x as i32;
            let wz = cz * edge + z as i32;
            let height = height(wx, wz);
            for y in 0..CHUNK_DIM {
                let wy = y as i32;
                if wy >= height {
                    continue;
                }
                let material = if wy + 1 == height {
                    3
                } else if wy + 4 >= height {
                    2
                } else {
                    1
                };
                voxels[local_index(x as u32, y as u32, z as u32) as usize] = material;
            }
        }
    }
    voxels
}

fn height(x: i32, z: i32) -> i32 {
    24 + (hash2(x, z) % 12) as i32
}

fn hash2(x: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (z as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h.wrapping_mul(0x846C_A68B) ^ (h >> 16)
}
