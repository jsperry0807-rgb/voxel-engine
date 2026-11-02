# Voxel benchmark report

`voxel-bench 0.1.0` on linux / x86_64 (AMD Ryzen 5 7600 6-Core Processor). Measured heap bytes come from the counting allocator; RSS is process-wide.

## Process

| Metric | Value |
|---|---:|
| Wall time | 10766.28 ms |
| CPU time | 10765.47 ms |
| CPU utilization | 100.0% |
| RSS at start | 3.40 MiB |
| RSS at end | 207.78 MiB |
| Peak RSS | 313.48 MiB |

## Memory

| Case | Resident | Peak | Allocated | Allocs | Note |
|---|---:|---:|---:|---:|---|
| `registry/1024` | 372.2 KiB | 581.8 KiB | 702.7 KiB | 12285 | 372.2 B/block; hot 8.0 KiB; sets 64 (768 B); 64 textures |
| `registry/65536` | 23.00 MiB | 36.49 MiB | 43.17 MiB | 786429 | 367.9 B/block; hot 512.0 KiB; sets 64 (768 B); 64 textures |
| `chunk/solid` | 1.7 KiB | 4.82 MiB | 7.15 MiB | 124 | 63 words = 252 B; 16644.1x smaller than dense 4.00 MiB; root 54 |
| `chunk/terrain` | 703.5 KiB | 4.50 MiB | 10.98 MiB | 6862 | 56667 words = 221.4 KiB; 18.5x smaller than dense 4.00 MiB; root 56662 |
| `chunk/noise30%` | 6.11 MiB | 26.98 MiB | 66.78 MiB | 55658 | 694169 words = 2.65 MiB; 1.5x smaller than dense 4.00 MiB; root 694160 |
| `chunk/sparse4k` | 155.1 KiB | 2.70 MiB | 4.39 MiB | 2794 | 9225 words = 36.0 KiB; 113.7x smaller than dense 4.00 MiB; root 9216 |
| `world/16_chunks_gpu` | 13.22 MiB | 13.22 MiB | 26.41 MiB | 86 | 16 chunks -> 72327 unique nodes; pool 2.36 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 152.5 KiB/chunk; 1.5x word dedup |
| `world/64_chunks_gpu` | 26.34 MiB | 26.34 MiB | 52.66 MiB | 92 | 64 chunks -> 229024 unique nodes; pool 7.53 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 120.8 KiB/chunk; 1.8x word dedup |
| `world/256_chunks_gpu` | 105.10 MiB | 105.10 MiB | 210.18 MiB | 102 | 256 chunks -> 731128 unique nodes; pool 24.09 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 96.4 KiB/chunk; 2.3x word dedup |
| `gpu_tables/65536` | 512.8 KiB | 512.8 KiB | 512.8 KiB | 2 | 131072 hot words + 192 set words = 512.8 KiB; 8.0 B/block |
| `overlay/1024` | 18.0 KiB | 27.0 KiB | 36.1 KiB | 10 | 1024 entries; 18.0 B/edit; peak 27.0 KiB |
| `overlay/4096` | 72.0 KiB | 108.0 KiB | 144.2 KiB | 12 | 4096 entries; 18.0 B/edit; peak 108.0 KiB |
| `gpu_overlay/1` | 942.8 KiB | 942.8 KiB | 1.81 MiB | 73 | 1 edited 16^3 block(s); 2176 overlay words (8.5 KiB/block) |
| `gpu_overlay/64` | 1006.4 KiB | 1006.4 KiB | 1.94 MiB | 85 | 64 edited 16^3 block(s); 17408 overlay words (1.1 KiB/block) |
| `disk/64_chunks` | 7.5 KiB | 231.8 KiB | 13.86 MiB | 76 | 13.84 MiB on disk; 221.4 KiB/chunk; peak heap 231.8 KiB |

## Layout cost (M1-18)

Internal bytes are the banded arena's 8 B words plus the padding that aligns full runs to 64 B. Dedup is the expanded node count divided by the unique node count; `old` is the superseded per-node `u32` pool and `new` is the banded run layout. `Old/new` above 1 means the banded serialization is smaller. `B/node` and every byte figure include padding.

| Fixture | Internal bytes | B/node incl. pad | Padding | Internal runs | Leaf runs | Shared runs | Dedup new | Dedup old | Old bytes | New bytes | Old/new |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `solid` | 392 B | 8.0 | 0 B (0.0%) | 6 | 1 | 6 | 6114.1x | 42799.0x | 260 B | 496 B | 0.52x |
| `terrain` | 387.6 KiB | 8.3 | 12.3 KiB (3.2%) | 6440 | 142 | 719 | 1.5x | 10.7x | 221.4 KiB | 389.6 KiB | 0.57x |
| `noise30%` | 2.28 MiB | 8.4 | 114.1 KiB (4.9%) | 37449 | 17858 | 6465 | 1.0x | 2.5x | 2.65 MiB | 2.45 MiB | 1.08x |
| `sparse4k` | 51.6 KiB | 8.0 | 248 B (0.5%) | 2515 | 21 | 562 | 1.7x | 4.3x | 36.0 KiB | 51.8 KiB | 0.70x |

### COW cost

A batch appends one internal run per touched level and one leaf run per touched level-1 parent. `Internal B/path` includes full-run padding; `Total B/path` also counts leaf materials.

| Fixture | Paths | Runs | Leaf runs | Internal B/path | Total B/path |
|---|---:|---:|---:|---:|---:|
| `solid` | 1 | 7 | 1 | 392.0 | 408.0 |
| `solid` | 100 | 353 | 100 | 225.4 | 241.4 |
| `terrain` | 1 | 7 | 1 | 88.0 | 90.0 |
| `terrain` | 100 | 353 | 100 | 83.4 | 87.6 |
| `noise30%` | 1 | 7 | 1 | 392.0 | 394.0 |
| `noise30%` | 100 | 353 | 100 | 225.2 | 231.2 |
| `sparse4k` | 1 | 7 | 1 | 264.0 | 266.0 |
| `sparse4k` | 100 | 353 | 100 | 101.5 | 103.5 |

## CPU and throughput

| Case | ns/op | Rate | CPU % | Allocs/op | Bytes/op | Peak heap | Note |
|---|---:|---:|---:|---:|---:|---:|---|
| `registry/build_freeze/1024` | 486.54 µs | 2.06 Kops/s | 99.8% | 12285.00 | 702.7 KiB | 581.8 KiB | register + freeze; includes key and texture resolution |
| `registry/build_freeze/65536` | 37.44 ms | 26.71 ops/s | 100.0% | 786429.00 | 43.17 MiB | 36.49 MiB | register + freeze; includes key and texture resolution |
| `lookup/hot/random` | 0.8 ns | 1.32 Gops/s | 100.1% | 0.00 | 0 B | 0 B | 8-byte hot record, uniformly random ids |
| `lookup/hot/coherent` | 0.7 ns | 1.35 Gops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, runs of 16 ids |
| `lookup/resolved/random` | 6.4 ns | 156.43 Mops/s | 100.0% | 0.00 | 0 B | 0 B | assembled BlockRecord; one hot load + set lookup + template hash |
| `lookup/key/random` | 14.4 ns | 69.33 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap<NamespacedKey, MaterialId> |
| `palette/resolve_4096` | 56.53 µs | 17.69 Kops/s | 100.0% | 1.00 | 8.0 KiB | 8.0 KiB | key -> id for 4 096 entries; allocates the id vector |
| `dag/build/solid` | 11.03 ms | 90.62 ops/s | 100.0% | 122.00 | 7.15 MiB | 4.82 MiB | 64^3 dense -> SVDAG |
| `dag/build/terrain` | 5.99 ms | 166.90 ops/s | 100.0% | 6861.50 | 10.98 MiB | 4.50 MiB | 64^3 dense -> SVDAG |
| `dag/build/noise30%` | 39.02 ms | 25.63 ops/s | 100.0% | 55659.00 | 66.78 MiB | 26.98 MiB | 64^3 dense -> SVDAG |
| `dag/get/random` | 20.6 ns | 48.47 Mops/s | 100.0% | 0.00 | 0 B | 0 B | six-level octree descent, random voxel |
| `dag/apply_edits/1` | 2.95 ms | 339.39 ops/s | 100.0% | 1186.00 | 5.39 MiB | 3.38 MiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/100` | 3.00 ms | 333.33 ops/s | 100.0% | 1228.75 | 5.31 MiB | 3.38 MiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/1000` | 3.51 ms | 285.28 ops/s | 100.0% | 1712.38 | 5.82 MiB | 3.40 MiB | copy-on-write rebuild of the edited paths |
| `overlay/set` | 6.9 ns | 145.05 Mops/s | 100.0% | 0.00 | 36 B | 108.0 KiB | HashMap insert |
| `overlay/get` | 4.4 ns | 227.59 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 4 096-entry overlay, random hit |
| `gpu/set_chunk` | 444.34 µs | 2.25 Kops/s | 100.0% | 67.00 | 1.80 MiB | 932.1 KiB | fresh GpuStore per op; import + table insert |
| `gpu/lookup` | 18.6 ns | 53.90 Mops/s | 100.0% | 0.00 | 0 B | 0 B | hash-table probe + octree descent |
| `gpu/lookup_overlay` | 24.7 ns | 40.53 Mops/s | 100.0% | 0.00 | 12 B | 768.0 KiB | probe + block table + presence bitmap hit |
| `gpu/overlay_set/100` | 1.07 µs | 935.41 Kops/s | 100.0% | 0.00 | 0 B | 0 B | 100 overlay writes in one 16^3 block |
| `gpu/apply_edits/100` | 27.23 µs | 36.72 Kops/s | 100.0% | 1.00 | 928 B | 64.8 KiB | in-place COW edit + immediate reclaim, 100 paths |
| `gpu/edit+flush/64_loaded` | 743.8 ns | 1.34 Mops/s | 100.0% | 6.82 | 497 B | 384 B | one edit, coalesced dirty range, in-memory sink |
| `gpu/overlay_flush/64_loaded` | 660.5 ns | 1.51 Mops/s | 100.0% | 0.00 | 0 B | 0 B | one overlay write per chunk, dirty ranges folded, in-memory sink |
| `world/tick_idle` | 1.5 ns | 648.22 Mops/s | 100.0% | 0.00 | 0 B | 0 B | policy check with no pending overlays |
| `render/hot_words/65536` | 8.38 µs | 119.29 Kops/s | 100.0% | 1.00 | 512.0 KiB | 512.0 KiB | pack the whole hot table for GPU upload |
| `render/texture_set_words/65536` | 34.5 ns | 29.00 Mops/s | 100.0% | 1.00 | 768 B | 768 B | pack every distinct texture set for GPU upload |

## Latency

| Case | p50 | p95 | p99 | max | Note |
|---|---:|---:|---:|---:|---|
| `latency/hot_lookup` | 0.02 µs | 0.02 µs | 0.02 µs | 0.04 µs | random id, hot table |
| `latency/dag_get` | 0.03 µs | 0.08 µs | 0.12 µs | 0.21 µs | random voxel in a terrain DAG |
| `latency/gpu_edit_1` | 0.32 µs | 0.54 µs | 0.60 µs | 20.36 µs | one COW edit on a resident chunk; reclaim excluded |
| `latency/gpu_overlay_set` | 0.02 µs | 0.03 µs | 0.05 µs | 10.57 µs | one overlay write; block table + slot already resident |
| `latency/world_set` | 0.10 µs | 0.55 µs | 0.80 µs | 101.30 µs | overlay write through World::set, chunk resident |
| `latency/world_tick_idle` | 0.02 µs | 0.02 µs | 0.02 µs | 0.04 µs | policy check with no pending overlays |
| `latency/world_consolidate_1` | 3.49 ms | 4.36 ms | 5.10 ms | 5.50 ms | one edit + event fold; DAG rebuild and disk sync included |

## Other lenses

These software readings are one half of the picture. Run the rest with:

```sh
scripts/bench.sh          # all of the below, with graceful skips
cargo bench --workspace   # Criterion wall clock and throughput
cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches
perf stat -e cycles,instructions,cache-misses,branch-misses \
  target/release/voxel-bench cpu                  # hardware counters
```
