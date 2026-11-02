# M3-09 streaming hitch test

**Status:** landed 2026-10-31. `cargo run -p voxel-bench -- hitch` drives the
viewer's real frame-thread streaming path for 1,000 frames of camera motion and
reports per-stage percentiles. It also **halved the default upload budget**: at
1 MiB the upload stage's p99 was 6.8 ms, over the 4 ms budget; at 512 KiB it is
1.6–3.1 ms with a worst frame under 7 ms.

## Acceptance

| Requirement | Measured (128³, 1,000 frames, 16 u/s) |
|---|---|
| Upload p99 ≤ 4 ms | **RTX 4060 2.51–2.87 ms**, **Radeon iGPU 1.62–3.11 ms** (p50 1.12 ms on both) |
| No single frame > 8 ms | **6.16–6.42 ms** (4060), **5.72–6.13 ms** (iGPU) |
| Runs as `cargo run -p voxel-bench -- hitch` | yes; exits 1 outside budget, `--no-check` reports only |

## What the loop runs

`crates/engine/viewer/src/hitch.rs` + `voxel-bench hitch`:

1. Create a headless device (no surface, no swap chain).
2. Build the real `Streaming` (16×16 ring, four generator workers, one uploader)
   and a `StagingBelt`.
3. **Warm up** at the start pose until the ring is resident; those frames are
   reported separately, not counted in the measurement.
4. Fly a straight diagonal at 16 world units/s for 1,000 frames (≈ 42 ring
   recenters, ≈ 0.76 uploads per frame) and measure each frame.

Each frame is paced to the simulated 60 Hz duration. Pacing matters: without it
the loop spins through a thousand "frames" in milliseconds while the workers
mesh in wall time, and the measurement never sees an upload.

Per frame it measures the same calls the viewer makes:

| Stage | Call | What it covers |
|---|---|---|
| `pump` | `Streaming::pump` | ring recenter, evict, install budget |
| `upload` | `Streaming::install_frame` (new, shared with `Gpu::render`) | take, staging copies, install |
| `finish` | `finish_and_recall_on_submit` + `Queue::submit` | belt unmaps and submit |
| `total` | the three above | frame-thread work |

The upload step was extracted from `Gpu::render` so the bench measures the same
function the viewer runs — not a copy of it.

The GPU wait after the submit is **excluded**: a hitch test bounds frame-thread
stalls, not draw time (that is M3-10). The loop still polls to completion each
frame, so the belt recycles and the queue stays at the depth a presented frame
would see.

## Measured

Release, `--features chunk128`, default seed, 1,000 frames at 16 u/s, 512 KiB
upload budget:

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Upload p50 / p95 / p99 / max | 1.12 / 1.43–1.47 / **2.51–2.87** / 5.87–5.91 ms | 1.12–1.13 / 1.43–1.46 / **1.62–3.11** / 5.54–6.01 ms |
| Total p50 / p99 / max | 1.34 / 3.71–4.13 / **6.16–6.42** ms | 1.24–1.25 / 3.66–5.14 / **5.72–6.13** ms |
| Frames with an upload | 760 / 1000 | 763–766 / 1000 |
| Bytes copied during the run | 302 MiB | 304 MiB |
| Overruns / pump hitches | 27 / 0 | 26–27 / 0 |
| Resident at the end | 247 / 256 (9 pending) | 247 / 256 |

The slowest frames are isolated preemptions, not a stage that grows with load:
`slowest frame 766: upload 5.90 ms (491 KiB)`, next to frames that copy 450 KiB
in 1.1 ms. A single frame copying ~500 KiB occasionally takes 4–6× longer
because the four generator workers and the uploader compete for the same cores.

### The 1 MiB budget was too big

With the M3-03 default of 1 MiB the same run measured **upload p99 6.79 ms**
(max 8.63, total max 8.89) and failed. The staging copy is ~2.5 µs/KiB on the
reference machine, so a 1 MiB frame is ~2.6 ms of memcpy before any contention;
the measured p99 says it is over the budget once the generators are running.
The default is now **512 KiB** (`upload::DEFAULT_UPLOAD_BYTES`), which:

- halves the copy per frame (p99 1.6–3.1 ms, ~2× margin to 4 ms);
- doubles the frames needed to drain a backlog: the 256-chunk ring now uploads
  in **260 frames** instead of 141 (M3-08's table was recorded at 1 MiB);
- leaves meshes above the budget as counted overruns (27 in 1,000 frames),
  which still make progress one per frame.

### Worker contention is the tail

Warm-up (the initial fill) is reported separately, because it is the worst part
of the run:

| Workers | Warm-up worst upload | Warm-up worst frame | Motion p99 upload | Motion max total |
|---:|---:|---:|---:|---:|
| 4 (default) | 14.7–15.8 ms | 16.1–17.9 ms | 2.5–2.9 ms | 6.2–6.4 ms |
| 1 (`VOXEL_STREAM_WORKERS=1`) | 7.05 ms | 7.36 ms | 1.43 ms | 2.29 ms |

The upload path itself is ~1.1–1.5 ms per frame; the tens-of-milliseconds tail
during the initial fill is CPU contention from four generator workers plus the
uploader on WSL, and it disappears with one worker (at the cost of a much slower
fill). The motion window M3-09 gates on does not hit it.

## Tests

```bash
cargo test -p voxel-bench hitch
cargo test -p voxel-viewer --test hitch
```

`hitch::tests` pin the nearest-rank percentile, the pass rule (upload p99 and
frame max are both required) and the table. `tests/hitch.rs` runs a 30-frame
warm-up + motion smoke test on the first available adapter and checks the report
is coherent; it skips without an adapter.

## Limits

- **Headless:** no surface, no draw, no present. The render pass is GPU work and
  is covered by M3-10; this test bounds the CPU streaming path.
- **One machine class:** Dozen under WSL. The absolute copy rate is a property
  of that stack; the budget is sized from it.
- **Simulated motion:** a straight diagonal at one speed. Turns, teleports and
  vertical flight are not exercised; M3.5-13 adds a 2 km flight.
- **The initial fill still has ~15 ms copy outliers** with four workers. The
  budget bounds the bytes, not a preemption during the copy; a dedicated copy
  budget per frame (smaller than a mesh) would need mesh splitting.
- The pass rule is a measurement, not a CI gate: a busy machine can exceed the
  max, which is why `--no-check` exists for shared runners.
