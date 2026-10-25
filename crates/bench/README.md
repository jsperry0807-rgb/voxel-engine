# voxel-bench

Four lenses over the same workloads, because one number cannot describe a
voxel engine:

| Lens | Answers | Output |
|---|---|---|
| **Memory** | How many bytes does a registry, chunk, world, overlay or disk record keep alive? What is the peak? | `target/bench/report.md`, `report.json` |
| **CPU** | How much user+system CPU time, wall time, CPU utilization and allocation churn does an operation cost? | same |
| **Latency** | What are p50/p95/p99/max for the interactive paths? | same |
| **Wall clock** | Statistically stable ns/op and throughput for hot loops (Criterion). | `target/criterion` |
| **Hardware** | Deterministic instructions, cache misses and branch misses (iai-callgrind over Valgrind). | `target/criterion` |
| **Hardware counters** | Cycles, instructions, cache misses, branch misses, page faults (perf). | `target/bench/perf.txt` |

## Quick start

```sh
# Memory + CPU + latency report (a few seconds)
cargo run --release -p voxel-bench -- all

# Individual lenses
cargo run --release -p voxel-bench -- memory
cargo run --release -p voxel-bench -- cpu --print
cargo run --release -p voxel-bench -- latency --out target/bench

# Criterion (storage, block hot/cold, render packing)
cargo bench --workspace

# Deterministic instruction/cache counts (needs Valgrind)
cargo bench -p voxel-bench --features iai --bench iai

# Real wgpu upload timings (needs an adapter)
VOXEL_GPU_BENCH=1 cargo bench -p voxel-bench --bench render

# Everything, with graceful skips; archives into benchmarks/ by default
scripts/bench.sh
LABEL=my-change scripts/bench.sh  # name the archived run
RECORD=0 scripts/bench.sh         # local report only
```

## What is measured

**Memory** uses a counting `GlobalAlloc` installed by the binary. `resident`
is the heap still alive after the case, `allocated` is total churn including
scratch, and `peak` is the high-water mark above the case's baseline. Process
RSS and peak RSS (`VmRSS`/`VmHWM`) are reported separately so allocator slack
and mapped pages are not hidden.

Cases:

- `registry/{1024,65536}` — authoring properties, keys and resolved tables.
- `chunk/{solid,terrain,noise,sparse}` — SVDAG pool bytes versus a 512 KiB
  dense chunk.
- `world/64_chunks_gpu` — pooled nodes, chunk table, bytes per chunk and word
  dedup ratio.
- `gpu_tables/65536` — hot records plus deduplicated texture-set words.
- `overlay/{1024,4096}` — hash-map bytes per pending edit.
- `disk/64_chunks` — region-file bytes per chunk.

**CPU** runs each operation for a calibrated ~20 ms, three rounds, and keeps
the fastest wall time. It reports process user+system CPU time (so CPU
utilization below 100% means the thread was waiting), allocations per
operation and bytes per operation.

**Latency** takes 2 000 individual samples after a warmup and reports the
percentiles, which is the honest way to check the 60 Hz edit requirement:
a mean of 100 µs is irrelevant if p99 is 20 ms.

## Caveats

- The counting allocator adds one atomic operation per allocation. Use
  Criterion, not the CPU lens, for sub-nanosecond comparisons.
- `allocated` counts allocator calls, not page faults; RSS is the process
  truth and moves in 4 KiB pages.
- CPU% can exceed 100% only if the workload spawns threads; the suite is
  single-threaded, so it stays below 100%.
- Criterion and perf are machine-dependent. iai-callgrind is deterministic
  and is the right regression gate in CI.
- GPU tests and benchmarks skip when no wgpu adapter is available.

## Adding a case

- Shared data goes in `src/patterns.rs` or `src/fixtures.rs`, so every lens
  sees the same workload.
- Add memory/CPU/latency cases in `src/suite.rs`; the report tables update
  automatically.
- Add statistical or hardware benchmarks in `benches/`; keep the benchmark
  body free of setup work.
