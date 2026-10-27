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

# Inspect a save directory without modifying it (JSON on stdout)
cargo run --release -p voxel-bench -- save-scan --path saves/world
cargo run --release -p voxel-bench -- save-scan --path saves/world --out target/save.json

# Verify the frozen fixture save (hash, scan, rebuild from manifest)
cargo run -p voxel-bench -- fixture verify

# Per-chunk memory distribution over the fixture (M1-35)
cargo run -p voxel-bench -- gate memory
cargo run -p voxel-bench -- gate memory --json
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
- `gpu_overlay/{1,64}` — GPU block-overlay slots (8.5 KiB per edited 16³ block).
- `disk/64_chunks` — region-file bytes per chunk.

**Layout cost** (`layout/...`, M1-18) reports the banded layout against the
superseded v1 `u32` pool: internal bytes per node including full-run padding,
internal/leaf run counts, dedup ratios, old/new serialized size, and COW bytes
per edited path for 1- and 100-edit batches. It runs as part of `all` and can
be run alone with `cargo run --release -p voxel-bench -- layout --print`.

**CPU** runs each operation for a calibrated ~20 ms, three rounds, and keeps
the fastest wall time. It reports process user+system CPU time (so CPU
utilization below 100% means the thread was waiting), allocations per
operation and bytes per operation.

**Latency** takes 2 000 individual samples after a warmup and reports the
percentiles, which is the honest way to check the 60 Hz edit requirement:
a mean of 100 µs is irrelevant if p99 is 20 ms. The policy path is measured
separately: `latency/world_tick_idle` is the trigger check, and
`latency/world_consolidate_1` pays one edit, the DAG rebuild and the disk sync.

**Save scan** (`save-scan`, M1-27) is a reference reader, not a benchmark. It
walks every `r.<x>.<y>.<z>.vxr` region log, resolves the live record per cell
(last DAG wins, then the last overlay snapshot), and emits one JSON object:

- per chunk: serialized DAG bytes (`dag_bytes`), node words and node bytes
  (`node_words * 4`), overlay-snapshot bytes, all record bytes for that cell
  (superseded records included), non-air voxel count, overlay entries, and a
  material histogram;
- per save: file/record/live/superseded bytes, total voxels and overrides,
  the aggregate material histogram, and the per-chunk `node_bytes` and `voxels`
  distributions (`p50`/`p95`/`max`, nearest rank);
- `warnings` for torn tails, corrupt records and unreadable files.

The scan opens files read-only and never truncates a torn tail (unlike the live
store, which repairs it on open). Air is excluded from voxel counts and
histograms; material keys are decimal ids. An empty directory is a valid empty
report. A missing directory exits non-zero.

**Fixture world** (`fixture`, M1-28) is the frozen counterpart to `save-scan`.
`benchmarks/fixtures/m1-representative/` contains a deterministic 72-chunk save
with terrain, caves, canyon overhangs, structures, vegetation and 19 non-air
materials, plus a manifest that records provenance, the recipe, the raw-id
palette, BLAKE3 hashes and the M1-27 observations. `fixture build` freezes a new
directory; `fixture verify` checks the bytes, re-scans them and rebuilds the
recipe byte-for-byte. The save is **provisional until the first real save**: its
numbers are illustrative and must not be quoted as gate results. See
`benchmarks/fixtures/README.md`.

**Gate memory** (`gate memory`, M1-35) walks that frozen fixture and reports one
row per component with p50/p95/max over chunks plus the fixture-wide total:

| Component | Counted bytes |
|---|---|
| **DAG** | Banded 8 B internal nodes (alignment padding included) + 2 B leaf materials |
| **Overlay** | Allocated GPU overlay storage: a block table per edited chunk plus 8 704 B per edited 16³ block |
| **Light** | `LIGHT_BYTES_PER_CHUNK` (1 KiB at 64³, 8 KiB at 128³) |
| **Probes** | `PROBE_BYTES_PER_CHUNK` near-LOD volume (768 B at 64³, 6 KiB at 128³) |
| **Mesh estimate** | M1-31 greedy quads × 76 B; count-only, no vertex/index buffer overhead |
| **Total** | The five components above, per chunk |

The transitional v1 `Svdag` pool is printed as a separate row and excluded from
the total: it is the pre-banded compatibility view, not the layout the gate is
deciding. The fixture has no pending overlay entries, so its overlay component
is zero by measurement, not by assumption. Mesh is an upper bound at chunk
boundaries because the M1-31 reference has no neighbour apron; A/B root overhead
is not part of this measurement. The report carries the fixture's provisional
limitation verbatim and refuses to measure a save whose live chunk, voxel,
overlay or region counts differ from the manifest. `--json` prints the
serializable report; `--out FILE` also writes it.

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
