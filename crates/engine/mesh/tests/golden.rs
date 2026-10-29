//! M2-11: five golden mesh fixtures with stable content hashes.
//!
//! Each fixture is meshed through the real block driver, greedy core and 16 B
//! emitter. The committed golden file stores an overall mesh hash and one hash
//! per quad, so a mismatch identifies the first differing quad and its block.
//!
//! Regenerate after an intentional mesh change:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test -p voxel-mesh golden
//! cargo test -p voxel-mesh --features chunk128 golden
//! ```

use std::path::{Path, PathBuf};

use voxel_core::Face;
use voxel_mesh::{
    BLOCK_EDGE, BlockPos, GreedyQuad, Mesh, for_each_block, greedy_block, mesh_block,
};
use voxel_storage::{Apron, CHUNK_DIM, CHUNK_VOLUME, local_index};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[test]
fn golden() {
    for (name, voxels) in fixtures() {
        let built = build(&voxels);
        let path = golden_path(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            write_golden(&path, &built);
            continue;
        }
        let expected = read_golden(&path);
        if expected.mesh != built.mesh || expected.quads != built.quads {
            let index = first_difference(&expected.quads, &built.quads);
            let (block, quad) = built
                .records
                .get(index)
                .map(|record| (record.block, record.quad))
                .unwrap_or((BlockPos::new(0, 0, 0), placeholder()));
            panic!(
                "golden mismatch in {name} at quad {index}: block {block:?}, quad {quad:?}; \
                 expected {} quads, got {}; regenerate with UPDATE_GOLDEN=1 if intentional",
                expected.quads.len(),
                built.quads.len()
            );
        }
    }
}

struct Built {
    mesh: u64,
    quads: Vec<u64>,
    records: Vec<QuadRecord>,
}

struct QuadRecord {
    block: BlockPos,
    quad: GreedyQuad,
}

struct Golden {
    mesh: u64,
    quads: Vec<u64>,
}

fn build(voxels: &[u16]) -> Built {
    let apron = Apron::new();
    let mut mesh_hash = FNV_OFFSET;
    let mut quad_hashes = Vec::new();
    let mut records = Vec::new();

    for_each_block(voxels, &apron, |block, scratch| {
        let mut quads = Vec::new();
        greedy_block(scratch, &mut quads);
        let mesh = mesh_block(&quads, block);
        fnv_bytes(
            &mut mesh_hash,
            &[block.x as u8, block.y as u8, block.z as u8],
        );
        hash_mesh(&mut mesh_hash, &mesh);

        for (quad_index, quad) in quads.iter().enumerate() {
            quad_hashes.push(hash_quad(&mesh, quad_index));
            records.push(QuadRecord { block, quad: *quad });
        }
    });

    Built {
        mesh: mesh_hash,
        quads: quad_hashes,
        records,
    }
}

fn hash_mesh(hash: &mut u64, mesh: &Mesh) {
    for vertex in &mesh.vertices {
        fnv_bytes(hash, &vertex.encode());
    }
    for index in &mesh.indices {
        fnv_bytes(hash, &index.to_le_bytes());
    }
}

fn hash_quad(mesh: &Mesh, quad: usize) -> u64 {
    let mut hash = FNV_OFFSET;
    for vertex in &mesh.vertices[quad * 4..quad * 4 + 4] {
        fnv_bytes(&mut hash, &vertex.encode());
    }
    for index in &mesh.indices[quad * 6..quad * 6 + 6] {
        fnv_bytes(&mut hash, &index.to_le_bytes());
    }
    hash
}

fn fnv_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn first_difference(expected: &[u64], actual: &[u64]) -> usize {
    expected
        .iter()
        .zip(actual)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| expected.len().min(actual.len()))
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}-{CHUNK_DIM}.txt"))
}

fn write_golden(path: &Path, built: &Built) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create golden directory");
    }
    let mut text = format!("mesh {:016x}\n", built.mesh);
    for hash in &built.quads {
        text.push_str(&format!("quad {hash:016x}\n"));
    }
    std::fs::write(path, text).expect("write golden file");
}

fn read_golden(path: &Path) -> Golden {
    let text = std::fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "cannot read {}: {error}; run UPDATE_GOLDEN=1 cargo test -p voxel-mesh golden",
            path.display()
        )
    });
    let mut mesh = None;
    let mut quads = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(kind) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next() else {
            continue;
        };
        let value = u64::from_str_radix(value, 16).expect("hex golden hash");
        match kind {
            "mesh" => mesh = Some(value),
            "quad" => quads.push(value),
            other => panic!("unknown golden line `{other}` in {}", path.display()),
        }
    }
    Golden {
        mesh: mesh.expect("golden mesh hash"),
        quads,
    }
}

fn placeholder() -> GreedyQuad {
    GreedyQuad {
        face: Face::Top,
        material: 0,
        ao: [0; 4],
        origin: [0; 3],
        extent: [0; 3],
    }
}

fn fixtures() -> Vec<(&'static str, Vec<u16>)> {
    let mut flat = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            for y in 0..16 {
                set(&mut flat, x, y, z, 1);
            }
        }
    }

    let mut cave = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            for y in 0..48 {
                set(&mut cave, x, y, z, 1);
            }
        }
    }
    for z in 0..CHUNK_DIM {
        for x in 24..40 {
            for y in 24..32 {
                set(&mut cave, x, y, z, 0);
            }
        }
    }

    let mut overhang = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            for y in 0..20 {
                set(&mut overhang, x, y, z, 1);
            }
        }
    }
    for z in 16..48 {
        for x in 16..48 {
            for y in 24..28 {
                set(&mut overhang, x, y, z, 2);
            }
        }
    }

    let mut mixed = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            for y in 0..24 {
                set(&mut mixed, x, y, z, 1 + ((x / 8 + z / 8) % 3) as u16);
            }
        }
    }

    let mut border = vec![0u16; CHUNK_VOLUME];
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            for y in 0..20 {
                set(&mut border, x, y, z, 1);
            }
        }
    }
    for z in 0..4 {
        for x in 0..4 {
            for y in 20..24 {
                set(&mut border, x, y, z, 2);
            }
        }
    }
    for z in CHUNK_DIM - 4..CHUNK_DIM {
        for x in CHUNK_DIM - 4..CHUNK_DIM {
            for y in 20..24 {
                set(&mut border, x, y, z, 0);
            }
        }
    }

    vec![
        ("flat", flat),
        ("cave", cave),
        ("overhang", overhang),
        ("mixed_material", mixed),
        ("chunk_border", border),
    ]
}

fn set(voxels: &mut [u16], x: usize, y: usize, z: usize, material: u16) {
    voxels[local_index(x as u32, y as u32, z as u32) as usize] = material;
}

const _: () = assert!(CHUNK_DIM.is_multiple_of(BLOCK_EDGE));
