# M3-11 memory report: VRAM and host for the 256-chunk scene

**Status:** landed 2026-11-01. The 256-chunk scene at 128³ holds **107.5 MiB of
VRAM** (mesh 104.0, light 2.0, probes 1.5, origins 4 KiB) and **16.8 MiB of host
heap** (the workers' dense volumes). The 40% VRAM target is **3.20 GiB**, so the
scene uses **3.4%** of it: **29.8× headroom**, and usage × 1.5 = 161.3 MiB still
fits with 20× to spare.

## Acceptance

| Requirement | Measured (128³, 256 chunks) |
|---|---|
| VRAM and host for the 256-chunk scene | 107.5 MiB VRAM, 16.8 MiB host heap; RSS 220 MiB (process, driver included) |
| Rows split by DAG / mesh / overlay / light / probes | the table below, plus origins and the host columns |
| Fits the 40% VRAM budget with 1.5× margin | 3.20 GiB target; **107.5 MiB = 3.4%**, ×1.5 = 161.3 MiB → **PASS** |

## Method

```sh
cargo run --release -p voxel-bench --features chunk128 -- scene-memory
cargo run --release -p voxel-bench --features chunk128 -- scene-memory --json
```

`scene-memory` builds the **real** streaming scene headlessly (same `Streaming`,
same workers, same uploader, same staging belt), fills the 256-chunk ring, and
reads:

- **VRAM** from the streaming scene's own accounting: the mesh pool's committed
  pages (`ArenaStats::capacity_bytes`), and the light, probe and origin buffers
  of every resident chunk (`ChunkLighting::light_bytes`/`probe_bytes`). Nothing
  is estimated from constants except the 16 B origin uniform.
- **Host** from the scene's buffers (each worker's dense volume, the finished
  uploads waiting for a frame budget) plus the process counters: the binary's
  counting allocator (live and peak heap) and `/proc/self/status` RSS.
- The heap is read **while the scene is still resident**; dropping it first would
  report the workers' volumes as already freed.

The five rows the milestone names are the first five; `origins` is the small
per-chunk binding the renderer also holds.

## Measured

Release, `--features chunk128`, default seed, 512 KiB upload budget:

| row | VRAM | host | per chunk | note |
|---|---:|---:|---:|---|
| DAG | 0 | **16.0 MiB** | — | M3 generates dense 2 B/voxel volumes on the host; no SVDAG is uploaded (M4) |
| mesh | **104.0 MiB** | 0 | 416 KiB | 13 × 8 MiB pool pages; 100.5 MiB live, 3.5 MiB slack |
| overlay | 0 | 0 | — | no edit overlay in the M3 scene (M4) |
| light | **2.0 MiB** | 0 | 8 KiB | one light-cell buffer per chunk |
| probes | **1.5 MiB** | 0 | 6 KiB | one near-grid SH4 buffer per chunk |
| origins | 4 KiB | 0 | 16 B | per-chunk origin uniform |
| **total** | **107.5 MiB** | **16.0 MiB** | **430 KiB** | |

| | |
|---|---:|
| VRAM target (40% of 8 GiB) | 3.20 GiB |
| Hard fail | 4.80 GiB |
| Used | 107.5 MiB = **3.4%** |
| Headroom | **29.8×** |
| Usage × 1.5 | 161.3 MiB → **PASS** |
| Host heap live / peak above baseline | 16.8 / 20.4 MiB |
| Process RSS / peak | 220.0 MiB |

Both GPUs report the same bytes: the allocations are size-only, and the pool
page size comes from the adapter's `max_buffer_size` (2 GiB advertised by
Dozen), so both land on 8 MiB pages.

### Viewer overhead, outside the scene rows

The headless report covers the streaming scene. A running viewer also holds:

| Resource | Bytes |
|---|---:|
| Material atlas, 256² + 5 mips, RGBA8 | 341 KiB |
| Depth target, 1920×1080, D32 | 7.9 MiB |
| Staging belt chunk | 2.0 MiB |
| Camera uniform and readbacks | ≈0.2 KiB |
| **Total** | **≈10.3 MiB** |

Scene plus viewer is **≈118 MiB**, still 3.7% of the target.

### 64³ for scale

| | 128³ (gate) | 64³ |
|---|---:|---:|
| VRAM total | 107.5 MiB | 32.4 MiB |
| Mesh pool | 104.0 MiB (13 pages) | 32.0 MiB (4 pages) |
| Light + probes | 3.5 MiB | 0.45 MiB |
| Host volumes | 16.0 MiB | 2.0 MiB |

The mesh is ~3.2× the 64³ scene while the light and probe buffers are ~8×: the
mesh follows surface area, the content volumes follow voxel count.

## What these numbers are not

**This is a render-scene number, not the M1 gate's row 1.** The M1 gate charges
DAG + overlay + light + probes + mesh per chunk over an 805-chunk residency
profile; its production 128³ row 1 was 5.29 MiB/chunk and 4.16 GiB. The M3
viewer stores no DAG and no overlay on the GPU, and its mesh is the 44 B/quad
packed format, so 430 KiB/chunk is not comparable to that 5.29 MiB/chunk. The two
converge when M4 uploads versioned roots and meshes; until then, the honest
statement is:

- the **renderer's** 256-chunk scene is 107.5 MiB of VRAM;
- the **storage model's** projection is unchanged and still recorded in
  [`m1-gate-decision.md`](m1-gate-decision.md) and
  [`m2-exit.md`](m2-exit.md);
- if the 805-chunk profile were rendered with today's per-chunk bytes it would
  need ~338 MiB, under the target — but mesh size follows surface area, not chunk
  count, so that figure is an upper bound rather than a plan.

## Tests

```bash
cargo test -p voxel-viewer --lib memory
```

- the row set is exactly DAG/mesh/overlay/light/probes (plus origins);
- DAG and overlay are zero in M3 and carry a reason;
- totals are the row sums, per-chunk is total / resident;
- the margin check passes at the measured sizes and fails a 3 GiB pool;
- host bytes land on the DAG (worker volumes) and mesh (queued uploads) rows.

## Limits

- **Headless measurement.** The rows are the streaming scene's allocations, not
  a driver memory query; wgpu has no portable VRAM counter. The viewer adds
  ~10.3 MiB of fixed resources, listed above.
- **No DAG/overlay yet.** Those rows are zero by construction, not because they
  were skipped; M4 is where they enter VRAM.
- **RSS includes the driver, the binary and thread stacks** (220 MiB against
  16.8 MiB of tracked host buffers). It is a ceiling, not an attribution.
- **The pool commits in 8 MiB pages**, so the mesh row rounds up: 104.0 MiB
  committed for 100.5 MiB live. At 64³ the rounding is larger (32.0 MiB for
  24.8 MiB live), which is why the gate reports both.
- One seed, one scene, one upload budget.
