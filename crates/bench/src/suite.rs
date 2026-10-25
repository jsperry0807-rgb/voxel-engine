//! The software benchmark cases: memory, CPU/throughput and latency.
//!
//! Each case measures one data structure or one hot path. Setup (fixtures,
//! input generation) happens outside the measured region; the notes say what
//! the measured region includes.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use voxel_block::{MaterialId, MissingBlockPolicy, NamespacedKey, Palette};
use voxel_storage::CHUNK_VOLUME;
use voxel_storage::ChunkPos;
use voxel_storage::dag::Svdag;
use voxel_storage::disk::RegionStore;
use voxel_storage::gpu::{GpuStore, MemSink};
use voxel_storage::overlay::Overlay;
use voxel_storage::world::World;

use crate::fixtures;
use crate::mem;
use crate::patterns;
use crate::proc;
use crate::report::{CpuRecord, LatencyRecord, MemoryRecord, bytes};

// ------------------------------------------------------------------ memory ---

/// Every memory case, in report order.
pub fn memory() -> Vec<MemoryRecord> {
    let mut out = Vec::new();
    memory_registry(&mut out);
    memory_chunks(&mut out);
    memory_world(&mut out);
    memory_gpu_tables(&mut out);
    memory_overlay(&mut out);
    memory_gpu_overlay(&mut out);
    memory_disk(&mut out);
    out
}

fn memory_registry(out: &mut Vec<MemoryRecord>) {
    for blocks in [1_024, 65_536] {
        let (registry, stats) = mem::measure(|| fixtures::registry(blocks));
        let hot = std::mem::size_of_val(registry.hot_table());
        let sets = std::mem::size_of_val(registry.texture_sets());
        out.push(MemoryRecord {
            case: format!("registry/{blocks}"),
            resident: stats.resident,
            allocated: stats.allocated,
            allocs: stats.allocs,
            peak: stats.peak,
            note: format!(
                "{:.1} B/block; hot {}; sets {} ({}); {} textures",
                stats.resident as f64 / blocks as f64,
                bytes(hot as u64),
                registry.texture_sets().len(),
                bytes(sets as u64),
                registry.textures().len(),
            ),
        });
    }
}

fn memory_chunks(out: &mut Vec<MemoryRecord>) {
    let dense_bytes = (CHUNK_VOLUME * size_of::<u16>()) as u64;
    for (name, dense) in [
        ("solid", patterns::solid(1)),
        ("terrain", patterns::terrain(0, 0)),
        ("noise30%", patterns::noise(0x1234, 300)),
        ("sparse4k", patterns::sparse(0x5678, 4096)),
    ] {
        let (dag, stats) = mem::measure(|| Svdag::from_dense(&dense));
        let words = dag.words();
        let dag_bytes = (words * size_of::<u32>()) as u64;
        out.push(MemoryRecord {
            case: format!("chunk/{name}"),
            resident: stats.resident,
            allocated: stats.allocated,
            allocs: stats.allocs,
            peak: stats.peak,
            note: format!(
                "{words} words = {}; {:.1}x smaller than dense {}; root {}",
                bytes(dag_bytes),
                dense_bytes as f64 / dag_bytes.max(1) as f64,
                bytes(dense_bytes),
                dag.root,
            ),
        });
    }
}

fn memory_world(out: &mut Vec<MemoryRecord>) {
    for chunks_wanted in [16usize, 64, 256] {
        let chunks: Vec<(ChunkPos, Svdag)> = (0..chunks_wanted as i32)
            .map(|i| {
                let (x, z) = (i % 16, i / 16);
                (
                    ChunkPos::new(x, 0, z),
                    Svdag::from_dense(&patterns::terrain(x, z)),
                )
            })
            .collect();
        let source_words: usize = chunks.iter().map(|(_, dag)| dag.words()).sum();

        let (store, stats) = mem::measure(|| {
            let mut store = GpuStore::new();
            for (pos, dag) in &chunks {
                store.set_chunk(*pos, dag);
            }
            store
        });

        let pool_words = store.pool().len();
        let table_words = store.table().len();
        let total = ((pool_words + table_words) * size_of::<u32>()) as u64;
        out.push(MemoryRecord {
            case: format!("world/{chunks_wanted}_chunks_gpu"),
            resident: stats.resident,
            allocated: stats.allocated,
            allocs: stats.allocs,
            peak: stats.peak,
            note: format!(
                "{} chunks -> {} unique nodes; pool {} + table {} (len; resident includes capacity slack, rc and dedup); {:.1} KiB/chunk; {:.1}x word dedup",
                store.chunk_count(),
                store.unique_nodes(),
                bytes(std::mem::size_of_val(store.pool()) as u64),
                bytes(std::mem::size_of_val(store.table()) as u64),
                total as f64 / chunks_wanted as f64 / 1024.0,
                source_words as f64 / pool_words.max(1) as f64,
            ),
        });
    }
}

fn memory_gpu_tables(out: &mut Vec<MemoryRecord>) {
    const BLOCKS: usize = 65_536;
    let registry = fixtures::registry(BLOCKS);
    let ((hot, sets), stats) =
        mem::measure(|| (registry.gpu_hot_words(), registry.gpu_texture_set_words()));
    out.push(MemoryRecord {
        case: format!("gpu_tables/{BLOCKS}"),
        resident: stats.resident,
        allocated: stats.allocated,
        allocs: stats.allocs,
        peak: stats.peak,
        note: format!(
            "{} hot words + {} set words = {}; {:.1} B/block",
            hot.len(),
            sets.len(),
            bytes(stats.resident),
            stats.resident as f64 / BLOCKS as f64,
        ),
    });
}

fn memory_overlay(out: &mut Vec<MemoryRecord>) {
    for edits in [1_024usize, 4_096] {
        let (overlay, stats) = mem::measure(|| {
            let mut overlay = Overlay::default();
            for i in 0..edits {
                overlay.set(
                    (i as u32).wrapping_mul(2_654_435_761) % CHUNK_VOLUME as u32,
                    2,
                );
            }
            overlay
        });
        out.push(MemoryRecord {
            case: format!("overlay/{edits}"),
            resident: stats.resident,
            allocated: stats.allocated,
            allocs: stats.allocs,
            peak: stats.peak,
            note: format!(
                "{} entries; {:.1} B/edit; peak {}",
                overlay.len(),
                stats.resident as f64 / edits as f64,
                bytes(stats.peak),
            ),
        });
    }
}

fn memory_gpu_overlay(out: &mut Vec<MemoryRecord>) {
    let pos = ChunkPos::new(0, 0, 0);
    let dag = Svdag::from_dense(&patterns::terrain(0, 0));
    for blocks in [1usize, 64] {
        let (store, stats) = mem::measure(|| {
            let mut store = GpuStore::new();
            store.set_chunk(pos, &dag);
            for b in 0..blocks as u32 {
                // The first voxel of block `b`, so every write allocates one slot.
                let idx = ((b & 3) << 4) | (((b >> 2) & 3) << 10) | (((b >> 4) & 3) << 16);
                store.set_overlay(pos, idx, 3);
            }
            store
        });
        out.push(MemoryRecord {
            case: format!("gpu_overlay/{blocks}"),
            resident: stats.resident,
            allocated: stats.allocated,
            allocs: stats.allocs,
            peak: stats.peak,
            note: format!(
                "{blocks} edited 16^3 block(s); {} overlay words ({:.1} KiB/block)",
                store.overlays().len(),
                store.overlays().len() as f64 * 4.0 / blocks as f64 / 1024.0,
            ),
        });
    }
}

fn memory_disk(out: &mut Vec<MemoryRecord>) {
    let dir = temp_dir("disk");
    let chunks: Vec<(ChunkPos, Svdag)> = (0..64i32)
        .map(|i| {
            let (x, z) = (i % 8, i / 8);
            (
                ChunkPos::new(x, 0, z),
                Svdag::from_dense(&patterns::terrain(x, z)),
            )
        })
        .collect();

    let (store, stats) = mem::measure(|| {
        let mut store = RegionStore::open(&dir).expect("open region store");
        for (pos, dag) in &chunks {
            store.put_dag(*pos, dag).expect("write dag");
        }
        store.sync().expect("sync region store");
        store
    });
    let on_disk = dir_bytes(&dir);
    out.push(MemoryRecord {
        case: "disk/64_chunks".to_owned(),
        resident: stats.resident,
        allocated: stats.allocated,
        allocs: stats.allocs,
        peak: stats.peak,
        note: format!(
            "{} on disk; {:.1} KiB/chunk; peak heap {}",
            bytes(on_disk),
            on_disk as f64 / 64.0 / 1024.0,
            bytes(stats.peak),
        ),
    });

    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

// --------------------------------------------------------------------- CPU ---

/// Every CPU/throughput case, in report order.
pub fn cpu() -> Vec<CpuRecord> {
    let mut out = Vec::new();
    cpu_registry(&mut out);
    cpu_lookups(&mut out);
    cpu_dag(&mut out);
    cpu_edits(&mut out);
    cpu_gpu(&mut out);
    cpu_world(&mut out);
    cpu_render(&mut out);
    out
}

fn cpu_registry(out: &mut Vec<CpuRecord>) {
    for blocks in [1_024, 65_536] {
        out.push(bench_ops(
            format!("registry/build_freeze/{blocks}"),
            "register + freeze; includes key and texture resolution",
            || fixtures::registry(blocks),
        ));
    }
}

fn cpu_lookups(out: &mut Vec<CpuRecord>) {
    let registry = fixtures::registry(4_096);
    let random = fixtures::random_ids(&registry, 1 << 16, 0xC0FF_EE01);
    let coherent = fixtures::coherent_ids(&registry, 1 << 16, 0xC0FF_EE02);
    let keys: Vec<NamespacedKey> = registry.keys().map(|(_, key)| key.clone()).collect();

    let mut ids = random.iter().cycle();
    out.push(bench_ops(
        "lookup/hot/random",
        "8-byte hot record, uniformly random ids",
        || {
            let id = *ids.next().expect("cycle is infinite");
            black_box(registry.hot(id))
        },
    ));

    let mut ids = coherent.iter().cycle();
    out.push(bench_ops(
        "lookup/hot/coherent",
        "8-byte hot record, runs of 16 ids",
        || {
            let id = *ids.next().expect("cycle is infinite");
            black_box(registry.hot(id))
        },
    ));

    let mut ids = random.iter().cycle();
    out.push(bench_ops(
        "lookup/resolved/random",
        "assembled BlockRecord; one hot load + set lookup + template hash",
        || {
            let id = *ids.next().expect("cycle is infinite");
            black_box(registry.resolved(id))
        },
    ));

    let mut key_iter = keys.iter().cycle();
    out.push(bench_ops(
        "lookup/key/random",
        "HashMap<NamespacedKey, MaterialId>",
        || black_box(registry.id(key_iter.next().expect("cycle is infinite"))),
    ));

    let palette = Palette::new(keys.to_vec());
    out.push(bench_ops(
        "palette/resolve_4096",
        "key -> id for 4 096 entries; allocates the id vector",
        || {
            black_box(
                registry
                    .resolve_palette(&palette, MissingBlockPolicy::Error)
                    .expect("palette names registered blocks"),
            )
        },
    ));
}

fn cpu_dag(out: &mut Vec<CpuRecord>) {
    let solid = patterns::solid(1);
    let terrain = patterns::terrain(0, 0);
    let noise = patterns::noise(0x1234, 300);
    out.push(bench_ops("dag/build/solid", "64^3 dense -> SVDAG", || {
        Svdag::from_dense(&solid)
    }));
    out.push(bench_ops(
        "dag/build/terrain",
        "64^3 dense -> SVDAG",
        || Svdag::from_dense(&terrain),
    ));
    out.push(bench_ops(
        "dag/build/noise30%",
        "64^3 dense -> SVDAG",
        || Svdag::from_dense(&noise),
    ));

    let dag = Svdag::from_dense(&terrain);
    let coords = random_coords(1 << 16, 0xDEAD_BEEF);
    // Slice iterators clone in O(1) when the cycle wraps; `IntoIter` would
    // reallocate the whole vector on every wrap.
    let mut coords = coords.iter().copied().cycle();
    out.push(bench_ops(
        "dag/get/random",
        "six-level octree descent, random voxel",
        || {
            let (x, y, z) = coords.next().expect("cycle is infinite");
            black_box(dag.get(x, y, z))
        },
    ));

    for edits in [1usize, 100, 1_000] {
        let edits: Vec<(u32, u16)> = random_indices(edits, 0xFEED)
            .into_iter()
            .map(|index| (index, 4))
            .collect();
        out.push(bench_ops(
            format!("dag/apply_edits/{edits_len}", edits_len = edits.len()),
            "copy-on-write rebuild of the edited paths",
            || black_box(dag.apply_edits(edits.iter().copied()).words()),
        ));
    }
}

fn cpu_edits(out: &mut Vec<CpuRecord>) {
    let mut overlay = Overlay::default();
    let mut index = 0u32;
    out.push(bench_ops("overlay/set", "HashMap insert", || {
        index = index.wrapping_add(1);
        overlay.set(index.wrapping_mul(2_654_435_761) % CHUNK_VOLUME as u32, 2);
    }));

    let mut overlay = Overlay::default();
    for i in 0..4_096u32 {
        overlay.set(i.wrapping_mul(2_654_435_761) % CHUNK_VOLUME as u32, 2);
    }
    let get_indices = random_indices(1 << 16, 0xBEEF);
    let mut indices = get_indices.iter().copied().cycle();
    out.push(bench_ops(
        "overlay/get",
        "4 096-entry overlay, random hit",
        || {
            let index = indices.next().expect("cycle is infinite");
            black_box(overlay.get(index))
        },
    ));
}

fn cpu_gpu(out: &mut Vec<CpuRecord>) {
    let terrain = patterns::terrain(0, 0);
    let dag = Svdag::from_dense(&terrain);
    let pos = ChunkPos::new(0, 0, 0);

    out.push(bench_ops(
        "gpu/set_chunk",
        "fresh GpuStore per op; import + table insert",
        || {
            let mut store = GpuStore::new();
            store.set_chunk(pos, &dag);
            black_box(store.unique_nodes())
        },
    ));

    let mut store = GpuStore::new();
    store.set_chunk(pos, &dag);
    let coords = random_coords(1 << 16, 0x1234_5678);
    let mut coords = coords.iter().copied().cycle();
    out.push(bench_ops(
        "gpu/lookup",
        "hash-table probe + octree descent",
        || {
            let (x, y, z) = coords.next().expect("cycle is infinite");
            black_box(store.lookup(pos, x, y, z))
        },
    ));

    // Block 0 fully overridden, so the coordinates below hit the overlay path.
    let mut overlay_store = GpuStore::new();
    overlay_store.set_chunk(pos, &dag);
    for within in 0..4096u32 {
        let x = within & 15;
        let y = (within >> 4) & 15;
        let z = (within >> 8) & 15;
        overlay_store.set_overlay(pos, x | (y << 6) | (z << 12), 4);
    }
    let mut overlay_coords = random_coords(1 << 16, 0x8765_4321)
        .into_iter()
        .map(|(x, y, z)| (x & 15, y & 15, z & 15))
        .cycle();
    out.push(bench_ops(
        "gpu/lookup_overlay",
        "probe + block table + presence bitmap hit",
        || {
            let (x, y, z) = overlay_coords.next().expect("cycle is infinite");
            black_box(overlay_store.lookup(pos, x, y, z))
        },
    ));

    let mut overlay_index = 0u32;
    out.push(bench_ops(
        "gpu/overlay_set/100",
        "100 overlay writes in one 16^3 block",
        || {
            for _ in 0..100 {
                overlay_index = (overlay_index + 1) & 0xFFF;
                overlay_store.set_overlay(pos, overlay_index, 4);
            }
            black_box(overlay_index)
        },
    ));

    let edits: Vec<(u32, u16)> = random_indices(100, 0x9999)
        .into_iter()
        .map(|index| (index, 4))
        .collect();
    let mut value = 4u16;
    out.push(bench_ops(
        "gpu/apply_edits/100",
        "in-place COW edit + immediate reclaim, 100 paths",
        || {
            value = if value == 4 { 5 } else { 4 };
            store.apply_edits(pos, edits.iter().map(|&(index, _)| (index, value)));
            let tag = store.end_batch();
            store.reclaim(tag);
        },
    ));

    let mut store = GpuStore::new();
    for i in 0..64i32 {
        let (x, z) = (i % 8, i / 8);
        let dag = Svdag::from_dense(&patterns::terrain(x, z));
        store.set_chunk(ChunkPos::new(x, 0, z), &dag);
        store.end_batch();
    }
    let mut sink = MemSink::default();
    let mut index = 0u32;
    out.push(bench_ops(
        "gpu/edit+flush/64_loaded",
        "one edit, coalesced dirty range, in-memory sink",
        || {
            index = index.wrapping_add(1);
            store.apply_edits(pos, [(index % CHUNK_VOLUME as u32, 4)]);
            store.flush(&mut sink);
            let tag = store.end_batch();
            store.reclaim(tag);
            black_box(sink.bytes_written)
        },
    ));

    let mut store = GpuStore::new();
    for i in 0..64i32 {
        let (x, z) = (i % 8, i / 8);
        let dag = Svdag::from_dense(&patterns::terrain(x, z));
        store.set_chunk(ChunkPos::new(x, 0, z), &dag);
        store.end_batch();
    }
    let mut sink = MemSink::default();
    let mut index = 0u32;
    out.push(bench_ops(
        "gpu/overlay_flush/64_loaded",
        "one overlay write per chunk, dirty ranges folded, in-memory sink",
        || {
            index = index.wrapping_add(1);
            for i in 0..64i32 {
                let (x, z) = (i % 8, i / 8);
                store.set_overlay(ChunkPos::new(x, 0, z), index & 0xFFF, 4);
            }
            store.flush(&mut sink);
            let tag = store.end_batch();
            store.reclaim(tag);
            black_box(sink.bytes_written)
        },
    ));
}

fn cpu_world(out: &mut Vec<CpuRecord>) {
    let dir = temp_dir("world-tick");
    let mut world = World::new(
        &dir,
        Box::new(|p: ChunkPos, dense: &mut [u16]| {
            dense.copy_from_slice(&patterns::terrain(p.x, p.z));
        }),
        MemSink::default(),
    )
    .expect("open world");
    world.load(ChunkPos::new(0, 0, 0)).expect("load chunk");
    let now = Instant::now();
    out.push(bench_ops(
        "world/tick_idle",
        "policy check with no pending overlays",
        || black_box(world.tick(now).expect("tick")),
    ));
    drop(world);
    std::fs::remove_dir_all(dir).ok();
}

fn cpu_render(out: &mut Vec<CpuRecord>) {
    const BLOCKS: usize = 65_536;
    let registry = fixtures::registry(BLOCKS);
    out.push(bench_ops(
        "render/hot_words/65536",
        "pack the whole hot table for GPU upload",
        || black_box(registry.gpu_hot_words()),
    ));
    out.push(bench_ops(
        "render/texture_set_words/65536",
        "pack every distinct texture set for GPU upload",
        || black_box(registry.gpu_texture_set_words()),
    ));
}

// ----------------------------------------------------------------- latency ---

/// Every latency case, in report order.
pub fn latency() -> Vec<LatencyRecord> {
    let mut out = Vec::new();
    let pos = ChunkPos::new(0, 0, 0);
    let terrain = Svdag::from_dense(&patterns::terrain(0, 0));

    let registry = fixtures::registry(4_096);
    let ids = fixtures::random_ids(&registry, 1 << 14, 0xAA55);
    out.push(latency_case(
        "latency/hot_lookup",
        "random id, hot table",
        2_000,
        |i| {
            black_box(registry.hot(ids[i % ids.len()]));
        },
    ));
    let dag = &terrain;
    let coords = random_coords(8_192, 0x55AA);
    out.push(latency_case(
        "latency/dag_get",
        "random voxel in a terrain DAG",
        2_000,
        |i| {
            let (x, y, z) = coords[i % coords.len()];
            black_box(dag.get(x, y, z));
        },
    ));

    let mut store = GpuStore::new();
    store.set_chunk(pos, &terrain);
    let mut value = 4u16;
    for _ in 0..64 {
        value = if value == 4 { 5 } else { 4 };
        store.apply_edits(pos, [(7, value)]);
        let tag = store.end_batch();
        store.reclaim(tag);
    }
    let mut durations = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        value = if value == 4 { 5 } else { 4 };
        let start = Instant::now();
        store.apply_edits(pos, [(7, value)]);
        durations.push(start.elapsed().as_nanos() as f64 / 1_000.0);
        let tag = store.end_batch();
        store.reclaim(tag);
    }
    out.push(summarize(
        "latency/gpu_edit_1",
        "one COW edit on a resident chunk; reclaim excluded",
        durations,
    ));

    let mut store = GpuStore::new();
    store.set_chunk(pos, &terrain);
    store.set_overlay(pos, 0, 4); // allocate the block table + overlay slot outside the samples
    let mut overlay_index = 0u32;
    let mut durations = Vec::with_capacity(2_000);
    for _ in 0..2_000 {
        overlay_index = (overlay_index + 1) & 0xFFF;
        let start = Instant::now();
        store.set_overlay(pos, overlay_index, 4);
        durations.push(start.elapsed().as_nanos() as f64 / 1_000.0);
    }
    out.push(summarize(
        "latency/gpu_overlay_set",
        "one overlay write; block table + slot already resident",
        durations,
    ));

    let dir = temp_dir("world-latency");
    let mut world = World::new(
        &dir,
        Box::new(|p: ChunkPos, dense: &mut [u16]| {
            dense.copy_from_slice(&patterns::terrain(p.x, p.z));
        }),
        MemSink::default(),
    )
    .expect("open world");
    world.load(ChunkPos::new(0, 0, 0)).expect("load chunk");
    let mut overlay_writes = 0u32;
    out.push(latency_case(
        "latency/world_set",
        "overlay write through World::set, chunk resident",
        2_000,
        |_| {
            overlay_writes = overlay_writes.wrapping_add(1);
            let x = (overlay_writes.wrapping_mul(2_654_435_761) & 63) as i32;
            let y = ((overlay_writes >> 6) & 63) as i32;
            let z = ((overlay_writes >> 12) & 63) as i32;
            world
                .set(x, y, z, MaterialId::from_raw(1))
                .expect("set voxel");
        },
    ));
    drop(world);
    std::fs::remove_dir_all(dir).ok();

    // Policy path: an idle frame is a cheap check; an event-triggered fold pays
    // the DAG rebuild and the disk sync.
    let dir = temp_dir("world-policy");
    let mut world = World::new(
        &dir,
        Box::new(|p: ChunkPos, dense: &mut [u16]| {
            dense.copy_from_slice(&patterns::terrain(p.x, p.z));
        }),
        MemSink::default(),
    )
    .expect("open world");
    world.load(ChunkPos::new(0, 0, 0)).expect("load chunk");
    let now = Instant::now();
    out.push(latency_case(
        "latency/world_tick_idle",
        "policy check with no pending overlays",
        2_000,
        |_| {
            black_box(world.tick(now).expect("tick"));
        },
    ));

    let mut next = 0u32;
    let mut next_voxel = || {
        let index = next;
        next = next.wrapping_add(1);
        (
            (index & 63) as i32,
            ((index >> 6) & 63) as i32,
            (index >> 12) as i32,
        )
    };
    for _ in 0..16 {
        let (x, y, z) = next_voxel();
        world
            .set(x, y, z, MaterialId::from_raw(4))
            .expect("set voxel");
        let r = world.consolidate(1).expect("consolidate");
        assert_eq!(r.remaining, 0);
        world.reclaim(r.epoch);
    }
    let mut durations = Vec::with_capacity(512);
    for _ in 0..512 {
        let (x, y, z) = next_voxel();
        let start = Instant::now();
        world
            .set(x, y, z, MaterialId::from_raw(4))
            .expect("set voxel");
        let r = world.consolidate(1).expect("consolidate");
        durations.push(start.elapsed().as_nanos() as f64 / 1_000.0);
        world.reclaim(r.epoch);
    }
    out.push(summarize(
        "latency/world_consolidate_1",
        "one edit + event fold; DAG rebuild and disk sync included",
        durations,
    ));
    drop(world);
    std::fs::remove_dir_all(dir).ok();

    out
}

// ---------------------------------------------------------------- harness ---

const CALIBRATE: Duration = Duration::from_millis(20);
const ROUNDS: usize = 3;
const MAX_ITERS: u64 = 1 << 22;

fn bench_ops<T>(
    case: impl Into<String>,
    note: impl Into<String>,
    mut op: impl FnMut() -> T,
) -> CpuRecord {
    for _ in 0..3 {
        black_box(op());
    }

    let mut iters = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..iters {
            black_box(op());
        }
        if start.elapsed() >= CALIBRATE || iters >= MAX_ITERS {
            break;
        }
        iters = (iters * 2).min(MAX_ITERS);
    }

    let mut best: Option<(u64, Option<u64>, u64, u64, u64)> = None;
    for _ in 0..ROUNDS {
        let cpu_start = proc::cpu_time();
        let start = Instant::now();
        let ((), stats) = mem::measure(|| {
            for _ in 0..iters {
                black_box(op());
            }
        });
        let wall_ns = start.elapsed().as_nanos() as u64;
        let cpu_ns = match (cpu_start, proc::cpu_time()) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a).as_nanos() as u64),
            _ => None,
        };
        let candidate = (wall_ns, cpu_ns, stats.allocated, stats.allocs, stats.peak);
        if best.is_none_or(|b| wall_ns < b.0) {
            best = Some(candidate);
        }
    }

    let (wall_ns, cpu_ns, allocated, allocs, peak) = best.expect("at least one round");
    let seconds = wall_ns as f64 / 1e9;
    CpuRecord {
        case: case.into(),
        iters,
        wall_ns,
        cpu_ns,
        ns_per_op: wall_ns as f64 / iters as f64,
        mops_per_sec: iters as f64 / seconds / 1e6,
        cpu_percent: cpu_ns.map(|cpu| cpu as f64 / wall_ns as f64 * 100.0),
        allocated,
        allocs,
        peak,
        bytes_per_op: allocated as f64 / iters as f64,
        allocs_per_op: allocs as f64 / iters as f64,
        note: note.into(),
    }
}

fn latency_case(
    case: &str,
    note: &str,
    samples: usize,
    mut op: impl FnMut(usize),
) -> LatencyRecord {
    for i in 0..64 {
        op(i);
    }
    let mut durations = Vec::with_capacity(samples);
    for i in 0..samples {
        let start = Instant::now();
        op(i);
        durations.push(start.elapsed().as_nanos() as f64 / 1_000.0);
    }
    summarize(case, note, durations)
}

fn summarize(case: &str, note: &str, mut durations: Vec<f64>) -> LatencyRecord {
    durations.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        let index = ((durations.len() - 1) as f64 * p).round() as usize;
        durations[index]
    };
    LatencyRecord {
        case: case.to_owned(),
        iters: durations.len() as u64,
        p50_us: percentile(0.50),
        p95_us: percentile(0.95),
        p99_us: percentile(0.99),
        max_us: *durations.last().expect("at least one sample"),
        note: note.to_owned(),
    }
}

fn random_coords(count: usize, seed: u64) -> Vec<(u32, u32, u32)> {
    let mut state = seed | 1;
    (0..count)
        .map(|_| {
            let r = rng(&mut state);
            (
                (r & 63) as u32,
                ((r >> 6) & 63) as u32,
                ((r >> 12) & 63) as u32,
            )
        })
        .collect()
}

fn random_indices(count: usize, seed: u64) -> Vec<u32> {
    let mut state = seed | 1;
    (0..count)
        .map(|_| rng(&mut state) as u32 % CHUNK_VOLUME as u32)
        .collect()
}

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("voxel-bench-{name}-{}-{nanos}", std::process::id()))
}

fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total += dir_bytes(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}
