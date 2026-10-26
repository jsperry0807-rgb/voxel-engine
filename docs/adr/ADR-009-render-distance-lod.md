# ADR-009: Render Distance and LOD Policy

**Status:** Accepted (closed) — 2026-10-26
**Amendment:** M6-02 selects the LOD representation with measured numbers; the ring
policy below does not change.
**Decision owner:** engine
**Supersedes:** the 64 m world height once planned for this ADR, and the explicit
per-chunk LOD-tree position in
[`../reference/adr-three-tier-store-v2.md`](../reference/adr-three-tier-store-v2.md) §10.3
**Related:** spec §4.1, §4.2, §7, §13.5, §14.1;
[`../pre-registration.md`](../pre-registration.md) §1–2;
[`../performance-targets.md`](../performance-targets.md) §2–4;
[`../reference-machine.md`](../reference-machine.md) §3;
[`../worldgen-design.md`](../worldgen-design.md) §3.2; ADR-001, ADR-002, ADR-005,
ADR-006, ADR-010, ADR-012

## Context

The world is 1,024 km square and 12,000 m tall; the horizon is game identity. The
gated profiles draw near voxels only to 64 m (Minimum) or 128 m (Default), then two
LOD rings, then a macro heightfield to an 8 km or 16 km far clip (High: 256 m,
1,024 m, 32 km). Drawing the whole 512 m disc at full detail is not a tuning choice:
it is 16× the Default LOD0 area, and the Default LOD0 shell alone is 3,216 chunks at
1.2–3.2 MiB = **3.8–9.8 GiB** against a 3,275 MiB target
([`../pre-registration.md`](../pre-registration.md) §2–3). LOD and streaming are the
memory policy, not an optimization added after the fact.

The spec fixes the shape of the answer: LOD is a cutoff of the same SVDAG traversal,
not a second tree (§4.1); probes are per-block near, half-res mid, none far (§7); the
far field is a separate raster heightfield with a near/far seam (§14.1). What is not
fixed in one place is the ring table, the vertical residency rule, and what M6 must
measure before choosing the LOD representation. The 64 m world height from the old
task plan is retired by spec v2.3 and [`../worldgen-design.md`](../worldgen-design.md).

## Decision

**The ring policy is fixed now. The LOD representation is measured at M6 and amended
under this ADR.**

1. **Four view layers, profile-defined radii.** LOD0 emits every block separately
   with its full-resolution voxel mesh; LOD1 aggregates 2 × 2 × 2 blocks per node;
   LOD2 aggregates 4 × 4 × 4 blocks per node; beyond LOD2 the macro heightfield takes
   over.

   | Profile | LOD0 | LOD1 | LOD2 | Far clip |
   |---|---:|---:|---:|---:|
   | Minimum | 64 m | 128 m | 256 m | 8 km |
   | Default | 128 m | 256 m | 512 m | 16 km |
   | High (not gated) | 256 m | 512 m | 1,024 m | 32 km |

   These are the radii in [`../reference-machine.md`](../reference-machine.md) §3 and
   [`../pre-registration.md`](../pre-registration.md) §1. Default is the shipping
   target; Minimum is the fallback; High is not gated on the reference machine. The
   radii are in metres and therefore survive the chunk-edge gate; only chunk counts
   and per-chunk bytes change with the edge.
2. **LOD is a depth cutoff in the existing traversal.** For ring *n*,
   `lodMaxDepth = BLOCK_LEVEL − n` (LOD0 = `BLOCK_LEVEL`, the emit level in ADR-012);
   traversal stops at that depth or at a leaf, as spec §4.1 requires. There is no
   per-chunk LOD tree, no second culling structure and no per-frame remesh. The SVDAG
   remains the culling, lighting and picking structure (ADR-001). LOD0’s emitted
   blocks still carry voxel-resolution meshes; only the block aggregation changes with
   the ring. Whether a far ring keeps full-resolution storage or uses an aggregated
   LOD mesh is the M6 measurement; the ring selection and cutoff are fixed here.
3. **Vertical residency is a surface shell, not a column.** The gated profiles hold
   chunks within ±16 m of the surface (32 m total); High holds ±32 m (64 m total). A
   128 m cave cache follows the player below the surface and is evicted on ascent. The
   12,000 m world is never resident as full columns: one 8 m column alone is 1,500
   chunks. The “64 m height” in M0-17 was the old world height; the only 64 m vertical
   figure now is High’s total shell, and it is not gated.
4. **Ring behavior is explicit.** The worldgen detail pass is LOD0-only; LOD1/2 drop
   sub-block detail content and collapse dedicated detail templates to the dominant
   material. Probes follow the same rings: per-block near, half-res mid, none far; a
   probe-LOD change is a content event, not a frame event (spec §7). The 128 m occlusion
   flood-fill horizon is a culling bound, not a render distance: beyond it the far
   rings are frustum-culled only (spec §4.2).
5. **The far field is not a voxel LOD.** At the outer LOD2 radius the near renderer
   hands off to the macro heightfield: 128 m macro cells, 250 × 250 cells at 32 km,
   its own LOD rings, a 64–128 m blend band plus skirt, and fog matched to the near
   renderer (spec §14.1). The seam always coincides with the outer LOD2 radius
   (512 m at Default, per spec §14.1; the reduced profiles move it in with their
   rings: 256 m at Minimum, 1,024 m at High), so there is no gap between voxels and
   heightfield. The far-clip floor is 8 km and is never reduced to fix memory
   (ADR-002).
6. **Transitions are events, not per-frame work.** Ring membership uses horizontal
   camera-to-chunk-AABB distance and hysteresis; M6-06 owns the margin. Streaming
   prefetches by camera velocity, evicts by distance with hysteresis, and keeps a
   per-frame budget (M6-05…M6-07). **M6-01** measures depth-limited traversal (hold
   full-resolution chunks, cut at `lodMaxDepth`) against explicit aggregated LOD
   meshes (2× per level, cached by content hash) on three seeds, reporting VRAM,
   frame time and pop-in. **M6-02** records the decision as a one-page amendment with
   numbers inline. M6-03/04 implement the winner; M6-15/16 choose stitch or skirt and
   prove no seam crack. The amendment may change the LOD representation and per-ring
   memory; it may not change the ring radii, layer order, vertical shell, far-field
   seam or 8 km floor without a new ADR.

## Consequences

- **Positive:** detail is bounded to a disc whose area is fixed in M0, and the
  16–32 km horizon comes from a heightfield costing under 50 MB rather than voxel
  chunks. LOD selection is one compare in the existing traversal; probes, streaming
  and eviction share the same ring events.
- **Negative:** Default LOD0 is already the memory-risk profile: 3,216 chunks ×
  1.2–3.2 MiB = 3.8–9.8 GiB against a 3,275 MiB target and a 4,913 MiB fail line.
  Extending LOD0 to 512 m would be 16× the area (about 61–157 GiB), so LOD1/2 must
  carry most of the visible world at a fraction of LOD0’s per-chunk bytes. Whether
  that fraction is achievable is exactly what M6-01 measures. At 64³ the same rings
  hold 8× the chunks (25,736 in the Default LOD0 shell), which is ADR-002’s gate.
- **Explicit trade:** M0 fixes *where* detail drops, not *how* it is stored. M6 may
  replace full-resolution far chunks with explicit LOD meshes, or keep the cutoff; it
  may not quietly move the rings outward. The player sees a fixed horizon profile;
  the implementation is allowed to get cheaper.
- **Invariants:** LOD0/1/2 at the table’s radii; ±16 m gated shell (±32 m High); no
  full vertical columns; far clip ≥ 8 km; no per-frame GPU→CPU readback; no per-frame
  remesh; probe LOD change is a content event; the far field is a raster heightfield,
  not raymarched voxels.
- **Verification:** M6-01 experiment table (VRAM, frame time, pop-in); M6-17 ring
  budget within 40% VRAM; M6-18 streaming p99 ≤ 4 ms over a 2,000-frame flight;
  M6-15/16 seam fixture; M3.5 far-field view and seam; M6-20 updates the rings table
  and tags `m6`.

## Rejected alternatives

- **LOD0 to the full 512 m near field.** It is 16× the LOD0 area; at Default per-chunk
  bytes that is about 61–157 GiB. The LOD rings exist precisely to avoid it.
- **A second explicit LOD tree per chunk as the default.** It duplicates the SVDAG and
  adds a build/cache/streaming pipeline before M6 measures a need; it is the reference
  design’s position this ADR supersedes. Explicit LOD *meshes* remain a measured
  alternative at M6, not the default.
- **Full vertical columns.** 12,000 m / 8 m = 1,500 chunks per column; a 128 m disc
  would be over a million chunks. The surface shell plus cave cache is the only
  affordable residency shape.
- **Stream the 64 m world height.** Retired by spec v2.3; a 64 m world would erase the
  vertical scale the worldgen design depends on.
- **Uniform LOD or one global cutoff.** One detail level either wastes memory far away
  or looks blocky underfoot. The ring table is the quality/memory dial.
- **Cut the far clip to fix memory.** The horizon is game identity; the floor is 8 km,
  and memory failures follow the sacrifice ladder (mesh compression, near radius,
  shell), not the horizon.
- **Remesh per frame or transition without hysteresis.** Content-timeline work only;
  per-frame remeshing blows the 12.0 ms draw budget and the 8 ms edit budget, and no
  hysteresis thrashes at every ring boundary.

## Status of open questions

None at the policy level. The ring table, vertical shell, far-field handoff and 8 km
floor are fixed. M6-02’s amendment is a pre-registered, data-driven choice of LOD
representation within that policy; changing the policy itself requires a new ADR.
