# ADR-004: Light — Per-Block Cells + Vertex AO

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the per-voxel light volumes in
[`../reference/material-spec-v1.md`](../reference/material-spec-v1.md) and
[`../reference/adr-three-tier-store-v2.md`](../reference/adr-three-tier-store-v2.md)
**Related:** spec §1.1, §2, §6, §7, §8; [`../pre-registration.md`](../pre-registration.md) §3;
[`../performance-targets.md`](../performance-targets.md) §5; ADR-001 raster;
ADR-005 probes; ADR-010 mesh format; `voxel-core::content_constants`;
M0-07, M1-29, M2-07…M2-17

## Context

A block is 16³ voxels, one cubic metre; a chunk is 64³ or 128³ voxels. The atlas
gives each block face one 16×16 tile, so a voxel face reads one 6.25 cm texel, and
the mesher merges at block granularity. Light has to pick a grid on the same scale,
and that grid is not free: it is a per-chunk content volume that is persisted, rebuilt
on edit and read by shading on the content timeline.

The two historical reference documents inherited per-voxel light from the 6.25 cm
era. At 64³ that is 262,144 cells and **4 MiB per chunk**; at 128³ it is 2.09 M cells
and **32 MiB per chunk** (spec §7). Per-block light is 64 cells and **1 KiB per 64³
chunk**, 512 cells and 8 KiB at 128³. The decision is therefore not a small quality
dial: it changes light memory and flood-fill work by **4,096×**, against an 8 ms edit
budget and a 3,275 MiB voxel-memory target.

The spec already fixes one 16 B cell per block and vertex AO as the sub-block
detail. This ADR records why that pair is the resolution, what the cell contains,
and why the two rejected finer grids lose on measured cost rather than taste.

## Decision

**Direct light is one 16 B cell per 1 m block; sub-block detail is vertex AO. There is
no per-voxel or per-4³-cluster light volume.**

1. **One cell per block, 16 B.** A 64³ chunk holds 4³ = **64 cells = 1 KiB**; a 128³
   chunk holds 8³ = **512 cells = 8 KiB**. The cell has two 8 B channels — **sky** and
   **block/emissive** — each `RGB + intensity` in 16-bit fixed point. It is not SH2:
   direct light is not directional; directional GI is the probe grid's job (ADR-005).

2. **Flood fill over block cells.** Both channels propagate across block adjacency
   with a **1-block apron** (16 voxels) at chunk borders. The fill is deterministic
   (fixed traversal order, fixed-point arithmetic), seeded from sky exposure and
   emissive blocks, and attenuated by the material registry. Incremental edits relight
   only the dirty region and escalate to a full-chunk relight when the removal front
   reaches its boundary. It runs on the **content timeline**, before the chunk returns
   to `READY`; never per frame.

3. **Vertex AO supplies sub-block lighting.** Four `u8` corner values per vertex are
   sampled from the **1-voxel apron** before the merge decision — three neighbours per
   corner (two edge, one diagonal) — and stored in the 16 B vertex (ADR-010). The four
   values are part of the merge key `(material, face, 4 × AO)`, so quads split rather
   than interpolate across an AO change. AO is **not** a texture, a volume or a
   separate per-chunk buffer; it adds no storage beyond the vertex attribute already
   budgeted.

4. **Per-block is the finest allowed resolution.** M2 may measure **per-2-block**
   (2 m cells, one eighth of the cells) as the already-specced quality/cost dial.
   The dial only coarsens. A finer grid requires a new ADR that supersedes this one.

5. **Light and AO are content, not frame state.** Cells are persisted with the chunk
   and covered by the dirty-region journal (spec §2). They are rebuilt on the content
   timeline and replayed from the journal on reopen; there is no per-frame relight and
   no GPU→CPU readback. The SVDAG supplies occupancy for propagation but does not
   store light in its nodes.

## Consequences

- **Positive:** light costs **0.8% of the 3,275 MiB target** for the whole Default near
  set — **25.1 MiB at either edge**, because the counts trade against cells exactly
  (25,736 × 1 KiB = 3,216 × 8 KiB). At 64³ it is 1 KiB per chunk against a 60–90 KiB DAG
  and a 150–600 KB mesh, so it is effectively free. A 64-cell flood fill fits the 8 ms edit
  budget with room for COW and upload; AO gives 6.25 cm contact detail at no added
  residency and matches the atlas's block-face texel density.

- **Negative:** light cannot vary inside a block: a torch at a block corner lights the
  whole 1 m block, and a wall face gets one light value plus corner gradients. Light
  seams need the apron, and incremental *removal* is the hard part (M2-15/M2-16).
  AO darkens corners but carries no colour, so coloured sub-block bounce is out of
  scope.

- **Explicit trade:** per-voxel GI is given up. The 6.25 cm read comes from the atlas
  and vertex AO; light carries block-scale colour and intensity, probes carry
  direction. This is the same trade the raster and atlas decisions already make.

- **Invariants:** 16 B/cell; 1 KiB per 64³ chunk, 8 KiB per 128³ chunk; two channels;
  per-block is the finest grid; AO lives in the 16 B vertex and the merge key; light
  and AO are content-timeline products; no per-voxel BFS and no per-frame light.

- **Verification:** light reference and three-case fixtures (M1-29); full-pass,
  apron, incremental and escalation equality (M2-13…M2-17); AO brute-force,
  merge-key and chunk-seam tests (M2-07…M2-09); memory row and edit p95 in the gate
  (M1-35, M1-37); scrub ≤ 4 ms (M2-24); light/AO debug view (M7-09).

## Rejected alternatives

- **Per-voxel light (6.25 cm).** 64³: 262,144 cells, **4 MiB/chunk**, 4,096× the
  per-block volume and flood-fill work. 128³: 2.09 M cells, **32 MiB/chunk**. At
  Default that is **~100.5 GiB** for the near set — **~31× the 3,275 MiB budget** —
  before mesh, DAG or overlay. It does not remove the need for AO (corners still need
  the 3-neighbour test) and it multiplies the dirty-region work that the 8 ms edit
  budget caps. This is the position the reference documents inherited, and the reason
  this ADR supersedes them.

- **Per-4³-cluster light (25 cm).** 64³: 16³ = 4,096 cells, **64 KiB/chunk** (64×
  per-block); 128³: 32³ = 32,768 cells, **512 KiB/chunk**. The Default near set pays
  **~1.57 GiB at either edge** — 49% of the 3,275 MiB target on light alone. At 64³
  the 64 KiB cell volume alone is on the scale of the whole 60–90 KiB DAG and twice
  the 32 KiB overlay. It also needs a second sub-block grid aligned to 4 voxels and
  64× the propagation work, for detail that AO already supplies at the vertices. The
  spec calls this needless complexity; the numbers agree.

- **Per-2-block light as the default.** It quarters the cells and the fill cost, but
  loses the 1 m light variation that the block-scale atlas and mesher are built
  around. Kept as an M2 quality/cost dial, not as the architecture.

- **SH2 in the 16 B cell.** Two channels already fill 16 B at 16-bit precision. SH2
  adds basis bookkeeping and directional storage to direct light, while the probe
  grid already carries SH4 RGB8 for directional GI. Rejected as duplicated machinery.

- **AO as a volume or texture.** AO must be known at quad-merge time to split the
  merge key, and it is already paid for in the 16 B vertex. A separate AO volume adds
  residency and a second update path for no visual gain.

- **Per-frame relight.** It violates the content timeline, duplicates the block
  flood fill every frame, and turns a 1 KiB/8 KiB content volume into frame work that
  the draw budget does not contain.

## Status of open questions

None. The resolution is closed at one cell per block. The M2 per-2-block measurement
is a coarsening test already specified by the spec, not an open architecture question;
going finer requires a new ADR. The cell's two-channel, 16-bit fixed-point layout and
the AO sampling rule are fixed here for M1-29 and M2-07…M2-17.
