//! Dense chunk generators shared by the benchmarks.
//!
//! Every generator returns a `64³` dense material array (`u16` per voxel), the
//! input shape `Svdag::from_dense` consumes. Keeping them here means the memory,
//! CPU, Criterion and Callgrind lenses all measure the same data.

use voxel_storage::CHUNK_VOLUME;

/// A chunk filled with one material.
pub fn solid(material: u16) -> Vec<u16> {
    vec![material; CHUNK_VOLUME]
}

/// Deterministic xorshift.
fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A Minecraft-like column: stone below dirt below grass, with sparse caves.
///
/// `ox`/`oz` select the chunk column, so neighbouring chunks share large-scale
/// structure and deduplicate differently.
pub fn terrain(ox: i32, oz: i32) -> Vec<u16> {
    const STONE: u16 = 1;
    const DIRT: u16 = 2;
    const GRASS: u16 = 3;

    let mut d = vec![0u16; CHUNK_VOLUME];
    let mut seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((ox as u64) << 20) ^ (oz as u64);
    for z in 0..64usize {
        for x in 0..64usize {
            let fx = (x as i32 + ox * 64) as f32;
            let fz = (z as i32 + oz * 64) as f32;
            let height = 28
                + ((fx * 0.15).sin() * 6.0
                    + (fz * 0.11).cos() * 5.0
                    + ((fx + fz) * 0.07).sin() * 4.0) as i32;
            for y in 0..64usize {
                let yi = y as i32;
                let material = if yi < height - 4 {
                    STONE
                } else if yi < height {
                    DIRT
                } else if yi == height {
                    GRASS
                } else {
                    0
                };
                let cave = material == STONE && rng(&mut seed) % 100 < 3;
                d[x + y * 64 + z * 4096] = if cave { 0 } else { material };
            }
        }
    }
    d
}

/// Random sparse voxels over air. `permille` is the fill rate.
pub fn noise(seed: u64, permille: u64) -> Vec<u16> {
    let mut state = seed | 1;
    (0..CHUNK_VOLUME)
        .map(|_| {
            if rng(&mut state) % 1000 < permille {
                (rng(&mut state) % 8 + 1) as u16
            } else {
                0
            }
        })
        .collect()
}

/// Exactly `voxels` non-air voxels at random positions.
pub fn sparse(seed: u64, voxels: usize) -> Vec<u16> {
    let mut state = seed | 1;
    let mut d = vec![0u16; CHUNK_VOLUME];
    let mut placed = 0;
    while placed < voxels {
        let index = rng(&mut state) as usize % CHUNK_VOLUME;
        if d[index] == 0 {
            d[index] = (rng(&mut state) % 8 + 1) as u16;
            placed += 1;
        }
    }
    d
}
