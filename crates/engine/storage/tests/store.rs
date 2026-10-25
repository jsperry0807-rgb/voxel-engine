use std::path::PathBuf;
use voxel_block::{BlockRegistryBuilder, MaterialId};
use voxel_storage::dag::Svdag;
use voxel_storage::gpu::{GpuStore, MemSink};
use voxel_storage::world::World;
use voxel_storage::{ChunkPos, CHUNK_VOLUME};

fn id(raw: u16) -> MaterialId {
    MaterialId::from_raw(raw)
}

fn tmp(name: &str) -> PathBuf {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("vstore-{name}-{}-{t}", std::process::id()))
}

fn flat() -> Box<dyn FnMut(ChunkPos, &mut [u16]) + Send> {
    Box::new(|p, d| {
        if p.y == 0 {
            for z in 0..64 {
                for y in 0..32 {
                    for x in 0..64 {
                        d[x + y * 64 + z * 4096] = 1;
                    }
                }
            }
        }
    })
}

#[test]
fn dag_roundtrip() {
    let mut d = vec![0u16; CHUNK_VOLUME];
    let mut s = 12345u32;
    for i in 0..CHUNK_VOLUME {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        if (s >> 24) < 40 {
            d[i] = ((s >> 8) & 7) as u16;
        }
    }
    let dag = Svdag::from_dense(&d);
    let mut out = vec![0u16; CHUNK_VOLUME];
    dag.expand(&mut out);
    assert_eq!(d, out);
    for i in (0..CHUNK_VOLUME).step_by(97) {
        assert_eq!(
            dag.get((i & 63) as u32, ((i >> 6) & 63) as u32, (i >> 12) as u32),
            d[i]
        );
    }
    assert_eq!(Svdag::from_bytes(&dag.to_bytes()).unwrap(), dag);
    // solid chunk collapses to 6 nodes
    let solid = Svdag::from_dense(&vec![3u16; CHUNK_VOLUME]);
    assert!(solid.pool.len() <= 6 * 9);
}

#[test]
fn overlay_then_batched_consolidation() {
    let dir = tmp("w");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.load(ChunkPos::new(0, 0, 0)).unwrap();
    w.load(ChunkPos::new(-1, 0, -1)).unwrap();
    w.flush_gpu();
    let c0 = ChunkPos::new(0, 0, 0);
    assert_eq!(w.gpu().lookup(c0, 5, 10, 5), 1);

    w.set(5, 10, 5, id(0)).unwrap(); // mine
    w.set(5, 40, 5, id(2)).unwrap(); // place
    w.set(-1, 0, -1, id(3)).unwrap(); // negative coords
    assert_eq!(w.get(5, 10, 5), Some(id(0)));
    assert_eq!(w.get(5, 40, 5), Some(id(2)));
    assert_eq!(w.get(-1, 0, -1), Some(id(3)));
    // GPU still shows the old DAG until the batch
    assert_eq!(w.gpu().lookup(c0, 5, 10, 5), 1);
    assert_eq!(w.dirty_chunks(), 2);

    let r = w.consolidate(16).unwrap();
    assert_eq!(r.chunks, 2);
    assert_eq!(w.dirty_chunks(), 0);
    assert_eq!(w.gpu().lookup(c0, 5, 10, 5), 0);
    assert_eq!(w.gpu().lookup(c0, 5, 40, 5), 2);
    assert_eq!(w.gpu().lookup(ChunkPos::new(-1, 0, -1), 63, 0, 63), 3);
    // sink mirrors CPU copy
    let g = w.gpu();
    assert_eq!(&w.sink().nodes[..g.pool().len()], g.pool());
    assert_eq!(w.sink().table, g.table());

    w.reclaim(r.epoch);
    // reverting an edit removes the overlay entry
    w.set(5, 10, 5, id(0)).unwrap();
    assert_eq!(w.chunk(c0).unwrap().overlay.len(), 0);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn persistence_diff_and_dag() {
    let dir = tmp("p");
    {
        let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
        w.set(1, 1, 1, id(0)).unwrap();
        w.flush_edits().unwrap(); // diff only
        w.set(2, 2, 2, id(0)).unwrap();
        w.consolidate(4).unwrap(); // dag supersedes diff
        w.set(3, 3, 3, id(0)).unwrap();
        w.flush_edits().unwrap();
    }
    {
        let mut w = World::new(
            &dir,
            Box::new(|_, _| panic!("must load from disk")),
            MemSink::default(),
        )
        .unwrap();
        w.load(ChunkPos::new(0, 0, 0)).unwrap();
        assert_eq!(w.get(1, 1, 1), Some(id(0)));
        assert_eq!(w.get(2, 2, 2), Some(id(0)));
        assert_eq!(w.get(3, 3, 3), Some(id(0)));
        assert_eq!(w.get(4, 4, 4), Some(id(1)));
        w.disk_mut().compact((0, 0, 0)).unwrap();
        w.evict(ChunkPos::new(0, 0, 0)).unwrap();
        w.load(ChunkPos::new(0, 0, 0)).unwrap();
        assert_eq!(w.get(3, 3, 3), Some(id(0)));
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn gpu_refcount_dedup_and_table_growth() {
    let mut g = GpuStore::new();
    let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
    for i in 0..3000 {
        g.set_chunk(ChunkPos::new(i, -i, i * 7), &solid);
    }
    assert_eq!(g.chunk_count(), 3000);
    assert_eq!(g.unique_nodes(), 6); // all chunks share one DAG
    for i in 0..3000 {
        assert_eq!(g.lookup(ChunkPos::new(i, -i, i * 7), 1, 2, 3), 1);
    }
    let mut s = MemSink::default();
    g.flush(&mut s);
    assert_eq!(s.table, g.table());
    for i in 0..3000 {
        g.set_chunk(ChunkPos::new(i, -i, i * 7), &Svdag::empty());
    }
    assert_eq!(g.chunk_count(), 0);
    let tag = g.consolidate(std::iter::empty());
    g.reclaim(tag);
    assert_eq!(g.unique_nodes(), 0);
}

#[test]
fn apply_edits_matches_dense_rebuild() {
    // blocky base: solid lower half, a few mixed blocks, lots of air
    let mut d = vec![0u16; CHUNK_VOLUME];
    for z in 0..64 {
        for y in 0..64 {
            for x in 0..64 {
                d[x + y * 64 + z * 4096] = if y < 20 {
                    1
                } else if (x / 8 + z / 8) % 3 == 0 && y < 30 {
                    2
                } else {
                    0
                };
            }
        }
    }
    let base = Svdag::from_dense(&d);
    let mut s = 777u32;
    for n_edits in [1usize, 10, 500, 5000] {
        let mut dd = d.clone();
        let mut edits = std::collections::HashMap::new();
        for _ in 0..n_edits {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let idx = (s >> 4) % CHUNK_VOLUME as u32;
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            edits.insert(idx, ((s >> 16) % 4) as u16);
        }
        for (&i, &v) in &edits {
            dd[i as usize] = v;
        }
        let fast = base.apply_edits(edits.iter().map(|(&i, &v)| (i, v)));
        let slow = Svdag::from_dense(&dd);
        let mut out = vec![0u16; CHUNK_VOLUME];
        fast.expand(&mut out);
        assert_eq!(out, dd, "{n_edits} edits: content");
        assert_eq!(
            fast.pool.len(),
            slow.pool.len(),
            "{n_edits} edits: same canonical node count"
        );
    }
    // carving a solid chunk to nothing, and filling air
    let solid = Svdag::from_dense(&vec![1u16; CHUNK_VOLUME]);
    let all: Vec<(u32, u16)> = (0..CHUNK_VOLUME as u32).map(|i| (i, 0)).collect();
    assert!(solid.apply_edits(all).is_empty());
    let filled = Svdag::empty().apply_edits((0..CHUNK_VOLUME as u32).map(|i| (i, 5)));
    assert_eq!(
        filled.pool.len(),
        Svdag::from_dense(&vec![5u16; CHUNK_VOLUME]).pool.len()
    );
}

#[test]
fn gpu_apply_edits_matches_cpu_and_leaks_nothing() {
    let mut d = vec![0u16; CHUNK_VOLUME];
    for z in 0..64 {
        for y in 0..64 {
            for x in 0..64 {
                d[x + y * 64 + z * 4096] = if y < 24 + ((x * 7 + z * 3) % 5) {
                    1 + ((x / 16) % 3) as u16
                } else {
                    0
                };
            }
        }
    }
    let p = ChunkPos::new(2, 0, 3);
    let mut g = GpuStore::new();
    g.set_chunk(p, &Svdag::from_dense(&d));
    let mut s = 4242u32;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        s >> 8
    };
    for round in 0..25 {
        let n = [1usize, 3, 50, 400][round % 4];
        let mut m = std::collections::HashMap::new();
        for _ in 0..n {
            m.insert(rnd() % CHUNK_VOLUME as u32, (rnd() % 4) as u16);
        }
        for (&i, &v) in &m {
            d[i as usize] = v;
        }
        g.apply_edits(p, m.iter().map(|(&i, &v)| (i, v)));
        let tag = g.end_batch();
        g.reclaim(tag);
        for i in (0..CHUNK_VOLUME).step_by(61) {
            let (x, y, z) = ((i & 63) as u32, ((i >> 6) & 63) as u32, (i >> 12) as u32);
            assert_eq!(g.lookup(p, x, y, z), d[i], "round {round} voxel {i}");
        }
        // no leaks / canonical: node set equals a fresh import of the same content
        let mut fresh = GpuStore::new();
        fresh.set_chunk(p, &Svdag::from_dense(&d));
        assert_eq!(g.unique_nodes(), fresh.unique_nodes(), "round {round}");
    }
    // edit an absent chunk, then erase everything
    let q = ChunkPos::new(-5, 1, 9);
    g.apply_edits(q, [(0u32, 7u16), (CHUNK_VOLUME as u32 - 1, 7)]);
    assert_eq!(g.lookup(q, 0, 0, 0), 7);
    assert_eq!(g.lookup(q, 63, 63, 63), 7);
    g.apply_edits(q, [(0u32, 0u16), (CHUNK_VOLUME as u32 - 1, 0)]);
    assert!(!g.contains(q));
    g.apply_edits(p, (0..CHUNK_VOLUME as u32).map(|i| (i, 0u16)));
    let tag = g.end_batch();
    g.reclaim(tag);
    assert!(!g.contains(p));
    assert_eq!(g.unique_nodes(), 0);
}

#[test]
fn registry_ids_drive_chunk_storage() {
    let mut builder = BlockRegistryBuilder::new();
    builder
        .load_ron(include_str!("../../block/data/blocks.ron"))
        .expect("blocks.ron should load");
    let registry = builder.freeze().unwrap();

    let stone = registry.id_of("voxel:stone").unwrap();
    let dirt = registry.id_of("voxel:dirt").unwrap();
    let water = registry.id_of("voxel:water").unwrap();
    assert!(!stone.is_air());
    assert!(registry.get(stone).unwrap().is_solid());
    assert!(registry.get(water).unwrap().textures.is_some());

    let dir = tmp("blocks");
    let mut w = World::new(
        &dir,
        Box::new(move |_, d| {
            for z in 0..64 {
                for x in 0..64 {
                    for y in 0..32 {
                        d[x + y * 64 + z * 4096] = stone.as_raw();
                    }
                }
            }
        }),
        MemSink::default(),
    )
    .unwrap();
    w.load(ChunkPos::new(0, 0, 0)).unwrap();

    assert_eq!(w.get(3, 4, 5), Some(stone));
    assert_eq!(w.get(3, 40, 5), Some(MaterialId::from_raw(0)));

    w.set(3, 4, 5, dirt).unwrap();
    w.set(3, 40, 5, water).unwrap();
    assert_eq!(w.get(3, 4, 5), Some(dirt));
    assert_eq!(w.get(3, 40, 5), Some(water));

    w.consolidate(4).unwrap();
    assert_eq!(w.get(3, 4, 5), Some(dirt));
    assert_eq!(w.get(3, 40, 5), Some(water));

    std::fs::remove_dir_all(dir).ok();
}
