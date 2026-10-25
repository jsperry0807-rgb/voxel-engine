# Voxel benchmark report

`voxel-bench 0.1.0` on linux / x86_64 (AMD Ryzen 5 7600 6-Core Processor). Measured heap bytes come from the counting allocator; RSS is process-wide.

## Process

| Metric | Value |
|---|---:|
| Wall time | 4077.87 ms |
| CPU time | 4077.78 ms |
| CPU utilization | 100.0% |
| RSS at start | 3.15 MiB |
| RSS at end | 43.85 MiB |
| Peak RSS | 91.17 MiB |

## Memory

| Case | Resident | Peak | Allocated | Allocs | Note |
|---|---:|---:|---:|---:|---|
| `registry/1024` | 372.2 KiB | 581.8 KiB | 702.7 KiB | 12285 | 372.2 B/block; hot 8.0 KiB; sets 64 (768 B); 64 textures |
| `registry/65536` | 23.00 MiB | 36.49 MiB | 43.17 MiB | 786429 | 367.9 B/block; hot 512.0 KiB; sets 64 (768 B); 64 textures |
| `chunk/solid` | 288 B | 560 B | 964 B | 8 | 54 words = 216 B; 2427.3x smaller than dense 512.0 KiB; root 45 |
| `chunk/terrain` | 72.0 KiB | 174.0 KiB | 280.1 KiB | 23 | 16843 words = 65.8 KiB; 7.8x smaller than dense 512.0 KiB; root 16834 |
| `chunk/noise30%` | 512.0 KiB | 1.30 MiB | 2.06 MiB | 30 | 108930 words = 425.5 KiB; 1.2x smaller than dense 512.0 KiB; root 108921 |
| `chunk/sparse4k` | 32.0 KiB | 134.0 KiB | 200.1 KiB | 23 | 8190 words = 32.0 KiB; 16.0x smaller than dense 512.0 KiB; root 8181 |
| `world/16_chunks_gpu` | 3.34 MiB | 3.34 MiB | 6.65 MiB | 77 | 16 chunks -> 22129 unique nodes; pool 733.9 KiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 47.1 KiB/chunk; 1.4x word dedup |
| `world/64_chunks_gpu` | 13.18 MiB | 13.18 MiB | 26.34 MiB | 87 | 64 chunks -> 71989 unique nodes; pool 2.35 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 37.9 KiB/chunk; 1.7x word dedup |
| `world/256_chunks_gpu` | 26.31 MiB | 26.31 MiB | 52.61 MiB | 93 | 256 chunks -> 229105 unique nodes; pool 7.54 MiB + table 20.0 KiB (len; resident includes capacity slack, rc and dedup); 30.2 KiB/chunk; 2.1x word dedup |
| `gpu_tables/65536` | 512.8 KiB | 512.8 KiB | 512.8 KiB | 2 | 131072 hot words + 192 set words = 512.8 KiB; 8.0 B/block |
| `overlay/1024` | 18.0 KiB | 27.0 KiB | 36.1 KiB | 10 | 1024 entries; 18.0 B/edit; peak 27.0 KiB |
| `overlay/4096` | 72.0 KiB | 108.0 KiB | 144.2 KiB | 12 | 4096 entries; 18.0 B/edit; peak 108.0 KiB |
| `gpu_overlay/1` | 309.0 KiB | 318.1 KiB | 589.1 KiB | 65 | 1 edited 16^3 block(s); 2176 overlay words (8.5 KiB/block) |
| `gpu_overlay/64` | 850.6 KiB | 850.6 KiB | 1.63 MiB | 86 | 64 edited 16^3 block(s); 139264 overlay words (8.5 KiB/block) |
| `disk/64_chunks` | 7.5 KiB | 75.6 KiB | 4.06 MiB | 76 | 4.04 MiB on disk; 64.6 KiB/chunk; peak heap 75.6 KiB |

## CPU and throughput

| Case | ns/op | Rate | CPU % | Allocs/op | Bytes/op | Peak heap | Note |
|---|---:|---:|---:|---:|---:|---:|---|
| `registry/build_freeze/1024` | 458.64 µs | 2.18 Kops/s | 100.0% | 12285.00 | 702.7 KiB | 581.8 KiB | register + freeze; includes key and texture resolution |
| `registry/build_freeze/65536` | 49.02 ms | 20.40 ops/s | 100.0% | 786429.00 | 43.17 MiB | 36.49 MiB | register + freeze; includes key and texture resolution |
| `lookup/hot/random` | 0.7 ns | 1.41 Gops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, uniformly random ids |
| `lookup/hot/coherent` | 0.7 ns | 1.42 Gops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, runs of 16 ids |
| `lookup/resolved/random` | 6.1 ns | 164.03 Mops/s | 100.0% | 0.00 | 0 B | 0 B | assembled BlockRecord; one hot load + set lookup + template hash |
| `lookup/key/random` | 14.0 ns | 71.47 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap<NamespacedKey, MaterialId> |
| `palette/resolve_4096` | 55.95 µs | 17.87 Kops/s | 100.0% | 1.00 | 8.0 KiB | 8.0 KiB | key -> id for 4 096 entries; allocates the id vector |
| `dag/build/solid` | 53.64 µs | 18.64 Kops/s | 100.0% | 8.00 | 964 B | 560 B | 64^3 dense -> SVDAG |
| `dag/build/terrain` | 356.31 µs | 2.81 Kops/s | 100.0% | 23.00 | 280.1 KiB | 174.0 KiB | 64^3 dense -> SVDAG |
| `dag/build/noise30%` | 1.10 ms | 905.79 ops/s | 100.0% | 30.00 | 2.06 MiB | 1.30 MiB | 64^3 dense -> SVDAG |
| `dag/get/random` | 12.8 ns | 78.12 Mops/s | 100.0% | 0.00 | 0 B | 0 B | six-level octree descent, random voxel |
| `dag/apply_edits/1` | 107.71 µs | 9.28 Kops/s | 100.0% | 35.00 | 352.2 KiB | 194.1 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/100` | 118.59 µs | 8.43 Kops/s | 100.0% | 35.00 | 353.0 KiB | 194.8 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/1000` | 188.74 µs | 5.30 Kops/s | 100.0% | 35.00 | 468.0 KiB | 237.8 KiB | copy-on-write rebuild of the edited paths |
| `overlay/set` | 4.9 ns | 204.72 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap insert |
| `overlay/get` | 4.4 ns | 229.02 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 4 096-entry overlay, random hit |
| `gpu/set_chunk` | 128.53 µs | 7.78 Kops/s | 100.0% | 59.00 | 580.2 KiB | 318.1 KiB | fresh GpuStore per op; import + table insert |
| `gpu/lookup` | 20.5 ns | 48.76 Mops/s | 100.0% | 0.00 | 0 B | 0 B | hash-table probe + octree descent |
| `gpu/lookup_overlay` | 8.1 ns | 122.88 Mops/s | 100.0% | 0.00 | 12 B | 768.0 KiB | probe + block table + presence bitmap hit |
| `gpu/overlay_set/100` | 1.07 µs | 933.66 Kops/s | 100.0% | 0.00 | 0 B | 0 B | 100 overlay writes in one 16^3 block |
| `gpu/apply_edits/100` | 19.39 µs | 51.59 Kops/s | 100.0% | 1.00 | 32.9 KiB | 16.06 MiB | in-place COW edit + immediate reclaim, 100 paths |
| `gpu/edit+flush/64_loaded` | 646.8 ns | 1.55 Mops/s | 100.0% | 6.65 | 475 B | 2.5 KiB | one edit, coalesced dirty range, in-memory sink |
| `gpu/overlay_flush/64_loaded` | 648.5 ns | 1.54 Mops/s | 100.0% | 0.00 | 0 B | 0 B | one overlay write per chunk, dirty ranges folded, in-memory sink |
| `render/hot_words/65536` | 8.06 µs | 124.10 Kops/s | 100.0% | 1.00 | 512.0 KiB | 512.0 KiB | pack the whole hot table for GPU upload |
| `render/texture_set_words/65536` | 31.4 ns | 31.80 Mops/s | 100.0% | 1.00 | 768 B | 768 B | pack every distinct texture set for GPU upload |

## Latency

| Case | p50 | p95 | p99 | max | Note |
|---|---:|---:|---:|---:|---|
| `latency/hot_lookup` | 0.02 µs | 0.02 µs | 0.03 µs | 0.03 µs | random id, hot table |
| `latency/dag_get` | 0.04 µs | 0.05 µs | 0.06 µs | 7.78 µs | random voxel in a terrain DAG |
| `latency/gpu_edit_1` | 0.27 µs | 0.37 µs | 0.47 µs | 1.79 µs | one COW edit on a resident chunk; reclaim excluded |
| `latency/gpu_overlay_set` | 0.03 µs | 0.03 µs | 0.04 µs | 1.69 µs | one overlay write; block table + slot already resident |
| `latency/world_set` | 0.06 µs | 0.08 µs | 0.16 µs | 27.01 µs | overlay write through World::set, chunk resident |

## Other lenses

These software readings are one half of the picture. Run the rest with:

```sh
scripts/bench.sh          # all of the below, with graceful skips
cargo bench --workspace   # Criterion wall clock and throughput
cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches
perf stat -e cycles,instructions,cache-misses,branch-misses \
  target/release/voxel-bench cpu                  # hardware counters
```
