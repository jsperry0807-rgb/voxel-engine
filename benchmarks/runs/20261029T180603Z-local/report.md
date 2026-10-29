# Voxel benchmark report

`voxel-bench 0.1.0` on linux / x86_64 (Intel(R) Core(TM) i7-10700 CPU @ 2.90GHz). Measured heap bytes come from the counting allocator; RSS is process-wide.

## Process

| Metric | Value |
|---|---:|
| Wall time | 6956.41 ms |
| CPU time | 6953.23 ms |
| CPU utilization | 100.0% |
| RSS at start | 3.12 MiB |
| RSS at end | 93.25 MiB |
| Peak RSS | 136.29 MiB |

## Memory

| Case | Resident | Peak | Allocated | Allocs | Note |
|---|---:|---:|---:|---:|---|
| `registry/1024` | 372.2 KiB | 581.8 KiB | 702.7 KiB | 12285 | 372.2 B/block; hot 8.0 KiB; sets 64 (768 B); 64 textures |
| `registry/65536` | 23.00 MiB | 36.49 MiB | 43.17 MiB | 786429 | 367.9 B/block; hot 512.0 KiB; sets 64 (768 B); 64 textures |
| `chunk/solid` | 1.3 KiB | 618.4 KiB | 922.0 KiB | 98 | 54 words = 216 B; 2427.3x smaller than dense 512.0 KiB; root 45 |
| `chunk/terrain` | 260.0 KiB | 927.4 KiB | 2.68 MiB | 2156 | 16843 words = 65.8 KiB; 7.8x smaller than dense 512.0 KiB; root 16834 |
| `chunk/noise30%` | 942.0 KiB | 3.79 MiB | 9.73 MiB | 10161 | 108930 words = 425.5 KiB; 1.2x smaller than dense 512.0 KiB; root 108921 |
| `chunk/sparse4k` | 118.1 KiB | 691.0 KiB | 1.89 MiB | 2136 | 8190 words = 32.0 KiB; 16.0x smaller than dense 512.0 KiB; root 8181 |
| `world/16_chunks_gpu` | 3.34 MiB | 3.34 MiB | 6.65 MiB | 77 | 16 chunks -> 22129 unique nodes; pool 733.9 KiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 47.1 KiB/chunk; 1.4x word dedup |
| `world/64_chunks_gpu` | 13.18 MiB | 13.18 MiB | 26.34 MiB | 87 | 64 chunks -> 71989 unique nodes; pool 2.35 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 37.9 KiB/chunk; 1.7x word dedup |
| `world/256_chunks_gpu` | 26.31 MiB | 26.31 MiB | 52.61 MiB | 93 | 256 chunks -> 229105 unique nodes; pool 7.54 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 30.2 KiB/chunk; 2.1x word dedup |
| `gpu_tables/65536` | 512.8 KiB | 512.8 KiB | 512.8 KiB | 2 | 131072 hot words + 192 set words = 512.8 KiB; 8.0 B/block |
| `overlay/1024` | 18.0 KiB | 27.0 KiB | 36.1 KiB | 10 | 1024 entries; 18.0 B/edit; peak 27.0 KiB |
| `overlay/4096` | 72.0 KiB | 108.0 KiB | 144.2 KiB | 12 | 4096 entries; 18.0 B/edit; peak 108.0 KiB |
| `gpu_overlay/1` | 309.0 KiB | 318.1 KiB | 589.1 KiB | 65 | 1 edited 16^3 block(s); 2176 overlay words (8.5 KiB/block) |
| `gpu_overlay/64` | 850.6 KiB | 850.6 KiB | 1.63 MiB | 86 | 64 edited 16^3 block(s); 139264 overlay words (8.5 KiB/block) |
| `disk/64_chunks` | 7.5 KiB | 75.6 KiB | 4.06 MiB | 76 | 4.04 MiB on disk; 64.6 KiB/chunk; peak heap 75.6 KiB |

## Layout cost (M1-18)

Internal bytes are the banded arena's 8 B words plus the padding that aligns full runs to 64 B. Dedup is the expanded node count divided by the unique node count; `old` is the superseded per-node `u32` pool and `new` is the banded run layout. `Old/new` above 1 means the banded serialization is smaller. `B/node` and every byte figure include padding.

| Fixture | Internal bytes | B/node incl. pad | Padding | Internal runs | Leaf runs | Shared runs | Dedup new | Dedup old | Old bytes | New bytes | Old/new |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `solid` | 328 B | 8.0 | 0 B (0.0%) | 5 | 1 | 5 | 913.4x | 6241.5x | 224 B | 424 B | 0.53x |
| `terrain` | 108.1 KiB | 8.2 | 3.1 KiB (2.9%) | 1813 | 119 | 231 | 1.4x | 9.1x | 65.8 KiB | 109.8 KiB | 0.60x |
| `noise30%` | 292.4 KiB | 8.4 | 14.5 KiB (5.0%) | 4681 | 5220 | 1840 | 1.0x | 1.8x | 425.5 KiB | 336.5 KiB | 1.26x |
| `sparse4k` | 45.4 KiB | 8.1 | 416 B (0.9%) | 1888 | 32 | 311 | 1.2x | 3.4x | 32.0 KiB | 45.6 KiB | 0.70x |

### COW cost

A batch appends one internal run per touched level and one leaf run per touched level-1 parent. `Internal B/path` includes full-run padding; `Total B/path` also counts leaf materials.

| Fixture | Paths | Runs | Leaf runs | Internal B/path | Total B/path |
|---|---:|---:|---:|---:|---:|
| `solid` | 1 | 6 | 1 | 328.0 | 344.0 |
| `solid` | 100 | 249 | 100 | 158.8 | 174.8 |
| `terrain` | 1 | 6 | 1 | 352.0 | 366.0 |
| `terrain` | 100 | 249 | 100 | 99.1 | 106.8 |
| `noise30%` | 1 | 6 | 1 | 328.0 | 332.0 |
| `noise30%` | 100 | 249 | 100 | 158.8 | 164.7 |
| `sparse4k` | 1 | 6 | 1 | 256.0 | 258.0 |
| `sparse4k` | 100 | 249 | 100 | 91.0 | 93.3 |

## CPU and throughput

| Case | ns/op | Rate | CPU % | Allocs/op | Bytes/op | Peak heap | Note |
|---|---:|---:|---:|---:|---:|---:|---|
| `registry/build_freeze/1024` | 903.29 µs | 1.11 Kops/s | 100.0% | 12285.00 | 702.7 KiB | 581.8 KiB | register + freeze; includes key and texture resolution |
| `registry/build_freeze/65536` | 64.18 ms | 15.58 ops/s | 100.0% | 786429.00 | 43.17 MiB | 36.49 MiB | register + freeze; includes key and texture resolution |
| `lookup/hot/random` | 1.2 ns | 820.82 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, uniformly random ids |
| `lookup/hot/coherent` | 1.2 ns | 824.50 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, runs of 16 ids |
| `lookup/resolved/random` | 9.0 ns | 111.13 Mops/s | 100.0% | 0.00 | 0 B | 0 B | assembled BlockRecord; one hot load + set lookup + template hash |
| `lookup/key/random` | 26.3 ns | 37.98 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap<NamespacedKey, MaterialId> |
| `palette/resolve_4096` | 98.46 µs | 10.16 Kops/s | 100.0% | 1.00 | 8.0 KiB | 8.0 KiB | key -> id for 4 096 entries; allocates the id vector |
| `dag/build/solid` | 1.77 ms | 563.83 ops/s | 100.0% | 98.00 | 922.0 KiB | 618.4 KiB | 64^3 dense -> SVDAG |
| `dag/build/terrain` | 2.05 ms | 488.61 ops/s | 100.0% | 2155.00 | 2.68 MiB | 927.4 KiB | 64^3 dense -> SVDAG |
| `dag/build/noise30%` | 6.09 ms | 164.26 ops/s | 100.0% | 10161.00 | 9.73 MiB | 3.79 MiB | 64^3 dense -> SVDAG |
| `dag/get/random` | 32.2 ns | 31.03 Mops/s | 100.0% | 0.00 | 0 B | 0 B | six-level octree descent, random voxel |
| `dag/apply_edits/1` | 2.09 ms | 478.37 ops/s | 100.0% | 875.75 | 2.50 MiB | 888.2 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/100` | 2.12 ms | 471.69 ops/s | 100.0% | 887.50 | 2.34 MiB | 930.2 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/1000` | 2.78 ms | 359.08 ops/s | 100.0% | 1420.75 | 3.65 MiB | 1.64 MiB | copy-on-write rebuild of the edited paths |
| `overlay/set` | 7.4 ns | 135.42 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap insert |
| `overlay/get` | 5.3 ns | 187.06 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 4 096-entry overlay, random hit |
| `gpu/set_chunk` | 196.36 µs | 5.09 Kops/s | 100.0% | 59.00 | 580.2 KiB | 318.1 KiB | fresh GpuStore per op; import + table insert |
| `gpu/lookup` | 26.8 ns | 37.35 Mops/s | 100.0% | 0.00 | 0 B | 0 B | hash-table probe + octree descent |
| `gpu/lookup_overlay` | 11.7 ns | 85.19 Mops/s | 100.0% | 0.00 | 12 B | 768.0 KiB | probe + block table + presence bitmap hit |
| `gpu/overlay_set/100` | 1.45 µs | 690.84 Kops/s | 100.0% | 0.00 | 0 B | 0 B | 100 overlay writes in one 16^3 block |
| `gpu/apply_edits/100` | 30.71 µs | 32.56 Kops/s | 100.0% | 1.00 | 800 B | 800 B | in-place COW edit + immediate reclaim, 100 paths |
| `gpu/edit+flush/64_loaded` | 1.02 µs | 981.26 Kops/s | 100.0% | 6.65 | 475 B | 320 B | one edit, coalesced dirty range, in-memory sink |
| `gpu/overlay_flush/64_loaded` | 1.04 µs | 958.04 Kops/s | 100.0% | 0.00 | 0 B | 0 B | one overlay write per chunk, dirty ranges folded, in-memory sink |
| `world/tick_idle` | 6.2 ns | 162.01 Mops/s | 100.0% | 0.00 | 0 B | 0 B | policy check with no pending overlays |
| `render/hot_words/65536` | 19.34 µs | 51.70 Kops/s | 100.0% | 1.00 | 512.0 KiB | 512.0 KiB | pack the whole hot table for GPU upload |
| `render/texture_set_words/65536` | 63.2 ns | 15.83 Mops/s | 100.0% | 1.00 | 768 B | 768 B | pack every distinct texture set for GPU upload |

## Latency

| Case | p50 | p95 | p99 | max | Note |
|---|---:|---:|---:|---:|---|
| `latency/hot_lookup` | 0.00 µs | 0.10 µs | 0.10 µs | 0.10 µs | random id, hot table |
| `latency/dag_get` | 0.10 µs | 0.10 µs | 0.10 µs | 0.40 µs | random voxel in a terrain DAG |
| `latency/gpu_edit_1` | 0.40 µs | 0.50 µs | 0.50 µs | 8.30 µs | one COW edit on a resident chunk; reclaim excluded |
| `latency/gpu_overlay_set` | 0.00 µs | 0.10 µs | 0.10 µs | 12.80 µs | one overlay write; block table + slot already resident |
| `latency/world_set` | 0.10 µs | 0.20 µs | 0.60 µs | 15.10 µs | overlay write through World::set, chunk resident |
| `latency/world_tick_idle` | 0.00 µs | 0.10 µs | 0.10 µs | 0.10 µs | policy check with no pending overlays |
| `latency/world_consolidate_1` | 2.19 ms | 2.45 ms | 2.99 ms | 3.32 ms | one edit + event fold; DAG rebuild and disk sync included |

## Other lenses

These software readings are one half of the picture. Run the rest with:

```sh
scripts/bench.sh          # all of the below, with graceful skips
cargo bench --workspace   # Criterion wall clock and throughput
cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches
perf stat -e cycles,instructions,cache-misses,branch-misses \
  target/release/voxel-bench cpu                  # hardware counters
```
