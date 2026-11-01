# M3-07 frame breakdown

**Status:** landed 2026-10-31. Where the frame goes on the reference machine at
1080p, split into **draw** (GPU pass), **cull** (frustum + draw recording),
**upload** (staging copies/installs) and **content** (worker generation). The
short answer: steady state is draw-bound on the iGPU and present-bound on the
RTX 4060; the cull is 0.05 ms; uploads are zero; content is off-thread and only
matters while the ring fills.

## Method

Release viewer, default camera, 1920×1080, `--features chunk128`, ring fully
loaded. Counters from M3-06 (`stats::FrameStats`):

- **cpu submit** — encoder creation to `Queue::submit` return, split into
  `cpu_cull` (frustum + `draw_visible` recording), `cpu_upload` (take + staging
  copy + install) and `cpu_finish` (`finish_and_recall_on_submit` + submit).
- **gpu pass** — in-pass timestamp queries (M3-06).
- **residual** — frame time minus the measured CPU and GPU slices; present,
  driver and queue overhead. CPU and GPU overlap, so this is not additive.

Two runs per GPU (steady state three seconds apart, then a full load with
`RUST_LOG=trace`). The window is the default 1920×1080.

## Steady state

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Frame rate | 179–181 fps | 73 fps |
| **Frame** | **5.5 ms** | **13.7 ms** |
| GPU pass (draw) | 1.60–1.70 ms | 8.92–8.95 ms |
| CPU submit | 1.20–1.23 ms | 0.88–0.92 ms |
| ├ cull + draw recording | **0.04–0.05 ms** | 0.05–0.06 ms |
| ├ upload | 0.00 ms | 0.00 ms |
| └ finish + submit | 1.09–1.11 ms | 0.76–0.79 ms |
| Residual (present/driver) | 3.9–4.3 ms | 4.6–4.8 ms |
| Visible / drawn / resident | 94 / 94 / 256 | 93 / 93 / 256 |
| Geometry | 1,045,145 quads, 43.9 MiB | 1,032,763 quads, 43.3 MiB |

The 93-vs-94 difference is one chunk on a frustum edge between runs (window
aspect), not a culling difference.

### Against the 16.6 ms budget

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Budget at 60 Hz | 16.6 ms | 16.6 ms |
| GPU pass | 1.7 ms (10%) | 8.9 ms (54%) |
| Present/driver residual | 3.9 ms (23%) | 4.8 ms (29%) |
| CPU (overlaps both) | 1.2 ms | 0.9 ms |
| **Headroom** | **≈11 ms** | **≈2.9 ms** |

The RTX 4060 is not GPU-bound at this pose; the frame is paced by present at
~5.5 ms. The Radeon iGPU is **draw-bound**: 8.9 ms of its 13.7 ms frame is the
render pass, so M3-10's 60 Hz target on the iGPU has ~2.9 ms of slack and no
more.

## Where the draw time scales

The M3-01 run (256 draws, before the cull) is the other end of the range:

| Draw calls | RTX 4060 frame | Radeon iGPU frame |
|---:|---:|---:|
| 256 (M3-01, no cull) | 8.4 ms | 25.5 ms |
| 93–94 (M3-04 cull) | 5.5 ms | 13.7 ms |
| Marginal cost | ≈18 µs/chunk | ≈72 µs/chunk |

Draw is linear in visible chunks and dominant on the iGPU. That is the number
M3-08's stress scene and M3-10's 60 Hz gate will move.

## Load phase (content and upload)

Same build, ring filling from empty, `RUST_LOG=voxel_viewer=trace`:

| Stage | Measured |
|---|---|
| Worker generation | **64.8 ms/chunk** mean, p50 57.7, p99 115.9, max 119.3 |
| Ring fill wall time | **6.0 s** (queue → 256 loaded) |
| Upload frames with data | 156 |
| Upload staging copy | **3.15 ms/frame** mean, 17.4 ms max (1 MiB budget) |
| Frame CPU submit during load | 26–33 ms/frame |
| ├ upload | 3.1–3.5 ms |
| ├ cull | 0.02–0.05 ms |
| └ finish + submit | 22–29 ms |
| GPU pass during load | 0.3 → 4.6 ms as chunks appear |

Content is the load-time bottleneck: 256 chunks × 64.8 ms ÷ 4 workers ≈ 4.1 s
of generation, and the ring takes 6.0 s end to end. The frame thread is not
stalled by the pump (0.1 ms) but is slowed by CPU contention: `finish` +
`submit` grows from 1.1 ms steady state to 22–29 ms while four generators and
the staging copy compete for the same cores. Steady state has none of this.

## Verdict

- **Draw** is the only stage that matters at steady state. On the iGPU it is
  54% of the frame and the limiter; on the 4060 it is 10% and present is the
  limiter.
- **Cull** is 3–4% of CPU submit and ~0.4% of the frame. It removed 63% of the
  draw calls (256 → 93), which is the entire win; the loop itself is free.
- **Upload** is zero once the ring is resident and bounded at 1 MiB/frame while
  it fills (M3-03).
- **Content** is entirely off the frame thread but is the reason the ring takes
  seconds to fill, and it steals CPU from the submit path while it runs. M3-09's
  motion test is where that matters.

M3-08 sized the stress scene from these numbers (see
[`m3-08-stress-scene.md`](m3-08-stress-scene.md)); M3-09 bounds motion-time
uploads, and M3-10 decides whether the iGPU's pass leaves enough room for 60 Hz
without M5's indirect path.

## Limits

- One default pose, 1080p, no vsync control exposed; Dozen under WSL.
- The 256-draw row is from M3-01's recorded run, not re-measured here.
- CPU and GPU overlap, so the budget table's rows do not sum to the frame; the
  residual is a subtraction, not a measurement of a single stage.
- No overlay or history; see [`m3-06-instrumentation.md`](m3-06-instrumentation.md).
