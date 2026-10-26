//! 128³ smoke test (M1-26).
//!
//! This target is the compile-and-run proof that the pre-registered 128³ edge
//! still builds the chunk layout and serializes it, without turning the
//! parameterized path into a benchmark. It runs only under `--features
//! chunk128`; the default 64³ build compiles no test body here.
//!
//! The smoke is deliberately one pass of each operation: one dense build, one
//! `get` sweep against the source buffer, and one serialize/deserialize
//! round-trip. Exhaustive coverage belongs to the 64³ fixtures and the
//! property tests; this test only answers "does the 7-bit edge still work".

#![cfg(feature = "chunk128")]

use voxel_storage::serialize::CHUNK_BITS_OFFSET;
use voxel_storage::{CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME, ChunkDag, local_index};

// Compile-only half of the smoke: the target cannot be built unless the feature
// selected the 128³ edge and the derived constants follow it.
const _: () = assert!(CHUNK_BITS == 7);
const _: () = assert!(CHUNK_DIM == 128);
const _: () = assert!(CHUNK_VOLUME == 2_097_152);

/// A deterministic terrain-like chunk: a varying height field with a surface
/// layer, a cut cave pocket and air above it. It exercises present and absent
/// octants at every level of the seven-level tree without being random.
fn fixture() -> Vec<u16> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let height = CHUNK_DIM / 2 + (x * 7 + z * 13) % 17;
            for y in 0..height {
                let material = if y + 1 == height {
                    3 // surface
                } else if y < 8 {
                    1 // deep rock
                } else {
                    2 // fill
                };
                dense[local_index(x as u32, y as u32, z as u32) as usize] = material;
            }
            if x % 31 == 0 && z % 29 == 0 {
                // A pocket at mid-height, so a present branch contains air.
                dense[local_index(x as u32, (height / 2) as u32, z as u32) as usize] = 0;
            }
        }
    }
    dense
}

#[test]
fn smoke128_compile_only() {
    // The module's const assertions run in the compiler; this keeps the
    // compile-only half visible to `cargo test ... smoke128` as well.
    assert_eq!(CHUNK_BITS, 7);
    assert_eq!(CHUNK_DIM, 1 << CHUNK_BITS);
}

#[test]
fn smoke128_build_get_serialize() {
    let dense = fixture();

    // One build.
    let dag = ChunkDag::from_dense(&dense);
    assert!(!dag.is_empty());

    // One `get` pass over known points: a deep voxel, the surface, an air
    // voxel above the terrain and the far corner.
    let last = CHUNK_DIM as u32 - 1;
    for (x, y, z) in [(0, 0, 0), (64, 63, 64), (0, last, 0), (last, last, last)] {
        let index = local_index(x, y, z) as usize;
        assert_eq!(
            dag.get(x, y, z),
            dense[index],
            "get({x}, {y}, {z}) disagrees with the dense source"
        );
    }

    // One serialize round-trip. The v2 header must carry the 7-bit edge.
    let bytes = dag.to_bytes();
    assert_eq!(bytes[CHUNK_BITS_OFFSET] as u32, CHUNK_BITS);
    let restored = ChunkDag::from_bytes(&bytes).expect("the 128³ chunk should round-trip");
    assert_eq!(restored, dag);
}
