# ADR-010: Mesh Granularity and Vertex Format

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the "greedy quads merge across block boundaries" position in
[`ADR-003`](ADR-003-material-atlas.md); this ADR confines merging to the 16³ block
**Related:** spec §1.1, §1.2, §4.1, §6, §7, §8; [`../pre-registration.md`](../pre-registration.md)
§3; [`../performance-targets.md`](../performance-targets.md) §1.4; ADR-001 raster;
ADR-002 chunk edge; ADR-003 atlas; ADR-004 light; ADR-009 LOD; ADR-012 traversal;
M1.5-10…M1.5-16, M2-01…M2-12, M3-02, M4-10, M5-06, M5-15, M5-17

## Context

The raster path draws greedy quads (ADR-001). The SVDAG supplies the block
visibility that the traversal appends for indirect draw (ADR-012). The chunk edge is
64³ in M1 and 128³ at the gated upgrade (ADR-002); either way, the block is the
16³-voxel, one-cubic-metre unit that light, probes and occlusion already use. The
mesher must choose the unit at which geometry is built, merged, remeshed, culled and
drawn. That unit is not free:

- Mesh memory is the dominant per-chunk cost: **150–600 KB at 64³ and
  600 KB–2.4 MB at 128³**, assuming **76 B per indexed quad**
  ([`../pre-registration.md`](../pre-registration.md) §3). A format that adds a UV,
  a normal, per-vertex light or a second AO path moves every memory row in the gate.
- Edit latency is 8 ms p95. A one-voxel edit must dirty **one block plus its AO/light
  apron**, not a whole chunk, or the tail scales with chunk volume (M2-22, M2-23).
- Occlusion and the M5 append buffer operate at `BLOCK_LEVEL`: the traversal appends
  **surviving 16³ blocks**, and dispatch 3 draws that append buffer. A mesh that
  cannot be culled or drawn a block at a time leaves the block-level work with
  nothing to save.
- AO must agree across block and chunk borders. The merge decision needs the four
  corner values before a quad is emitted, so the mesher needs a one-voxel apron, not
  a guess from the block's own boundary.
- Scratch is capped at **64 KiB per worker** with no chunk-wide densify (M2-03,
  M2-29). A 128³ chunk as `u16` materials is 4 MiB; the block-plus-apron volume is
  11.4 KiB.

Two granularities were viable. A **whole-chunk mesh with greedy merging across
block boundaries** produces the fewest quads, but needs a chunk-scale scratch or a
second pass, makes a one-voxel edit potentially dirty every block a merged quad
crosses, and puts geometry outside the block that the traversal culls. A **per-block
meshlet** with a one-voxel apron has slightly more quads, but keeps build, remesh,
cull and draw at the same 16³ grain. This ADR chooses the latter and fixes the vertex
format that makes it cheap.

## Decision

**The mesh grain is the 16³ block plus a one-voxel apron. A non-empty block emits one
or more indexed meshlets; a quad never crosses a block boundary. The vertex is 16 B
and an indexed quad is 76 B. There is no UV, normal, tangent, colour or light
attribute.**

1. **Block grain and apron.** The mesher samples an 18³ volume: the 16³ block plus a
   one-voxel apron on every side, **5,832 voxels**. The apron supplies boundary face
   visibility and the three neighbours per AO corner. At chunk borders it comes from
   the six 1-voxel face planes of the resident neighbours (M2-01); interior neighbours
   are read through the store. No 130³ dense copy and no chunk-wide scratch is built.
   The 18³ `u16` material scratch is 11.4 KiB; masks, AO and emit buffers keep the
   per-worker total under 64 KiB.

   The six chunk face planes are a read-only chunk-level cache, shared rather than
   copied per worker: **48 KiB at 64³, 192 KiB at 128³**, against 4.2 MiB for a dense
   130³ `u16` volume. They are transient content-timeline data, not resident mesh
   memory.

   A missing neighbour at the edge of the loaded set is read as air. The chunk still
   meshes, so the M1.5 single-chunk slice and streaming edge do not stall; the border
   is marked provisional and remeshed when the neighbour arrives. Inside the loaded
   set the apron is exact, so M2-09's AO seam test sees equal corner AO on both sides
   of a chunk border.

2. **Quads never cross a block boundary.** Greedy merging runs within the block, face
   by face, with the merge key `(material, face, 4 × AO)`. A quad may span up to
   16 × 16 voxel faces — one complete block face — but no more. This has four
   consequences that are the point of the decision:
   - one quad maps to exactly one 1 m atlas tile, so positional UV never repeats
     inside a quad and needs no cross-block correction;
   - a block is an independent remesh unit, so one edit dirties that block plus the
     one-voxel AO/light apron, never an unbounded run of merged quads;
   - a block is an independent cull/draw unit, so the visible-block buffer can skip
     it without leaving geometry behind or drawing duplicate geometry;
   - the 18³ scratch is sufficient, so the 64 KiB worker budget survives at 128³.

   The cost is at most one extra quad per shared block face. On terrain that is a
   fraction of a percent of a chunk mesh, because side faces and detail faces
   dominate; M2-08 records the measured merge ratio. **This narrows spec §6's
   illustrative "quads spanning several blocks" to quads spanning several voxel
   faces within one block face.** The positional-UV rule is unchanged.

3. **One meshlet per non-empty block, aggregated per chunk.** A non-empty block emits
   a contiguous vertex/index range and a 16 B record: `first_index: u32`,
   `index_count: u32`, `base_vertex: u32`, `block_index: u16`, `meshlet_count: u16`
   (1 normally). The records and the vertex/index ranges are uploaded with the chunk
   mesh. An empty or fully enclosed block emits no record and no draw.

   A block normally has one meshlet. If a pathological block exceeds the `u16` range
   limit in point 6, it emits `meshlet_count > 1` contiguous sub-meshlets; the first
   record's count is authoritative, it covers the first range, and the next
   `meshlet_count − 1` records are the remaining sub-ranges. A
   `BLOCKS_PER_CHUNK`-entry `u16` `record_index` table (**128 B at 64³,
   1 KiB at 128³**) resolves a block to that record; a sentinel means the block is
   empty. The visible-block item carries the resolved record index, so the draw
   expands the block to one command per sub-meshlet. This is a correctness guard, not
   the expected path. The records and index together are about **1.1 KiB at 64³ and
   9 KiB at 128³** in the normal one-meshlet-per-block case — negligible against the
   mesh; the guard adds another 16 B per extra sub-meshlet.

   The logical resource is per chunk, so the mesh flips with the chunk's root under
   the M4 `VersionedResource` protocol (M4-10). The physical pool — one arena with
   suballocation or per-chunk buffers — is M3-02's decision; it must preserve the
   meshlet ranges and keep allocation off the frame path.

4. **Vertex: 16 B, four-byte aligned.** The byte layout is fixed:

   | Bytes | Field | Type | Meaning |
   |---:|---|---|---|
   | 0–1 | `local.x` | `u16` | chunk-local voxel coordinate, `0..=CHUNK_DIM` |
   | 2–3 | `local.y` | `u16` | chunk-local voxel coordinate, `0..=CHUNK_DIM` |
   | 4–5 | `local.z` | `u16` | chunk-local voxel coordinate, `0..=CHUNK_DIM` |
   | 6–7 | `material` | `u16` | atlas id, dense `0..=255`; air is never emitted |
   | 8 | `face` | `u8` | `0..5` = `+X, -X, +Y, -Y, +Z, -Z` |
   | 9–12 | `ao[4]` | `u8 × 4` | the quad's four corner AO values, `0..=255` |
   | 13–15 | reserved | `u8 × 3` | zero; do not read or repurpose without a new ADR |

   `size_of::<Vertex>() == 16` and `align_of::<Vertex>() == 4` are compile-time
   assertions. The GPU binding is three attributes with stride 16:
   `Uint16x4` at offset 0 (`local.xyz`, `material`), `Uint8x4` at offset 8 (`face`,
   `ao[0]`, `ao[1]`, `ao[2]`), and `Uint8x4` at offset 12 (`ao[3]`, three reserved
   bytes).

   Positions are **chunk-local voxel coordinates**, one unit per 6.25 cm. World
   position is reconstructed as `chunkOrigin + local × VOXEL_SIZE` relative to the
   draw origin; the mesh never stores a world-space `f32` and does not lose precision
   at the 1,024 km world edge. Values above `CHUNK_DIM` are invalid; the upper bits of
   each `u16` stay zero.

5. **AO is four `u8` corner values, replicated on every vertex.** The mesher computes
   the quad's four corner AO values from the apron before the merge decision; the four
   values are the merge key and are copied into every vertex of the quad. A quad's
   four vertices are contiguous, four-aligned and ordered to match `ao[0..4]`,
   indexed `0..3`; the vertex stage selects its own corner with the low two bits of
   the vertex index (the meshlet base vertex is also four-aligned). AO `0` is fully
   occluded and `255` fully open; the fragment interpolates the selected scalar
   between the quad's corners. A difference in any of the four values splits the quad
   rather than interpolating across the change (ADR-004).

   The replication uses bytes that would otherwise be padding in the 16 B vertex, so
   it does not change the 76 B/quad budget. AO is not a texture, a volume or a
   separate buffer. The three reserved bytes are **not** an AO extension or a
   compression flag.

6. **Index buffer: `u16`, six indices per quad, 76 B/quad.** Each quad owns four
   vertices; no vertices are shared between quads. The six indices are
   `(0, 1, 2), (0, 2, 3)` with counter-clockwise winding seen from outside. The total
   is `4 × 16 + 6 × 2 = 76 B` per quad. Index values are **meshlet-local**
   (`0..vertex_count−1`); `base_vertex` supplies the chunk vertex-buffer offset, so a
   `u16` index never has to address the whole chunk. `base_vertex` is a multiple of
   four and `index_count` a multiple of six, so the corner selection in point 5 is
   always valid.

   One `u16` range addresses at most **65,536 vertices / 16,384 quads**. A block that
   exceeds that is split into multiple contiguous sub-meshlets (point 3). `u32`
   indices are not used: they would raise the quad to 88 B (+16%) to solve a
   pathological case that a range split handles for free.

7. **No UV, normal, tangent, colour or light attribute.** The atlas tile is selected
   by `material`, and tile-local UV is derived per fragment from the position on the
   block face (ADR-003). `face` supplies the normal and the two face axes; voxel faces
   are flat, so there is no normal or tangent to interpolate. Block light is sampled
   from the 16 B block cell and directional GI from the probes at the fragment; neither
   is repeated per vertex. These omissions are what keep the vertex at 16 B and the
   quad at 76 B.

8. **Draw contract.** The traversal appends surviving blocks, not per-quad geometry
   (ADR-012). Each visible block maps to its record; the draw reads the chunk's table
   and issues one indexed range per recorded meshlet (normally one). M3's per-chunk
   draw is a correctness stopgap that draws every meshlet in the chunk; M5's indirect
   draw consumes the visible-block buffer and culls individual blocks. No per-frame
   CPU mapping, no readback, and no per-frame mesh rebuild is introduced.

9. **Determinism and LOD.** Block order is linear `block_index`; inside a block the
   mesher scans faces and slices in a fixed order, so the quad list and meshlet bytes
   are deterministic across thread counts and chunk order (M2-11, M2-12). LOD0 is this
   block meshlet with full voxel-resolution geometry. LOD1/LOD2 aggregate 2³/4³ blocks
   per node under ADR-009; their explicit mesh representation is measured at M6 and is
   not frozen here.

## Consequences

- **Positive:** build, remesh, cull and draw all share the 16³ grain. One edited block
  is the unit of dirty work; one visible block is the unit of draw work. Scratch is
  bounded by 18³, not by the chunk volume, so the 64 KiB worker budget holds at both
  edges. A quad lies inside one block face, so one atlas tile covers it exactly and
  positional UV needs no cross-block repetition. AO seams are exact inside the loaded
  set because the apron is exact. The vertex is a fixed 16 B and the quad a fixed
  76 B, matching every pre-registered memory row.
- **Negative:** greedy merging stops at the block boundary, so the mesh carries up to
  one extra quad per shared block face. Material, face and the four AO values are
  replicated across each quad's four vertices; the format spends bytes on simplicity
  rather than minimum encoding. A pathological block can need several `u16` ranges.
  The 16 B vertex is uncompressed, and mesh memory remains the dominant per-chunk
  cost: **150–600 KB at 64³ and 600 KB–2.4 MB at 128³**.
- **Explicit trade:** block independence, bounded scratch and a direct mapping to the
  visible-block buffer are bought with a small merge loss and a redundant but simple
  vertex. Compression, vertex sharing and per-meshlet constants are deferred levers,
  not part of the baseline the gate measures.
- **Invariants:** `Vertex` is 16 B with 4 B alignment; 4 vertices + 6 `u16` indices =
  76 B/quad; no UV, normal, tangent, colour or light attribute; `local` is chunk-local
  and `0..=CHUNK_DIM`; `material` is dense `0..=255`; `face` is `0..5`; AO is four
  `u8` values and part of the merge key; no quad crosses a block boundary; one
  meshlet record per non-empty block (sub-meshlets only for the `u16` range guard);
  `base_vertex` is 4-aligned and `index_count` a multiple of 6; 18³ block scratch and
  no chunk-wide densify; mesh and table flip with the root; no per-frame CPU mesh
  work.
- **Verification:** M1.5-10 (6 quads for an isolated voxel, 0 enclosed) and M1.5-11
  (buffer sizes match the quad count); M2-01/M2-02 (apron equals the dense read and
  beats it on bytes); M2-03/M2-29 (≤ 64 KiB/worker, no densify); M2-04…M2-08
  (reference match, merge key, backface, AO, merge-ratio cost); M2-09 (chunk-border
  AO seam); M2-10 (16 B packing and round-trip); M2-11/M2-12 (golden hashes and
  thread/order determinism); M3-02 (pool preserves meshlet ranges); M3-11 (memory
  rows); M4-10 (mesh and root flip together); M5-06/M5-15/M5-17 (indirect args,
  visible-block append, indirect draw).

## Rejected alternatives

- **Whole-chunk mesh with greedy merging across block boundaries.** The fewest quads,
  but a 128³ chunk as `u16` is 4 MiB, so the scratch violates 64 KiB; a one-voxel edit
  can change a merged quad that reaches across many blocks, so dirty work is
  unbounded; and a cross-block quad belongs to no single block, so block-level culling
  must either cull visible geometry or draw duplicates. The saved quads are at most
  one per shared block face, a fraction of a percent of a terrain chunk. This is the
  position ADR-003's phrasing implied and this ADR supersedes.
- **One per-chunk mesh with a single draw range.** It still remeshes the whole chunk
  on a one-block edit, cannot skip interior blocks in M5, and at 128³ exceeds a `u16`
  index range in normal content. Per-chunk drawing is kept only as the M3 stopgap.
- **Per-voxel or per-face meshlets.** 4,096 voxels per block and up to 24,576 exposed
  faces in a pathological block; the draw and metadata overhead swamps the greedy
  saving. The block is the natural cut the rest of the pipeline already uses.
- **Per-2³ or per-4³ meshlets.** Coarser than the block for no culling gain, or finer
  than the block and therefore outside the lighting, occlusion and remesh grain. LOD1
  and LOD2 already provide coarser aggregation at M6.
- **`u32` indices.** 88 B/quad, +16% on the dominant per-chunk memory, to avoid a
  range split that only pathological blocks need. `u16` plus a split is sufficient.
- **Non-indexed six vertices per quad.** 96 B/quad, +26%, and no vertex reuse at all.
  Indexed quads at 76 B are the pre-registered baseline.
- **Per-vertex UV.** Four to eight bytes per vertex, UV seams at block edges, and no
  benefit once the atlas tile is selected by `material`. A quad inside one block face
  needs no per-vertex UV at all.
- **Per-vertex normal or tangent.** Voxel faces are flat; the `face` byte supplies the
  normal and the two face axes. A normal attribute is pure overhead.
- **Per-vertex light or colour.** Block light is a 16 B cell and directional GI is the
  probe grid; repeating either per vertex adds bytes and loses the block/probe
  resolution. The fragment already has what it needs.
- **An 8 B or 12 B packed vertex.** The logical fields plus four AO bytes do not fit
  without quantising position and AO, changing the merge key and adding unpack cost
  before the gate. Compression is a post-gate lever, measured against the 76 B
  baseline, not a silent format change.
- **Material and face as per-meshlet constants.** It would save replication bytes but
  add a per-quad side buffer and a second lookup path. The 16 B vertex keeps the quad
  self-describing; a per-meshlet constant is a later compression option.
- **Sharing vertices between adjacent quads.** It lowers vertex count but complicates
  indexing, breaks the four-vertex/four-aligned corner rule and makes the golden mesh
  order content-dependent. Quad-owned vertices are deliberate.
- **Separate GPU buffers per block.** Up to 512 objects per chunk at 128³, with
  per-object allocation and draw overhead. Meshlet ranges inside a per-chunk resource
  give the same culling granularity without the object count.
- **AO as a texture or volume.** Rejected in ADR-004: AO must be known before the
  merge and already fits in the vertex; a volume adds residency and a second update
  path.
- **World-space positions in the vertex.** A `u16` world coordinate overflows the
  1,024 km world; a world-space `f32` loses precision at the edge. Chunk-local `u16`
  plus the draw origin is exact.

## Status of open questions

None for the grain or the format. The 16³ block, 18³ apron, 16 B vertex, `u16`
indices, 76 B indexed quad and no-UV/no-normal/no-light rules are closed. The physical
mesh pool is M3-02; vertex/quad compression and explicit LOD mesh formats are measured
later levers, and neither may change the baseline before the gate. A finer mesh grain,
`u32` indices, a packed vertex or a cross-block merge requires a new ADR that
supersedes this one.
