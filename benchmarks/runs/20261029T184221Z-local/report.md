# Voxel benchmark report

`voxel-bench 0.1.0` on linux / x86_64 (Intel(R) Core(TM) i7-10700 CPU @ 2.90GHz). Measured heap bytes come from the counting allocator; RSS is process-wide.

## Process

| Metric | Value |
|---|---:|
| Wall time | 7480.77 ms |
| CPU time | 7218.31 ms |
| CPU utilization | 96.5% |
| RSS at start | 3.12 MiB |
| RSS at end | 92.19 MiB |
| Peak RSS | 139.44 MiB |

## Memory

| Case | Resident | Peak | Allocated | Allocs | Note |
|---|---:|---:|---:|---:|---|
| `registry/1024` | 372.2 KiB | 581.8 KiB | 702.7 KiB | 12285 | 372.2 B/block; hot 8.0 KiB; sets 64 (768 B); 64 textures |
| `registry/65536` | 23.00 MiB | 36.49 MiB | 43.17 MiB | 786429 | 367.9 B/block; hot 512.0 KiB; sets 64 (768 B); 64 textures |
| `chunk/solid` | 1.4 KiB | 618.4 KiB | 922.3 KiB | 99 | 54 words = 216 B; 2427.3x smaller than dense 512.0 KiB; root 45 |
| `chunk/terrain` | 260.1 KiB | 927.4 KiB | 2.68 MiB | 2157 | 16843 words = 65.8 KiB; 7.8x smaller than dense 512.0 KiB; root 16834 |
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
| `registry/build_freeze/1024` | 1.38 ms | 727.10 ops/s | 97.8% | 12285.00 | 702.7 KiB | 581.8 KiB | register + freeze; includes key and texture resolution |
| `registry/build_freeze/65536` | 71.28 ms | 14.03 ops/s | 100.0% | 786429.00 | 43.17 MiB | 36.49 MiB | register + freeze; includes key and texture resolution |
| `lookup/hot/random` | 1.2 ns | 811.23 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, uniformly random ids |
| `lookup/hot/coherent` | 1.5 ns | 682.60 Mops/s | 93.8% | 0.00 | 0 B | 0 B | 8-byte hot record, runs of 16 ids |
| `lookup/resolved/random` | 9.6 ns | 104.15 Mops/s | 99.7% | 0.00 | 0 B | 0 B | assembled BlockRecord; one hot load + set lookup + template hash |
| `lookup/key/random` | 28.1 ns | 35.54 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap<NamespacedKey, MaterialId> |
| `palette/resolve_4096` | 99.48 µs | 10.05 Kops/s | 99.8% | 1.00 | 8.0 KiB | 8.0 KiB | key -> id for 4 096 entries; allocates the id vector |
| `dag/build/solid` | 1.78 ms | 561.73 ops/s | 100.0% | 98.00 | 922.0 KiB | 618.4 KiB | 64^3 dense -> SVDAG |
| `dag/build/terrain` | 2.09 ms | 479.46 ops/s | 100.0% | 2156.19 | 2.68 MiB | 927.4 KiB | 64^3 dense -> SVDAG |
| `dag/build/noise30%` | 7.04 ms | 142.05 ops/s | 100.0% | 10161.00 | 9.73 MiB | 3.79 MiB | 64^3 dense -> SVDAG |
| `dag/get/random` | 33.0 ns | 30.27 Mops/s | 100.0% | 0.00 | 0 B | 0 B | six-level octree descent, random voxel |
| `dag/apply_edits/1` | 2.10 ms | 476.64 ops/s | 100.0% | 875.75 | 2.50 MiB | 888.7 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/100` | 2.11 ms | 473.65 ops/s | 100.0% | 887.31 | 2.34 MiB | 930.2 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/1000` | 2.80 ms | 356.77 ops/s | 100.0% | 1419.50 | 3.64 MiB | 1.63 MiB | copy-on-write rebuild of the edited paths |
| `overlay/set` | 7.5 ns | 133.34 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap insert |
| `overlay/get` | 5.4 ns | 185.83 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 4 096-entry overlay, random hit |
| `gpu/set_chunk` | 196.35 µs | 5.09 Kops/s | 100.0% | 59.00 | 580.2 KiB | 318.1 KiB | fresh GpuStore per op; import + table insert |
| `gpu/lookup` | 28.8 ns | 34.75 Mops/s | 100.0% | 0.00 | 0 B | 0 B | hash-table probe + octree descent |
| `gpu/lookup_overlay` | 11.8 ns | 84.97 Mops/s | 100.0% | 0.00 | 12 B | 768.0 KiB | probe + block table + presence bitmap hit |
| `gpu/overlay_set/100` | 1.47 µs | 680.29 Kops/s | 100.0% | 0.00 | 0 B | 0 B | 100 overlay writes in one 16^3 block |
| `gpu/apply_edits/100` | 32.53 µs | 30.74 Kops/s | 100.0% | 1.00 | 800 B | 800 B | in-place COW edit + immediate reclaim, 100 paths |
| `gpu/edit+flush/64_loaded` | 1.07 µs | 938.59 Kops/s | 100.0% | 6.65 | 475 B | 320 B | one edit, coalesced dirty range, in-memory sink |
| `gpu/overlay_flush/64_loaded` | 1.13 µs | 883.83 Kops/s | 100.0% | 0.00 | 0 B | 0 B | one overlay write per chunk, dirty ranges folded, in-memory sink |
| `world/tick_idle` | 6.2 ns | 160.17 Mops/s | 100.0% | 0.00 | 0 B | 0 B | policy check with no pending overlays |
| `render/hot_words/65536` | 19.08 µs | 52.41 Kops/s | 100.0% | 1.00 | 512.0 KiB | 512.0 KiB | pack the whole hot table for GPU upload |
| `render/texture_set_words/65536` | 65.0 ns | 15.39 Mops/s | 100.0% | 1.00 | 768 B | 768 B | pack every distinct texture set for GPU upload |

## Latency

| Case | p50 | p95 | p99 | max | Note |
|---|---:|---:|---:|---:|---|
| `latency/hot_lookup` | 0.00 µs | 0.10 µs | 0.10 µs | 0.20 µs | random id, hot table |
| `latency/dag_get` | 0.10 µs | 0.10 µs | 0.20 µs | 0.70 µs | random voxel in a terrain DAG |
| `latency/gpu_edit_1` | 0.40 µs | 0.50 µs | 0.50 µs | 7.90 µs | one COW edit on a resident chunk; reclaim excluded |
| `latency/gpu_overlay_set` | 0.00 µs | 0.10 µs | 0.10 µs | 3.40 µs | one overlay write; block table + slot already resident |
| `latency/world_set` | 0.10 µs | 0.20 µs | 0.70 µs | 16.60 µs | overlay write through World::set, chunk resident |
| `latency/world_tick_idle` | 0.00 µs | 0.10 µs | 0.10 µs | 0.10 µs | policy check with no pending overlays |
| `latency/world_consolidate_1` | 2.29 ms | 2.90 ms | 3.57 ms | 5.35 ms | one edit + event fold; DAG rebuild and disk sync included |

## Other lenses

These software readings are one half of the picture. Run the rest with:

```sh
scripts/bench.sh          # all of the below, with graceful skips
cargo bench --workspace   # Criterion wall clock and throughput
cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches
perf stat -e cycles,instructions,cache-misses,branch-misses \
  target/release/voxel-bench cpu                  # hardware counters
```
