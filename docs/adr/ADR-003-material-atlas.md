# ADR-003: Material Atlas — 256², 16×16 Tiles, Capped Mips

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the "texture array, not atlas" position and the 256²-per-face texture resolution in
[`../reference/material-spec-v1.md`](../reference/material-spec-v1.md)
**Related:** spec §1.1, §6, §6.1, §6.2; ADR-001 raster, ADR-010 mesh format;
`voxel-core::atlas`; M0-06, M1.5-06…M1.5-12

## Context

Rasterisation and the mesh format remove the UV attribute. ADR-001 and ADR-010 fix a
**16 B vertex with no UV**, and greedy quads merge within a 16³ block (ADR-010); an
interpolated per-vertex UV would stretch one tile across a merged quad. Texture
identity must therefore travel as a material id, and the tile-local UV must be
recomputed per fragment.

The alternatives were a texture array of 256² slices per material or one atlas. The
array avoids tile-edge bleeding but needs a slice index per face, a six-face
indirection and content-dependent array limits. The atlas needs one binding and one
material id; spec §6.1 locks it, and `voxel-core::atlas` encodes its constants and
compile-time invariants. The cost is density: a 1 m face is 16 voxels wide and a tile
is 16 texels wide, so each voxel face reads **exactly one texel — 6.25 cm**. The
historical material spec assumed 256² per face, or 3.9 mm per texel. This ADR records
that 16× linear-detail trade.

## Decision

**One always-resident 256×256 material atlas: a 16×16 grid of 16×16 px tiles, 256
slots, `tileIndex = materialID`, per-fragment UV and mips capped at level 4.**

1. **Layout and residency.** The atlas is 256×256 RGBA8 (BC7 on the GPU), built once
   at startup, never streamed or versioned and never owned by a chunk. Including the
   capped mip chain it is about **85 KiB**, and it is the only material binding the
   raster path uses.

2. **Addressing.** `tileIndex = materialID`; tile origin is row-major,
   `((id % 16) × 16, (id / 16) × 16)`. `MaterialId` stays `u16` in the DAG, but the
   registry freeze asserts ids are dense in `0..=255` and the 257th registration
   fails. Air is id 0 and never samples the atlas, so the 256 tiles leave at most
   **255 drawable** material variants.

3. **Per-face variation is material variants.** A material does not carry six texture
   indices. `grass_top`, `grass_side` and `dirt` are separate materials, each
   consuming one tile and shared by every block that uses them. A block resolves its
   six faces to variant material ids before meshing; a uniform block resolves all six
   to one id. The old `TextureSetId` six-face indirection into a texture array
   collapses into material ids. The resolved variant id enters the merge key, so
   different variants never merge.

4. **UV derivation.** UVs are not vertex data. The fragment selects the tile from the
   material id and derives the tile-local coordinate from the position on the block
   face:

   ```text
   uv = (tileOrigin + clamp(faceFrac × 16 + 0.5, 0.5, 15.5)) / 256
   ```

   `faceFrac` is the 0..1 position along the two axes perpendicular to the face
   normal, taken from the interpolated world position **relative to the block
   origin** — the same fraction as `fract(worldPosOnFace / 1 m)` without f32's
   precision loss at 1,024 km. The half-texel term puts samples on texel centres and
   keeps a bilinear level-0 sample off the neighbouring tile. Because the derivation
   is positional, a greedy quad spanning several voxel faces still samples the
   block-face tile correctly.

5. **Mip cap.** The atlas has exactly five levels: base plus `MAX_MIP = 4`. At level
   4 the whole atlas is 16×16 and each tile is 1×1. The builder must not generate
   level 5 or deeper, because it would average texels from different tiles.
   Cross-tile bilinear bleed at the coarsest levels is known and bounded; if it shows,
   the pre-registered fix is to pad tiles to 18×18 or 20×20 in a 320² atlas and
   recompute the origins, **not** to re-enable deeper mips.

## Consequences

- **Positive:** one small always-resident texture, one binding, no array limits and no
  per-chunk texture ownership. The atlas never enters COW, upload, eviction or the
  edit-latency path. The 16 B vertex survives with no UV to store, and greedy quads
  tile correctly.
- **Negative:** the 256-slot cap is hard and content-visible; every distinct face
  variant spends a slot. Texel density is 6.25 cm per voxel face, so close-up detail
  is block-scale and coarse mips may bleed slightly across tile edges. The historical
  256²-per-face detail is not recoverable.
- **Explicit trade:** 16× linear texture density is exchanged for trivial residency,
  positional UVs and no UV attribute. AO, light and probes carry the detail that
  texels no longer do.
- **Invariants:** no UV vertex attribute; `tileIndex = materialID`; ids dense in
  `0..=255`; every tile 16×16 and mip-aligned; exactly `MAX_MIP + 1 = 5` levels; no
  tile crosses a mip boundary; the atlas is built once and is never per-chunk. The
  literals 256, 16 and 4 live only in `voxel-core::atlas`.
- **Verification:** `cargo test -p voxel-core atlas` (origins, UV bounds, half-texel
  inset, mip cap, const context); M1.5-06 (256 tiles, 5 levels); M1.5-07
  (reproducible downsample); M1.5-08 (257th registration fails); M1.5-12 (one 1 m face
  shows exactly one 16×16 tile); M1.5-16 (texel-density screenshots).

## Rejected alternatives

- **Texture array of 256² slices per material.** The historical choice. It avoids
  tile bleed but needs six slice indices per block, an array binding and per-material
  residency. A 100-material six-face set is about 39 MB BC7, against about 85 KiB for
  the whole atlas, for detail the 6.25 cm art target does not use.
- **Per-vertex UVs.** Four to eight bytes per vertex, a second interpolation path and
  UV seams at block boundaries; greedy quads need per-block repetition anyway, which
  positional derivation provides for free.
- **Deeper mips.** Level 5 and below mix neighbouring tiles, so a sampled material
  changes with distance. The cap is a correctness boundary, not a quality dial.
- **Pad tiles immediately (18×18 or 20×20 in 320²).** It costs 25–56% more atlas area
  and complicates the row-major origins for bleed that is not yet measured. It stays
  the fallback if level-4 bleed shows.
- **Several atlases or per-chunk textures.** Extra bindings, ownership and a second
  upload path for no content requirement.
- **A larger atlas for more slots.** 512² quadruples the resident texture and hides
  the slot discipline that keeps variants shared; slot pressure is a content choice,
  not a memory purchase.

## Status of open questions

None. Layout, addressing, UV derivation and the mip cap are closed. Slot pressure is
governed by the freeze cap, and the only named follow-up — visible coarse-mip bleed —
has a measured trigger and a fixed fallback, so it cannot reopen the decision.
