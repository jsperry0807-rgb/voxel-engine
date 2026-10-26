# ADR-006: Occlusion — Block Flood-Fill Primary, Ray-Test M2 Flag

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** none — closes the occlusion row in spec §9
**Related:** spec §4.2, §4.3, §5, §8, §11, §12; ADR-001, ADR-002;
[`../pre-registration.md`](../pre-registration.md);
[`../performance-targets.md`](../performance-targets.md)

## Context

ADR-001 removed the per-pixel raymarch. The renderer rasterises meshes, so
visibility is no longer resolved implicitly by a ray at every pixel; the culling
traversal must decide before the draw whether a block can contribute. It must do so
inside the 12.0 ms draw/present budget and without CPU readback (spec §8, §12). A
missed visibility test is a hole in the image; an extra one is only overdraw. The
asymmetry is deliberate: the culler must never under-include.

Two occluders are available. A flood fill over the block grid answers
**connectivity** — can the camera reach this block through non-opaque neighbours?
It is flat, conservative and cheap. An SVDAG ray-test answers **line of sight** — is
the segment to this block clear? It is more accurate, but pays for a ray per test and
duplicates the traversal. The spec selects the first as primary and defers the second
to M2; this ADR fixes that choice, its semantics and its escalation path.

The fill is one visibility bit per block — **8 B per 64³ chunk, 64 B per 128³
chunk** — bounded to a **128 m horizon**: about 2 M block cells, budgeted at 0.5 ms
per frame. A 512 m fill is not affordable, so beyond the horizon the frustum alone
decides.

## Decision

**The primary and only M1 occlusion path is a camera-seeded, 6-connected flood fill
over the block grid, with one visibility bit per block and a 128 m horizon.**

1. **Fill.** Seed at the camera's block. Flood face-adjacent non-opaque blocks out
   to the horizon; every reached block is marked visible. Anything outside the
   horizon is conservatively visible for the frustum to test.
2. **Consumption.** The traversal reads the bit at `BLOCK_LEVEL` and skips the
   entire 16³ block in one test. A chunk whose blocks are all invisible aggregates
   to `OCCLUDED`. The bit test composes with frustum culling; it does not replace it.
3. **Conservative by construction.** The fill may over-include — visibility flows
   through any 1 m gap and around corners — but never under-includes. When seed or
   horizon policy is ambiguous, the block resolves visible.
4. **GPU per-frame state only.** M1 builds the slow CPU reference (M1-33) and the
   granularity instrumentation (M1-34); M5 ports the same fill to compute (M5-01).
   Visibility never leaves the GPU (spec §12).
5. **Semantics: connectivity, not line-of-sight.** This is the decision, not an
   implementation detail. The fill's value is enclosed pockets, caves and built
   structures; in open terrain it will cull little. Outdoor under-culling is expected
   behaviour (roadmap R15), not a defect to repair by changing the chunk edge.

**Escalation.** If the M1 granularity gate (≤ 15% traversed-but-invisible at chunk
boundaries at max view) shows that connectivity under-culls, the escalation is the
SVDAG ray-test in M2, never a chunk-size change. It ships behind
`occlusion.raytest`, default off (M2-25). M2-26 compares its accuracy and cost with
flood-fill on 100 poses over the M1 fixtures and records the recommendation.

**Kill criteria.**

- **Flood-fill is not killed by weak outdoor culling.** The 128 m connectivity fill
  stays primary even where it culls little; a granularity failure escalates to the
  ray-test. Tuning the chunk edge to raise the cull rate is forbidden by
  pre-registration §4 and roadmap R15.
- **The ray-test is killed as a default candidate** unless M2-26 measures a win over
  flood-fill on both accuracy and cost. Without that win it remains a flag and the
  default does not change.
- **The M5 GPU-driven cull is killed on cost** if its draw-work reduction is < 1.5×
  after the timebox (M5-22, M5-23): keep the M3 CPU cull and record the measured reason
  in a dated amendment here. If the M5 work still shows no win after three weeks, ship
  frustum-only (R2).

## Consequences

- **Positive:** the fill is conservative by construction, so it cannot produce a
  false negative; one bit per block keeps it flat and cheap (8 B / 64 B per chunk);
  a single bit test at `BLOCK_LEVEL` skips a whole 16³ block before descent; the same
  algorithm ports directly to the compute pass. Cost is independent of scene depth
  and content complexity.
- **Negative:** connectivity is not line-of-sight, so visible-through-a-gap blocks
  survive and cost overdraw; open terrain gets little culling; the 128 m horizon
  leaves the 128–512 m LOD range frustum-only; the fill is still a per-frame pass
  (0.5 ms) with a GPU/CPU parity obligation at M5.
- **Explicit trade:** over-inclusion is accepted for guaranteed no holes and a flat
  fill cost. Line-of-sight accuracy is deferred to the optional M2 ray-test rather
  than paid for in M1.
- **Invariants:** one bit per block (8 B at 64³, 64 B at 128³); default horizon
  128 m; no false negatives; beyond-horizon is not occlusion-culled; no CPU readback;
  the bit is tested at `BLOCK_LEVEL`; `occlusion.raytest` stays off by default; the
  chunk edge is never tuned to compensate for culling.
- **Verification:** M1-33 (enclosed cave occluded, open field not, no visible block
  occluded across 100 poses); M1-34 (granularity ratio per pose); M1-39 (gate
  report); M2-25/M2-26 (flag parity and the documented delta); M5-01 (bit equality
  with the reference on 100 poses); M5-13 (block skip); M5-22 (draw-work reduction).

## Rejected alternatives

- **SVDAG ray-test as the M1 primary occluder.** It buys line-of-sight accuracy the
  M1 gate does not yet require, at the cost of a ray per test and a duplicate
  traversal, before the raster path has a reference culler. It stays the M2
  escalation, behind a flag.
- **Frustum-only culling.** Cheapest, but it does no occlusion at all: caves and
  structures keep drawing their hidden interiors, and the M5 draw-work reduction has
  no basis.
- **Per-voxel occlusion rather than per-block.** One block is 4,096 voxels; filling
  or testing at that resolution multiplies the horizon work by 4,096 for detail the
  block-level mesher and light already discard. Block level is the natural cut
  (spec §4.2).
- **Chunk-size tuning to improve the cull rate.** A smaller chunk does not make a
  connectivity fill see line-of-sight; it only pays more chunk overhead. The
  pre-registration makes granularity a reason to reject 128³, not a reason to move
  the edge for occlusion.
- **A 512 m fill horizon.** Four times the horizon is 64× the cells at 1 m and is not
  affordable per frame; the 8 km view is a separate far-field mesh, and the LOD rings
  are frustum-tested beyond 128 m.

## Status of open questions

None. The primary occluder is fixed. M2 decides only whether the ray-test earns a
default; M5 decides only whether the GPU path beats the CPU cull, and records that in
an amendment rather than reopening this decision.
