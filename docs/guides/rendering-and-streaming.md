# Rendering and streaming

How 256 chunks get from “not generated” to “on screen” without the frame
stuttering — and what the current slice deliberately leaves for later.

The measured reports are in [`milestones/m3/`](../milestones/m3). The design
constraints are in the [architecture spec §11–§12](../specs/voxel-engine-architecture-spec.md).

## 1. The goal

At the gated edge the viewer keeps **256 chunks** resident around the camera
(16 × 16, 8 m each, 128³ voxels) and draws them at 1080p. The interesting part
is not the draw call; it is that generation and meshing take tens of
milliseconds per chunk, and the frame has **16 ms**.

The answer is a pipeline with **three stages** and **bounded work on the frame
thread**.

## 2. The ring

`streaming::ring_chunks(center)` returns a 16 × 16 ring of chunk positions,
nearest first. The ring is centred on the camera's horizontal chunk position.
As the camera moves, the trailing column is evicted and the leading column is
requested.

The ring is horizontal by design. Vertical streaming (the surface shell and cave
cache) is later work; this slice proves the resident set and the frame budget.

```
        ┌───────────────────────────┐
        │  ring: 16 × 16 chunks     │
        │  centre follows the camera│
        └───────────────────────────┘
```

## 3. Three stages

```
frame thread ──pump──> install finished resources
      ▲
      │ bounded channel
preparer / uploader ──> reserve pool space, write mesh bytes
      ▲
      │ bounded channel
generator + mesher workers ──> block grid + mesh + light + probes
```

**Workers** generate and mesh. The default pool is
`min(available_parallelism(), 4)`; `VOXEL_STREAM_WORKERS` overrides it.

**The preparer** reserves a range in the mesh pool and writes the mesh bytes. It
is deliberately off the frame thread: a single chunk upload once measured
**6.2 ms mean and 20.9 ms max**, which on the frame thread is a guaranteed
hitch.

**The frame thread** calls `pump` before acquiring the surface texture. `pump`
recentres the ring if needed and installs finished resources until the **4 ms**
budget or the channel is empty. Installing is a map insert, so it is normally
microseconds.

Measured result: **256/256 loaded, max pump 0.09 ms, 0 hitches over 4 ms** on
both reference GPUs.

See [M3-01](../milestones/m3/m3-01-streaming.md).

## 4. The mesh pool

Per-chunk vertex buffers were replaced by **one paged arena**:

- 8 MiB pages; a mesh is one contiguous range inside a page.
- Best-fit allocation over a free list, so a ring rotation reuses the column it
  just evicted.
- The frame thread never calls `create_buffer` after warm-up.
- No draw crosses a page boundary.

The measured ring puts **100–120 MiB of live meshes** into 8 MiB pages; because
the pool commits whole pages, the reports show **104–128 MiB** committed. That
is still a rounding cost of only a few percent.

Fixed-size slots were rejected: they would waste about 40% against the mean mesh
and still cap the largest one. Per-chunk buffers were rejected because every
install would allocate and every eviction free, forever.

See [M3-02](../milestones/m3/m3-02-mesh-pool.md) and
[ADR-010](../adr/ADR-010-mesh-granularity-vertex-format.md).

## 5. The upload queue

The pool removed the destination allocation, but the driver still wanted a
staging buffer per write. The upload queue turns that into a **per-frame byte
budget**:

- Finished meshes queue in a FIFO with a byte cost.
- The frame takes jobs in order until the next would exceed the budget.
- An oversized mesh is taken alone and counted as an overrun, so the queue
  always makes progress.
- The backlog is capped at 32 MiB; past that, workers block instead of growing
  memory without bound.

The default budget started at 1 MiB, but the motion test measured a **6.79 ms
p99** upload stage — over the 4 ms budget. It is now **512 KiB**, which measures
**1.6–3.1 ms p99** and drains the 256-chunk ring in **260 frames** instead of
141.

See [M3-03](../milestones/m3/m3-03-upload-queue.md) and
[M3-09](../milestones/m3/m3-09-hitch-test.md).

## 6. Culling: the cheapest win

The current cull is deliberately simple: build six frustum planes from the
camera view-projection matrix, test each resident chunk's world AABB, and skip
chunks entirely outside the view.

Measured at the default pose:

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Draw calls | **93 of 256** | **93 of 256** |
| Cull cost | **0.04–0.05 ms** | 0.05–0.06 ms |
| Frame before cull | 8.4 ms | 25.5 ms |
| Frame after cull | 5.5 ms | 13.7 ms |

The cull removes 63% of the draw calls and costs about 0.4% of the frame. It is
the **M1-32 reference** in per-chunk form; M5 replaces it with a GPU broad phase
and block-level occlusion, and must match it.

It is conservative: a box touching a plane survives, so the culler may draw more
than necessary, never less. There is no occlusion yet, so a chunk behind a
mountain is still drawn if its AABB intersects the frustum.

See [M3-04](../milestones/m3/m3-04-cpu-cull.md) and
[ADR-006](../adr/ADR-006-occlusion.md).

## 7. The draw loop

`draw_visible` is one pass over the resident map:

```rust
draw_visible(chunks, keep, draw) -> DrawStats
```

- A rejected chunk costs one plane test and nothing else.
- Each accepted chunk gets **one indexed draw** with its pooled mesh and light
  bind group.
- The image does not depend on order: every draw is opaque geometry into the
  same depth-tested pass. Sorting would only add allocations, so map order
  stays.

The correctness test renders a 5 × 5 patch twice — once culled, once with every
chunk accepted — and requires **byte-identical** output. It also asserts no
chunk is drawn twice.

See [M3-05](../milestones/m3/m3-05-draw-loop.md).

## 8. Where the frame goes

Steady state, 1080p, 128³, 256-chunk ring:

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Frame | **5.5 ms** | **13.7 ms** |
| GPU pass (draw) | 1.6–1.7 ms | 8.9 ms |
| CPU submit | 1.2 ms | 0.9 ms |
| ├ cull + recording | 0.05 ms | 0.05 ms |
| ├ upload | 0.00 ms | 0.00 ms |
| └ finish + submit | 1.1 ms | 0.8 ms |
| Residual (present/driver) | 3.9–4.3 ms | 4.6–4.8 ms |

The 4060 is **present-bound**, not geometry-bound: the render pass is only 10%
of its frame. The iGPU is **draw-bound**: 54% of its frame is the pass, leaving
about 2.9 ms of slack at 60 Hz.

That difference is exactly why the 60 Hz result splits:

- RTX 4060: **192–202 fps average, 104–113 fps 1% low** — clears comfortably.
- Radeon iGPU: **70–75 fps average, 56.6–61.2 fps 1% low** — misses the tail.

The iGPU miss is an open condition carried out of M3, not a streaming bug. The
levers are scene LOD, a cheaper fragment path and the indirect work in M5.

See [M3-07](../milestones/m3/m3-07-frame-breakdown.md) and
[M3-10](../milestones/m3/m3-10-60hz.md).

## 9. What this slice does *not* include

- **No far field, water, fog or map** — M3.5.
- **No GPU indirect draws or block occlusion** — M5.
- **No LOD** — M5/M6.
- **No versioned DAG roots or COW sync on the GPU** — M4. The M3 scene uploads
  dense meshes, light and probes; it does not upload the sparse tree.
- **No worldgen** — M2.5. The M3 scene is analytic and seeded, not generated by
  the worldgen design.

The memory number reflects that: the 256-chunk scene holds **107.5 MiB** of
VRAM (mesh 104.0, light 2.0, probes 1.5), which is 3.4% of the 3.20 GiB target.
It is a render-scene number, not the storage gate's row 1. See
[M3-11](../milestones/m3/m3-11-memory.md).

## Where to go deeper

- [Architecture tour](architecture-tour.md) — the data structures behind the
  frame.
- [M3 reports](../milestones/m3) — every measurement in this guide.
- [Performance and measurement](performance-and-measurement.md) — how to read
  the numbers.
- [`wgpu-traps.md`](../notes/wgpu-traps.md) — the API rules that cost time.
