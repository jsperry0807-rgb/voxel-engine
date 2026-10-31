use std::path::PathBuf;
use std::time::{Duration, Instant};
use voxel_block::{BlockRegistryBuilder, MaterialId};
use voxel_storage::dag::{ROOT_LEVEL, Svdag};
use voxel_storage::gpu::{BLOCKS_PER_CHUNK, GpuStore, MemSink, OVERLAY_VALUES, OVERLAY_WORDS};
use voxel_storage::world::{ConsolidationPolicy, World};
use voxel_storage::{CHUNK_DIM, CHUNK_VOLUME, ChunkPos, local_index, local_xyz};

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

type FillFn = Box<dyn FnMut(ChunkPos, &mut [u16]) + Send>;

fn flat() -> FillFn {
    Box::new(|p, d| {
        if p.y == 0 {
            for z in 0..CHUNK_DIM {
                for y in 0..CHUNK_DIM / 2 {
                    for x in 0..CHUNK_DIM {
                        d[local_index(x as u32, y as u32, z as u32) as usize] = 1;
                    }
                }
            }
        }
    })
}

fn air() -> FillFn {
    Box::new(|_, _| {})
}

#[test]
fn dag_roundtrip() {
    let mut d = vec![0u16; CHUNK_VOLUME];
    let mut s = 12345u32;
    for cell in &mut d {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        if (s >> 24) < 40 {
            *cell = ((s >> 8) & 7) as u16;
        }
    }
    let dag = Svdag::from_dense(&d);
    let mut out = vec![0u16; CHUNK_VOLUME];
    dag.expand(&mut out);
    assert_eq!(d, out);
    for i in (0..CHUNK_VOLUME).step_by(97) {
        let (x, y, z) = local_xyz(i as u32);
        assert_eq!(dag.get(x, y, z), d[i]);
    }
    assert_eq!(Svdag::from_bytes(&dag.to_bytes()).unwrap(), dag);
    // A solid chunk collapses to one node per internal level.
    let solid = Svdag::from_dense(&vec![3u16; CHUNK_VOLUME]);
    assert!(solid.pool.len() <= ROOT_LEVEL as usize * 9);
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
    // The GPU overlay shadows the DAG immediately, before the batch.
    let last = CHUNK_DIM as u32 - 1;
    assert_eq!(w.gpu().lookup(c0, 5, 10, 5), 0);
    assert_eq!(w.gpu().lookup(c0, 5, 40, 5), 2);
    assert_eq!(w.gpu().lookup(ChunkPos::new(-1, 0, -1), last, 0, last), 3);
    assert_eq!(w.dirty_chunks(), 2);
    // A carved voxel is represented by the presence bit, not by a zero value.
    let idx = local_index(5, 10, 5);
    let within = voxel_storage::gpu::block_index(idx);
    let slot = w
        .gpu()
        .block_overlay_slot(c0, idx)
        .expect("chunk 0 has an overlay");
    let base = slot as usize * OVERLAY_WORDS;
    assert_eq!(
        w.gpu().overlays()[base + OVERLAY_VALUES + (within >> 5) as usize] & (1 << (within & 31)),
        1 << (within & 31),
    );
    w.flush_gpu();
    assert_eq!(w.sink().table, w.gpu().table());
    assert_eq!(
        &w.sink().overlays[..w.gpu().overlays().len()],
        w.gpu().overlays()
    );

    let r = w.consolidate(16).unwrap();
    assert_eq!(r.chunks, 2);
    assert_eq!(w.dirty_chunks(), 0);
    assert_eq!(w.gpu().lookup(c0, 5, 10, 5), 0);
    assert_eq!(w.gpu().lookup(c0, 5, 40, 5), 2);
    assert_eq!(w.gpu().lookup(ChunkPos::new(-1, 0, -1), last, 0, last), 3);
    // Folding clears the overlay; the DAG now carries the edit.
    assert!(w.gpu().block_table_slot(c0).is_none());
    // sink mirrors CPU copy
    let g = w.gpu();
    assert_eq!(&w.sink().nodes[..g.pool().len()], g.pool());
    assert_eq!(w.sink().table, g.table());

    w.reclaim(r.epoch);
    // reverting an edit removes the overlay entry
    w.set(5, 10, 5, id(0)).unwrap();
    assert_eq!(w.chunk(c0).unwrap().overlay.len(), 0);
    assert!(w.gpu().block_table_slot(c0).is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn overlay_only_chunk_survives_and_folds() {
    let dir = tmp("overlay-only");
    let mut w = World::new(&dir, air(), MemSink::default()).unwrap();
    let p = ChunkPos::new(0, 0, 0);

    // An all-air chunk has no DAG; the overlay alone keeps it addressable.
    w.set(1, 2, 3, id(3)).unwrap();
    assert_eq!(w.gpu().lookup(p, 1, 2, 3), 3);
    assert!(w.gpu().contains(p));
    assert_eq!(w.gpu().root_of(p), voxel_storage::dag::NONE);
    assert!(w.gpu().block_table_slot(p).is_some());

    // Reverting to air removes the override and the now-empty chunk entry.
    w.set(1, 2, 3, id(0)).unwrap();
    assert!(!w.gpu().contains(p));
    assert!(w.gpu().block_table_slot(p).is_none());

    // A fresh placement folds into a real DAG at consolidation.
    w.set(4, 5, 6, id(3)).unwrap();
    let r = w.consolidate(4).unwrap();
    assert_eq!(r.chunks, 1);
    assert!(w.gpu().block_table_slot(p).is_none());
    assert_eq!(w.gpu().lookup(p, 4, 5, 6), 3);
    w.reclaim(r.epoch);

    std::fs::remove_dir_all(dir).ok();
}

/// A stream-only session (load, evict, never edit) must still be able to free
/// the pool: `tick` ends no batch when nothing is dirty, so the eviction has to.
#[test]
fn eviction_advances_the_gpu_epoch() {
    let dir = tmp("evict-epoch");
    let p = ChunkPos::new(0, 0, 0);
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.load(p).unwrap();
    let before = w.gpu().next_epoch();
    let tag = w.evict(p).unwrap();
    assert_eq!(tag, before, "the eviction retires under the current tag");
    assert_eq!(w.gpu().next_epoch(), before + 1);
    w.reclaim(tag);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn gpu_overlay_is_restored_from_disk() {
    let dir = tmp("overlay-disk");
    let p = ChunkPos::new(0, 0, 0);
    {
        let mut w = World::new(&dir, air(), MemSink::default()).unwrap();
        w.set(2, 3, 4, id(5)).unwrap();
        w.flush_edits().unwrap();
        w.evict(p).unwrap();
        assert!(!w.gpu().contains(p));
    }
    {
        let mut w = World::new(
            &dir,
            Box::new(|_, _| panic!("must load from disk")),
            MemSink::default(),
        )
        .unwrap();
        w.load(p).unwrap();
        assert_eq!(w.get(2, 3, 4), Some(id(5)));
        assert_eq!(w.gpu().lookup(p, 2, 3, 4), 5);
        assert!(w.gpu().block_table_slot(p).is_some());
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn overlay_slots_are_retired_and_reused() {
    let mut g = GpuStore::new();
    let p = ChunkPos::new(0, 0, 0);
    for i in 0..4 {
        g.set_overlay(p, i, 2);
    }
    let first_table = g.block_table_slot(p).expect("block table attached");
    let first_overlay = g.block_overlay_slot(p, 0).expect("overlay attached");
    g.clear_chunk_overlay(p);
    assert!(!g.contains(p));
    assert!(g.block_table_slot(p).is_none());

    let tag = g.end_batch();
    g.reclaim(tag);
    g.set_overlay(p, 9, 3);
    assert_eq!(g.block_table_slot(p), Some(first_table), "recycled table");
    assert_eq!(
        g.block_overlay_slot(p, 9),
        Some(first_overlay),
        "recycled overlay"
    );
    assert_eq!(g.lookup(p, 9, 0, 0), 3);
    assert_eq!(g.lookup(p, 0, 0, 0), 0, "stale entries were cleared");
}

#[test]
fn overlay_pool_grows_and_reuses_slots() {
    let mut g = GpuStore::new();
    for i in 0..8 {
        g.set_overlay(ChunkPos::new(i, 0, 0), 0, 4);
    }
    assert_eq!(g.overlays().len(), 8 * OVERLAY_WORDS);
    assert_eq!(g.block_tables().len(), 8 * BLOCKS_PER_CHUNK);

    for i in 0..8 {
        g.clear_chunk_overlay(ChunkPos::new(i, 0, 0));
    }
    assert_eq!(g.chunk_count(), 0);
    let tag = g.end_batch();
    g.reclaim(tag);

    g.set_overlay(ChunkPos::new(0, 0, 0), 0, 4);
    assert!(g.block_overlay_slot(ChunkPos::new(0, 0, 0), 0).unwrap() < 8);
    assert_eq!(g.overlays().len(), 8 * OVERLAY_WORDS);
    assert_eq!(g.block_tables().len(), 8 * BLOCKS_PER_CHUNK);
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
    assert_eq!(g.unique_nodes(), ROOT_LEVEL as usize); // all chunks share one DAG
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
    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                d[local_index(x as u32, y as u32, z as u32) as usize] = if y < 20 {
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
    for z in 0..CHUNK_DIM {
        for y in 0..CHUNK_DIM {
            for x in 0..CHUNK_DIM {
                d[local_index(x as u32, y as u32, z as u32) as usize] =
                    if y < 24 + ((x * 7 + z * 3) % 5) {
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
            let (x, y, z) = local_xyz(i as u32);
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
    let last = CHUNK_DIM as u32 - 1;
    assert_eq!(g.lookup(q, 0, 0, 0), 7);
    assert_eq!(g.lookup(q, last, last, last), 7);
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
            for z in 0..CHUNK_DIM {
                for x in 0..CHUNK_DIM {
                    for y in 0..32 {
                        d[local_index(x as u32, y as u32, z as u32) as usize] = stone.as_raw();
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

fn policy(
    size_trigger: usize,
    idle_secs: u64,
    frame_budget: Duration,
    max_chunks: usize,
) -> ConsolidationPolicy {
    ConsolidationPolicy {
        size_trigger,
        idle_trigger: Duration::from_secs(idle_secs),
        frame_budget,
        max_chunks,
    }
}

#[test]
fn consolidation_policy_waits_for_idle() {
    let dir = tmp("policy-idle");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.set_policy(policy(usize::MAX, 30, Duration::from_secs(10), 8));

    w.set(5, 10, 5, id(0)).unwrap();
    assert_eq!(w.pending_chunks(), 1);
    assert!(w.tick(Instant::now()).unwrap().is_none(), "no trigger yet");

    let later = Instant::now() + Duration::from_secs(31);
    let r = w.tick(later).unwrap().expect("idle trigger");
    assert_eq!(r.chunks, 1);
    assert_eq!(r.remaining, 0);
    assert_eq!(w.pending_chunks(), 0);
    assert_eq!(w.gpu().lookup(ChunkPos::new(0, 0, 0), 5, 10, 5), 0);
    w.reclaim(r.epoch);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn consolidation_policy_size_trigger_folds_without_idle() {
    let dir = tmp("policy-size");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.set_policy(policy(2, 3_600, Duration::from_secs(10), 8));

    w.set(1, 1, 1, id(0)).unwrap();
    assert!(
        w.tick(Instant::now()).unwrap().is_none(),
        "one entry is below the size trigger"
    );

    w.set(2, 2, 2, id(0)).unwrap();
    let r = w.tick(Instant::now()).unwrap().expect("size trigger");
    assert_eq!(r.chunks, 1);
    assert_eq!(r.remaining, 0);
    assert_eq!(w.pending_chunks(), 0);
    w.reclaim(r.epoch);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn consolidation_policy_budget_stops_between_chunks() {
    let dir = tmp("policy-budget");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.set_policy(policy(1, 3_600, Duration::ZERO, 8));

    w.set(1, 1, 1, id(0)).unwrap();
    w.set(CHUNK_DIM as i32 + 1, 1, 1, id(0)).unwrap(); // chunk (1, 0, 0), local x = 1
    assert_eq!(w.pending_chunks(), 2);

    let first = w.tick(Instant::now()).unwrap().expect("size trigger");
    assert_eq!(first.chunks, 1, "a zero budget still makes progress");
    assert_eq!(first.remaining, 1);

    let second = w.tick(Instant::now()).unwrap().expect("size trigger again");
    assert_eq!(second.chunks, 1);
    assert_eq!(second.remaining, 0);

    w.reclaim(first.epoch);
    w.reclaim(second.epoch);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pending_chunks_ignores_a_fully_reverted_overlay() {
    let dir = tmp("policy-revert");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.set_policy(policy(1, 30, Duration::from_secs(10), 8));

    w.set(1, 1, 1, id(0)).unwrap();
    w.set(1, 1, 1, id(1)).unwrap(); // back to the DAG value
    assert_eq!(w.pending_chunks(), 0);
    assert_eq!(w.dirty_chunks(), 1, "still needs an empty diff write");
    assert!(
        w.tick(Instant::now() + Duration::from_secs(31))
            .unwrap()
            .is_none()
    );

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn consolidation_policy_max_chunks_zero_disables_tick() {
    let dir = tmp("policy-disabled");
    let mut w = World::new(&dir, flat(), MemSink::default()).unwrap();
    w.set_policy(policy(1, 0, Duration::from_secs(10), 0));
    w.set(1, 1, 1, id(0)).unwrap();
    assert!(w.tick(Instant::now()).unwrap().is_none());
    std::fs::remove_dir_all(dir).ok();
}
