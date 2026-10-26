# ADR-005: Probes — Per-Block Near, Half-Res Mid, None Far

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** none; this is the first probe decision. Records the scheme fixed by
spec §7 and `voxel-core::content_constants` (M0-07).
**Related:** spec §2, §7, §8, §12, §13.5; [`../pre-registration.md`](../pre-registration.md)
§3; ADR-002 chunk edge; ADR-004 light; ADR-009 LOD; M0-07, M1-30, M1-38, M2-18…M2-21,
M6-11

## Context

The renderer has no real-time shadows. Lighting is direct block light, vertex AO and
one indirect term: probes (spec §7). Probes are baked on the content timeline and
stored with the chunk, so edits and streaming — not frames — own their cost.

A uniform per-block probe grid over the whole view spends the two things that are
not free: bake rays and per-edit updates. The contribution of indirect light falls
with distance, while LOD simplification and fog flatten what a far probe could add.
The spec therefore fixes three densities: per-block near, half-resolution mid, none
far.

This is not a memory decision. The near shell costs **~18.9 MiB** of SH payload at
either chunk edge (25,736 × 768 B at 64³; 3,216 × 6 KiB at 128³), under 1% of the
3,275 MiB target. What changes with distance is how many rays are cast and how many
probes an edit must touch. The ring radii are fixed world distances — 128 m and
256 m — matching the Default LOD0/LOD1 boundaries. They are not retuned per profile;
at Minimum the 64 m LOD0 ring sits inside the 128 m near probe ring, which is a
quality surplus rather than a dependency.

## Decision

**Probes are multi-resolution: one SH4 RGB8 probe per block inside 128 m, one per
2 m cell between 128 m and 256 m, and none beyond 256 m.**

| Ring | Outer radius (XZ) | Spacing | Probes at 64³ | Probes at 128³ | SH payload at 64³ / 128³ |
| --- | ---: | ---: | ---: | ---: | ---: |
| Near | 128 m | 1 m (per block) | 64 | 512 | 768 B / 6 KiB |
| Mid | 256 m | 2 m (per 2³ blocks) | 8 | 64 | 96 B / 768 B |
| Far | — | — | 0 | 0 | 0 |

Past 256 m there is no probe volume; shading uses direct light and vertex AO only.

1. **The probe record is SH4 RGB8:** four coefficients × RGB, 8 bits per channel,
   12 B. Centres are block centres near and 2 m cell centres mid. A validity flag is
   carried per probe (1 packed bit: 8 B at 64³ near, 64 B at 128³ near; 1 B / 8 B
   mid). The pre-registered 768 B / 6 KiB row is the SH payload; the mask is reported
   in the M1 memory breakdown.

2. **Storage follows the chunk's current LOD.** A near chunk allocates the per-block
   grid, a mid chunk the half-res grid, a far chunk nothing. Probe memory is a content
   volume: it persists with the chunk (M6-11) and is freed on eviction. The grid is
   edge-independent; 64³ and 128³ differ only by the block count.

3. **The bake is deterministic.** Each probe casts 64 hemisphere rays over the SVDAG
   and projects to SH4. The seed derives from chunk position, probe index and LOD —
   never from time or thread. Ten runs at 1 and 4 threads must produce the same bytes
   (M1-30). Validity is computed in the same pass from the occupancy of the probe
   centre.

4. **Invalid-in-solid fallback.** A probe whose centre lies in a solid block is
   flagged invalid; its SH payload is ignored. Sampling first uses the containing cell
   in the sample's owning chunk. If any corner of that cell is invalid, the sample
   falls back to the nearest valid probe in the 3×3×3 neighbourhood around the sample,
   clamped to the chunk's own grid; equal distances break by lowest linear probe index.
   If the neighbourhood has no valid probe, the probe contribution is zero and shading
   uses direct light plus vertex AO. No path returns a NaN. The search never mixes LODs
   and never depends on a neighbour chunk's probe residency.

5. **Validity is maintained by the same edit pass that schedules the values.** A
   solid→air edit makes the probe valid and schedules its bake; air→solid invalidates
   it immediately. Edits rebake only the probes whose rays reach the dirty region, not
   the whole grid. The previous probe volume stays bound until the new one is uploaded
   and flipped.

6. **A probe LOD change is a content event.** Ring membership is per chunk, using
   horizontal distance from the camera to the chunk AABB: ≤ 128 m near, ≤ 256 m mid,
   otherwise far. Crossing a radius queues a probe LOD change (spec §12); the chunk
   reallocates and fully rebakes at the new spacing, then flips. It never runs on the
   frame timeline, never reads back, and never samples across two grids. Targets
   coalesce: if the radius is recrossed before the rebuild starts, only the latest
   target is built, so a transition rebuilds once.

7. **Budget and gate.** Probe update p95 ≤ **4 ms per edited chunk**
   ([`../pre-registration.md`](../pre-registration.md) §3), with the same budget
   applied to the 128³ projection. Full-view bake is amortised across the content
   timeline, not charged to a frame. Probe bytes are part of the per-chunk memory
   gate; determinism, the invalid flag, the LOD transition and the budget are
   M1-30/M1-38 and M2-19…M2-21 checks.

## Consequences

- **Positive:** bake and sampling cost fall with distance while the near ring keeps
  the detail that is visible. The far ring pays zero. Probe work is edge-independent
  at equal loaded volume: 64 × 25,736 ≈ 512 × 3,216 ≈ **1.65 M** near probes, so the
  LOD scheme does not bias the chunk-edge gate. No probe update runs per frame, and
  the bake is reproducible byte-for-byte.
- **Negative:** the ring boundary is a hard density change. There is no cross-LOD
  interpolation, so a seam is accepted; probes are the lowest-frequency lighting term
  and direct light plus vertex AO carry the surface, which keeps it small. A mid-ring
  probe cannot resolve contact detail. The 3×3×3 fallback adds a lookup on invalid
  cells, and an all-solid neighbourhood degrades to direct light.
- **Explicit trade:** this is not a memory optimisation — the near shell's ~18.9 MiB
  is noise against 3,275 MiB. It spends rays where they are visible and keeps edits
  bounded. A full 128³ near rebake is 512 probes × 64 rays = **32,768 rays**, eight
  times a 64³ chunk; an edit touches only the dirty subset, but the p95 is re-checked
  on the 128³ projection.
- **Invariants:** 12 B SH4 RGB8; 1 m / 2 m spacing; 128 m / 256 m radii; 768 B / 6 KiB
  near, 1/8 of that mid, zero far; validity flag persisted with the volume; LOD changes
  only on the content timeline; no per-frame probe update and no readback.
- **Verification:** M1-30 (deterministic bake, invalid flag), M1-38 (update p95),
  M2-18…M2-21 (production bake, fallback, LOD transition, budget), M6-11 (persistence
  at the stored LOD).

## Rejected alternatives

- **Per-voxel probes.** 262,144 probes per 64³ chunk = **3 MiB**; 2.1 M per 128³
  chunk = **24 MiB**: 4,096× the near grid and the same multiplier in bake rays.
  Sub-block shape is AO's job, and the art target does not need it.
- **Per-block probes past 128 m.** Probe bytes are not the constraint; the waste is
  64-ray bakes and edit updates on chunks that are LOD-simplified and fogged. The
  128–256 m band is 3× the near disc's area, so per-block there would be 3× the near
  probes and 8× the mid probes and rays.
- **Half resolution everywhere.** It removes contact detail in the near ring, the only
  place a 1 m probe is clearly visible, and buys nothing at the far end where there
  are no probes.
- **No probes.** Direct light plus vertex AO has no directional bounce; with no
  real-time shadows, probes are the only indirect term carrying sky and bounce.
- **Cross-LOD interpolation or blended rings.** It requires both grids resident, a
  blend weight and a second sampling path. Per-chunk LOD with a content-event rebuild
  is simpler, deterministic and cheap enough; the seam is accepted.
- **Static bake at load.** Edits are the product. A probe volume that cannot follow a
  dug wall or a placed torch is wrong on the first edit.
- **Per-frame or screen-space GI as the primary path.** Temporal noise, frame-coupled
  cost and no deterministic bytes; it also adds a second lighting path beside the
  content bake. Probes are content, not frame.
- **Full raytraced GI.** Far outside the frame and content budgets, and it replaces a
  scheme that already fits the no-real-time-shadow target.

## Status of open questions

None. Radius, spacing, record, fallback and transition are fixed. M1-30 and M1-38
measure the bake and the update budget; a probe p95 miss kills 128³ by the
pre-registration, but it does not reopen the resolution scheme.
