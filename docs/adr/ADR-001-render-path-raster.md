# ADR-001: Render Path — Raster

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the GPU-raymarch-as-primary-path position in
[`../reference/adr-three-tier-store-v2.md`](../reference/adr-three-tier-store-v2.md)
**Related:** spec §2, §4.3, §5, §6, §7, §8, §11; ADR-003 atlas; ADR-004 light;
ADR-010 mesh format; ADR-012 traversal

## Context

The sparse store is an SVDAG. The renderer must turn that store into a 1080p image at
60 fps (45 fps 1% low) on the reference machine, with 12.0 ms budgeted for draw and
present and no per-frame GPU→CPU readback (spec §8, [`performance-targets.md`](../performance-targets.md)).
Two paths were viable: traverse the DAG per pixel (raymarch), or extract exposed
surfaces on the content timeline and rasterise them.

The spec already fixes the supporting decisions: block-level light plus vertex AO,
one material atlas, GPU-driven traversal and indirect draw. The render path must name
how those pieces meet, not leave a second primary renderer possible.

## Decision

**The primary and only near-field render path is rasterisation of greedy quads.** The
path is four specced components, with the mesher at its head: it feeds the other three
by writing AO into the vertices, culling at emit, and producing the block/quad output
that traversal appends for indirect draw.

1. **AO-safe greedy mesher** (spec §6). It consumes the 16³ block plus the 1-voxel
   apron and emits indexed quads. Merge key is `(materialID, face orientation, 4 × AO)`;
   a difference in any field splits the quad. The output contract is **16 B per
   vertex, no UV attribute, 76 B per indexed quad** (ADR-010).
2. **Vertex AO** (spec §6, §7). Four `u8` corner values are sampled in the apron
   *before* the merge decision, stored as vertex attributes and included in the merge
   key. This is the sub-block lighting detail that makes block-resolution light
   acceptable.
3. **Backface culling at emit** (spec §6). Only the outward-facing quad of an exposed
   voxel face is emitted; interior and inward faces are never indexed.
4. **Indirect draw** (spec §4.3, §11). GPU traversal appends surviving blocks to the
   append buffer; the draw consumes its indirect arguments. Production does not issue
   per-chunk CPU draws.

The mesher's material IDs address the single 256² atlas (ADR-003); tile-local UVs are
derived per fragment from world position, never interpolated. Shading combines the
atlas, per-block light and probes (spec §7). **The SVDAG remains the culling, lighting
and picking structure; it is not sampled per pixel.**

**Raymarch is deferred, not removed from consideration forever.** It is not built in
M0/M1 and is not a fallback for a missed frame budget. It may return after M2 only as
a flag for far-field silhouettes or shadow rays, and then only if it beats the raster
alternative on measured cost and quality. Promoting it to the primary path requires a
new ADR that supersedes this one.

## Consequences

- **Positive:** hardware rasterisation supplies depth, early-Z, MSAA/TAA and a stable
  per-frame cost. Greedy meshing amortises shading over merged quads. The culling
  traversal produces an append buffer that maps directly onto an indirect draw.
- **Negative:** mesh build, upload and AO sit on the content timeline and therefore
  contribute to the 8 ms edit-latency budget. Mesh memory dominates the per-chunk
  budget: **150–600 KB at 64³ and 600 KB–2.4 MB at 128³** ([`../pre-registration.md`](../pre-registration.md)).
  Quantisation, index reduction and near-ring LOD are the first memory levers.
- **Explicit trade:** per-pixel DAG detail is given up. Sub-voxel shape stays blocky;
  AO, atlas detail and light carry the surface. The 32 km view is a separate
  raster heightfield mesh (M3.5), not an extension of the voxel raster.
- **Invariants:** no CPU readback of visibility; atlas IDs stay dense in `0..=255`;
  the M1.5 vertex format and positional UV derivation freeze before M2.
- **Verification:** one textured chunk at 60 Hz in M1.5; 256 chunks at 60 Hz in M3;
  indirect draw in M5; mesh bytes per chunk recorded by the M1 gate.

## Rejected alternatives

- **SVDAG raymarch as the primary path.** Per-pixel pointer chasing is incoherent,
  cannot reuse hardware depth or antialiasing, and makes the 12.0 ms draw budget a
  function of scene depth and empty-space distribution. It also duplicates culling,
  lighting and picking inside the shader instead of reusing the traversal in §11.
  This is the decision the historical three-tier ADR got wrong.
- **Hybrid: raymarch near, raster far.** Two shading paths, two AA histories and two
  culling representations for no measured quality requirement. Rejected for M0/M1;
  the far field is already a raster mesh.
- **GPU-side remeshing or compute rasterisation.** Adds a second producer of geometry
  before the CPU content pipeline is stable. Deferred; no task in M0–M5 depends on it.
- **Per-voxel rasterisation.** Rejected with the lighting decision (ADR-004): the
  geometry and texture cost buy detail the art target does not need.

## Status of open questions

None. The render path is closed; the M1 gate decides chunk edge, not raster versus
raymarch. A missed frame budget follows the sacrifice ladder, not a renderer rewrite.
