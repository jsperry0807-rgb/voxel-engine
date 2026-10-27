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

# Loaded chunk count for the configured view/band (M1-36)
cargo run -p voxel-bench -- gate residency
cargo run -p voxel-bench -- gate residency --profile minimum
cargo run -p voxel-bench -- gate residency --json

# Per-edit latency split into fixed and dirty-region terms (M1-37)
cargo run -p voxel-bench -- gate edits
cargo run -p voxel-bench -- gate edits --json

# Incremental probe-update latency per edited chunk (M1-38)
cargo run -p voxel-bench -- gate probes
cargo run -p voxel-bench -- gate probes --json

# The twelve pre-registered rows as one gate.md + gate.json (M1-39)
cargo run -p voxel-bench -- gate report
cargo run -p voxel-bench -- gate report --profile minimum
cargo run -p voxel-bench -- gate report --json
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

**Gate residency** (`gate residency`, M1-36) evaluates the frozen residency
model for the configured near radius and vertical shell:

```text
N(edge, R, V) = ceil(pi * (R / edge)^2) * ceil(V / edge)
```

The baseline count is measured at the 64³ edge. The 128³ count is the
**equal-volume equivalent**, `N128 = N64 / 8`, not a second independently
rounded view; the report asserts both `N64 = 8 * N128` and
`N64 * 64³ = N128 * 128³`. `--profile` selects `minimum`, `default` (the
default) or `high`; `--radius` and `--band` override the profile in metres.
The report also prints the frozen pre-registration row and its delta. The
frozen table rounded some rows to nearest instead of ceiling: Minimum/64³ is
8 chunks lower (6,432 versus the formula's 6,440), and Default/128³ is one
lower (3,216 versus the equal-volume 3,217). Those deltas are visible rather
than hidden.

The count is geometric, not content-dependent: the 72-chunk fixture cannot
fill a 25,736-chunk view. `gate memory` supplies the per-chunk bytes; this row
supplies `N` for the memory gate. High is accepted but is not gated on the
reference machine, and a custom view/band is reported without a
pre-registration comparison.

**Gate edits** (`gate edits`, M1-37) measures 1,000 real edits on the fixture and
splits each one into two terms:

| Term | Components | What the gate charges |
|---|---|---|
| **Fixed** | upload, fence, flip | Appended run bytes + leaf materials + the 7-word `levelStart`, the batch-fence boundary, and the version-index flip |
| **Dirty region** | COW, light | Banded `apply_edits` for the edited path, and the M1-29 reference fill over the dirty block cell plus its 1-cell apron |

`fixed + dirty` is the per-edit total; the Markdown prints p50/p95/max and the
JSON also carries every edit. Upload is the CPU-side staging copy of exactly the
appended runs, with no padding words and no whole-chunk transfer, so
`upload_node_bytes + upload_leaf_bytes` equals the measured COW bytes. A
single-voxel edit dirties one 16³-voxel block cell, so the reference fill
evaluates 27 cells (the cell plus its apron); the dirty-region term is not a
full-chunk relight.

The gate is headless, so the fence is the CPU side of the M4 protocol: submit,
Release-publish, Acquire-poll, then flip. It is a lower bound on a real device
fence; M4-05/M4-16 replace it. The fixture is still provisional, and the report
carries that limitation. `--json` prints the serializable report; `--out FILE`
also writes it.

**Gate probes** (`gate probes`, M1-38) measures the incremental probe update that
one edited chunk triggers, with the pre-registered p95 ≤ 4 ms budget:

| Term | Components | What the gate charges |
|---|---|---|
| **Update** | select, rebake, upload | Find the probes whose rays reach the edited voxel, recast their 64-ray hemisphere, stage the changed 12 B records and validity bytes |

The near grid has one probe per block, so a 64³ chunk has 64 probes and a full
rebake is 4,096 rays. ADR-005 requires an edit to rebake only the probes whose
rays reach the dirty region; the harness implements that rule with the M1-30
reference. Selection uses the **pre-edit** DAG: a ray that stops at a solid voxel
today will continue after a solid→air edit, and a ray that passes through an air
voxel today will stop after air→solid. A probe whose centre is the edited voxel
is selected even though it casts no ray while invalid.

Every candidate is a real fixture voxel that is the first solid on a probe ray,
so the edit can change at least one ray's radiance; candidates are spread over
probes and rays and alternate solid→air (mine) and air→solid (place). The report
prints affected probes and rebaked rays alongside the latency, so a run cannot
look cheap by selecting nothing: zero-work edits are counted and shown. Upload is
the CPU-side staging copy of exactly the changed records and validity bytes; the
headless gate excludes device transfer and the M4 fence/flip. `--json` prints the
serializable report; `--out FILE` also writes it.

**Gate report** (`gate report`, M1-39) runs the four harnesses above and writes
one `gate.md` plus one `gate.json` (default `target/gate/`). The report contains
the twelve frozen rows from `docs/pre-registration.md` §3, in table order:

| # | Row | Source |
|---:|---|---|
| 1 | VRAM budget for voxel data | memory × residency |
| 2 | Target loaded chunk count | residency |
| 3 | SVDAG node budget/chunk | memory |
| 4 | Light volume/chunk | memory |
| 5 | Probe grid/chunk | probes + memory |
| 6 | Mesh buffer/chunk | memory |
| 7 | Edit latency | edits |
| 8 | Probe-update budget | probes |
| 9 | Culling granularity | M1-34 reference (not consumed by this generator) |
| 10 | Safety factor | pre-registration invariant |
| 11 | Measurement dataset | memory fixture |
| 12 | Fallback trigger | pre-registration §4 |

Every verdict is computed from a measured value and a frozen limit; the report
does not type `PASS`. Each row carries the measured value, the limit, a signed
margin and a machine-readable `numbers` block. The header names the reference
machine from `docs/reference-machine.md` and the actual host CPU/OS/architecture;
the headless gate does not open a wgpu adapter, so the GPU is named but not
re-probed. The overall gate is `pass` only when all twelve rows pass, `fail` when
any row fails, and `incomplete` when no row fails but a required measurement is
missing. The synthetic fixture and the missing granularity result keep today's
report from being a green gate; the fallback preview is for M1-42.

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
