# Voxel benchmark report

`voxel-bench 0.1.0` on linux / x86_64 (AMD Ryzen 5 7600 6-Core Processor). Measured heap bytes come from the counting allocator; RSS is process-wide.

## Process

| Metric | Value |
|---|---:|
| Wall time | 3593.57 ms |
| CPU time | 3592.85 ms |
| CPU utilization | 100.0% |
| RSS at start | 2.85 MiB |
| RSS at end | 12.92 MiB |
| Peak RSS | 61.50 MiB |

## Memory

| Case | Resident | Peak | Allocated | Allocs | Note |
|---|---:|---:|---:|---:|---|
| `registry/1024` | 372.2 KiB | 581.8 KiB | 702.7 KiB | 12285 | 372.2 B/block; hot 8.0 KiB; sets 64 (768 B); 64 textures |
| `registry/65536` | 23.00 MiB | 36.49 MiB | 43.17 MiB | 786429 | 367.9 B/block; hot 512.0 KiB; sets 64 (768 B); 64 textures |
| `chunk/solid` | 288 B | 560 B | 964 B | 8 | 54 words = 216 B; 2427.3x smaller than dense 512.0 KiB; root 45 |
| `chunk/terrain` | 72.0 KiB | 174.0 KiB | 280.1 KiB | 23 | 16843 words = 65.8 KiB; 7.8x smaller than dense 512.0 KiB; root 16834 |
| `chunk/noise30%` | 512.0 KiB | 1.30 MiB | 2.06 MiB | 30 | 108930 words = 425.5 KiB; 1.2x smaller than dense 512.0 KiB; root 108921 |
| `chunk/sparse4k` | 32.0 KiB | 134.0 KiB | 200.1 KiB | 23 | 8190 words = 32.0 KiB; 16.0x smaller than dense 512.0 KiB; root 8181 |
| `world/16_chunks_gpu` | 3.33 MiB | 3.33 MiB | 6.65 MiB | 77 | 16 chunks -> 22129 unique nodes; pool 733.9 KiB + table 16.0 KiB (len; resident includes capacity slack, rc and dedup); 46.9 KiB/chunk; 1.4x word dedup |
| `world/64_chunks_gpu` | 13.18 MiB | 13.18 MiB | 26.34 MiB | 87 | 64 chunks -> 71989 unique nodes; pool 2.35 MiB + table 16.0 KiB (len; resident includes capacity slack, rc and dedup); 37.9 KiB/chunk; 1.7x word dedup |
| `world/256_chunks_gpu` | 26.31 MiB | 26.31 MiB | 52.60 MiB | 93 | 256 chunks -> 229105 unique nodes; pool 7.54 MiB + table 16.0 KiB (len; resident includes capacity slack, rc and dedup); 30.2 KiB/chunk; 2.1x word dedup |
| `gpu_tables/65536` | 512.8 KiB | 512.8 KiB | 512.8 KiB | 2 | 131072 hot words + 192 set words = 512.8 KiB; 8.0 B/block |
| `overlay/1024` | 18.0 KiB | 27.0 KiB | 36.1 KiB | 10 | 1024 entries; 18.0 B/edit; peak 27.0 KiB |
| `overlay/4096` | 72.0 KiB | 108.0 KiB | 144.2 KiB | 12 | 4096 entries; 18.0 B/edit; peak 108.0 KiB |
| `disk/64_chunks` | 7.5 KiB | 75.6 KiB | 4.06 MiB | 76 | 4.04 MiB on disk; 64.6 KiB/chunk; peak heap 75.6 KiB |

## CPU and throughput

| Case | ns/op | Rate | CPU % | Allocs/op | Bytes/op | Peak heap | Note |
|---|---:|---:|---:|---:|---:|---:|---|
| `registry/build_freeze/1024` | 467.75 µs | 2.14 Kops/s | 100.0% | 12285.00 | 702.7 KiB | 581.8 KiB | register + freeze; includes key and texture resolution |
| `registry/build_freeze/65536` | 51.86 ms | 19.28 ops/s | 100.0% | 786429.00 | 43.17 MiB | 36.49 MiB | register + freeze; includes key and texture resolution |
| `lookup/hot/random` | 0.7 ns | 1.35 Gops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, uniformly random ids |
| `lookup/hot/coherent` | 0.7 ns | 1.36 Gops/s | 100.0% | 0.00 | 0 B | 0 B | 8-byte hot record, runs of 16 ids |
| `lookup/resolved/random` | 6.2 ns | 161.63 Mops/s | 100.0% | 0.00 | 0 B | 0 B | assembled BlockRecord; one hot load + set lookup + template hash |
| `lookup/key/random` | 14.2 ns | 70.33 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap<NamespacedKey, MaterialId> |
| `palette/resolve_4096` | 55.32 µs | 18.08 Kops/s | 100.0% | 1.00 | 8.0 KiB | 8.0 KiB | key -> id for 4 096 entries; allocates the id vector |
| `dag/build/solid` | 54.57 µs | 18.32 Kops/s | 100.0% | 8.00 | 964 B | 560 B | 64^3 dense -> SVDAG |
| `dag/build/terrain` | 375.18 µs | 2.67 Kops/s | 100.0% | 23.00 | 280.1 KiB | 174.0 KiB | 64^3 dense -> SVDAG |
| `dag/build/noise30%` | 1.12 ms | 891.74 ops/s | 100.0% | 30.00 | 2.06 MiB | 1.30 MiB | 64^3 dense -> SVDAG |
| `dag/get/random` | 12.9 ns | 77.32 Mops/s | 100.0% | 0.00 | 0 B | 0 B | six-level octree descent, random voxel |
| `dag/apply_edits/1` | 104.98 µs | 9.53 Kops/s | 100.0% | 35.00 | 352.2 KiB | 194.1 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/100` | 115.44 µs | 8.66 Kops/s | 100.0% | 35.00 | 353.0 KiB | 194.8 KiB | copy-on-write rebuild of the edited paths |
| `dag/apply_edits/1000` | 195.87 µs | 5.11 Kops/s | 100.0% | 35.00 | 468.0 KiB | 237.8 KiB | copy-on-write rebuild of the edited paths |
| `overlay/set` | 4.8 ns | 208.81 Mops/s | 100.0% | 0.00 | 0 B | 0 B | HashMap insert |
| `overlay/get` | 4.4 ns | 227.61 Mops/s | 100.0% | 0.00 | 0 B | 0 B | 4 096-entry overlay, random hit |
| `gpu/set_chunk` | 131.33 µs | 7.61 Kops/s | 100.0% | 59.00 | 576.2 KiB | 314.1 KiB | fresh GpuStore per op; import + table insert |
| `gpu/lookup` | 20.1 ns | 49.64 Mops/s | 100.0% | 0.00 | 0 B | 0 B | hash-table probe + octree descent |
| `gpu/apply_edits/100` | 19.03 µs | 52.56 Kops/s | 100.0% | 1.00 | 800 B | 800 B | in-place COW edit + immediate reclaim, 100 paths |
| `gpu/edit+flush/64_loaded` | 633.7 ns | 1.58 Mops/s | 100.0% | 6.65 | 475 B | 320 B | one edit, coalesced dirty range, in-memory sink |
| `render/hot_words/65536` | 8.14 µs | 122.78 Kops/s | 100.0% | 1.00 | 512.0 KiB | 512.0 KiB | pack the whole hot table for GPU upload |
| `render/texture_set_words/65536` | 32.2 ns | 31.06 Mops/s | 100.0% | 1.00 | 768 B | 768 B | pack every distinct texture set for GPU upload |

## Latency

| Case | p50 | p95 | p99 | max | Note |
|---|---:|---:|---:|---:|---|
| `latency/hot_lookup` | 0.02 µs | 0.02 µs | 0.02 µs | 0.08 µs | random id, hot table |
| `latency/dag_get` | 0.04 µs | 0.05 µs | 0.06 µs | 0.12 µs | random voxel in a terrain DAG |
| `latency/gpu_edit_1` | 0.26 µs | 0.28 µs | 0.41 µs | 1.34 µs | one COW edit on a resident chunk; reclaim excluded |
| `latency/world_set` | 0.05 µs | 0.07 µs | 0.10 µs | 19.71 µs | overlay write through World::set, chunk resident |

## Other lenses

These software readings are one half of the picture. Run the rest with:

```sh
scripts/bench.sh          # all of the below, with graceful skips
cargo bench --workspace   # Criterion wall clock and throughput
cargo bench -p voxel-bench --features iai --bench iai   # instructions/cache/branches
perf stat -e cycles,instructions,cache-misses,branch-misses \
  target/release/voxel-bench cpu                  # hardware counters
```
