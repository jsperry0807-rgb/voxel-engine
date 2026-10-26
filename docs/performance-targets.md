# Performance Targets

**Status:** v1 — frozen with the M1 pre-registration, 2026-10-26
**Machine:** [`reference-machine.md`](reference-machine.md) — RTX 4060, 8,188 MiB, 1080p
**Gate:** [`pre-registration.md`](pre-registration.md)

---

## 1. Budgets

### 1.1 Video memory

| Quantity | Value |
|---|---:|
| Total VRAM | 8,188 MiB |
| Voxel-data target (40%) | **3,275 MiB** |
| Hard fail (60%) | **4,913 MiB** |
| Non-voxel reserve (framebuffer, TAA, UI, driver) | ~1.5 GiB |

Voxel data means DAG nodes, meshes, overlays, light, probes, the far-field mesh and
culling buffers. It does not include the swapchain, TAA history, or the OS.

### 1.2 System memory

| Quantity | Value |
|---|---:|
| Dev machine | 32 GB |
| Minimum spec | 16 GB |
| Game budget at minimum | ≤ 8 GB |
| Macro map resident (tiles) | < 64 MB |
| Macro map on disk | ~384 MB raw, 120–220 MB compressed |

### 1.3 Frame budget (Default, 16.6 ms)

| Stage | Budget |
|---|---:|
| Occlusion fill | 0.5 ms |
| Broad phase + traversal | 1.0 ms |
| Content (amortised, budgeted) | 2.0 ms CPU |
| Upload / flip | 0.5 ms |
| Draw + present | 12.0 ms |
| **Headroom** | **1.1 ms** |

The 1% low is allowed to reach 22.2 ms. A frame above 33 ms is a hitch and fails the
profile.

### 1.4 CPU

- 6 cores; 4 per-frame workers for generation and meshing, 2 for main/render.
- Chunk generation p95 ≤ 2 ms; meshing ≤ 1 ms for a terrain chunk.
- No generation or meshing on the edit path.

## 2. Profiles

| | Minimum | Default | High (not gated) |
|---|---|---|---|
| Resolution | 1080p | 1080p | 1440p / 4K |
| Far clip | 8 km | 16 km | 32 km |
| Near voxels | 64 m | 128 m | 256 m |
| LOD rings | 64/128/256 | 128/256/512 | 256/512/1024 |
| Vertical shell | ±16 m | ±16 m | ±32 m |
| Detail pass | reduced | full | full |
| Shadows | none | none | optional |
| Avg / 1% low | 60 / 45 | 60 / 45 | 60 / 45 |

## 3. Why the Default profile is the risk

Using the pre-registered per-chunk ranges:

- 128³ chunk: 1.2–3.2 MiB (DAG 0.5–0.7, mesh 0.6–2.4, overlay 0.03–0.06, light
  0.008, probes 0.006).
- Default near shell: 3,216 chunks → **3.9–10.3 GiB**.
- Target 3.2 GiB, hard fail 4.9 GiB.

So the Default profile is tight at the low end and over budget at the high end. The
first levers, in order, are **mesh size** (quantise, drop indices, LOD the near ring),
then the **near radius**, then the vertical shell. Do not shrink the far clip to
compensate; the horizon is the game's identity.

At the 64³ baseline the same view holds 25,736 chunks at 0.24–0.70 MiB →
**6.2–18.0 GiB**. The chunk-count increase is not offset by the smaller chunks
because meshes and overlays scale with area and chunk count, not volume. This is the
single strongest argument for 128³ and the reason the gate separates memory from
latency.

## 4. Sacrifice ladder

When a profile misses, in order:

1. **Resolution** — 1080p → 1600×900 → 1280×720 native. No upscaler at launch.
2. **Optional AO / contact shadows** — keep baked vertex AO and probes.
3. **Near-field detail / LOD** — drop the detail pass, coarsen the near ring.
4. **Far clip** — 16 km → 8 km, the floor. Never below 8 km.

Never sacrifice: the 8 km floor, determinism, the save format, or the 1 m block grid.

## 5. Anti-aliasing and lighting

- **TAA**, native resolution. Static voxel meshes have no motion vectors; water and
  foliage do. Clamp the history to avoid ghosting on water.
- **No real-time shadows at launch.** Sun direction, vertex AO, block light and
  probes. Contact shadows are the first lighting feature to add if headroom appears.
- Fog is matched between the near voxels and the far heightfield.

## 6. Measurement and CI

- Every milestone records a run through `scripts/bench.sh` with the profile name in
  the label: `LABEL=m1-gate-default`.
- CI is **advisory until the machine has 10 stable runs**, then hard on:
  - memory over the pre-registered budget,
  - p99 hitch over 33 ms,
  - generation p95 over 2 ms.
- Wall-clock microbenchmarks stay advisory permanently; they are machine-specific
  and this is a solo project.

## 7. Platform policy

- Linux: continuous.
- Windows: build green from M3 onward, full test before public release.
- No platform-specific code in `voxel-core`, `voxel-storage`, `voxel-block` or
  `voxel-worldgen`.
