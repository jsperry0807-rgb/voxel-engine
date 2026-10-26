# Voxel Engine Architecture Spec

**Status:** v2.2 — four gates closed; chunk edge parameterized and pre-registered
**Date:** 2026-10-25
**Companion:** [`voxel-engine-roadmap.md`](voxel-engine-roadmap.md)

**Changes in v2.2**

- **Chunk edge is a parameter**, not a constant. M1 builds at **64³**; **128³** is the
  pre-registered upgrade target, gated by the table in the roadmap's M0 (§9.1).
- **Render path: raster, closed.** No raymarch in M0/M1; revisit only for far-field
  silhouette or shadow rays in M2+.
- **Occlusion: block-level flood-fill primary in M1; SVDAG ray-test deferred to M2**
  behind a flag. Not either/or (§4).
- **Light: one cell per block (16³), 16 B/cell**, sub-block detail via vertex AO.
  512 cells at 128³, 64 at 64³ (§7).
- **Probes: multi-resolution** — per-block near, half-res mid, none far (§7).
- Level numbering, bands, AABB and early-outs are now expressed in `CHUNK_BITS`,
  never in literals.

---

# Bulleted List

- **Three-Tier Storage Hierarchy** (GPU rendering / CPU editing & authoritative state / disk persistence) backed by a single **Hierarchical Sparse Volumetric Store** implemented as an **SVDAG**
- **Batched COW consolidation** using **per-chunk double-buffered roots with atomic version-index flip** over one shared, hash-consed node pool (GPU-safe, no mid-frame races)
- **Level-order (BFS) node bands**, 8 siblings contiguous, **8 B internal node**, a full sibling run = **one 64 B cache line** (§10)
- Textures — **one always-resident 256×256 material atlas**: 16×16 grid of 16×16 px tiles = **256 material slots**; `tileIndex = materialID`, tile-local UV from face coordinates (§6)
- **6.25 cm voxel** (16 voxels/m)
- **Blocks are 16×16×16 voxels** (= 1 m³)
- **Chunks are `2^CHUNK_BITS` blocks per edge**: **64³ voxels** (4³ blocks = 64 blocks, 4 m) is the M1 baseline; **128³ voxels** (8³ blocks = 512 blocks, 8 m, 512 m³) is the pre-registered target (§1, §9.1)
- **AO-safe binary Greedy Mesher** (same-material merge, AO + face orientation in merge key, backface culled at quad emit, no vertex UV)
- **Flood-fill direct light at block resolution** + **multi-resolution probe GI** + **voxel ambient occlusion** (per-chunk content pipeline, recomputed on COW dirty regions)
- **Sparse-set ECS** carrying per-chunk lifecycle (`READY`, `EDITING`, `PENDING_CONSOLIDATION`, `UPLOADING`, `EVICTING`) — visibility is GPU-side and never read back (§5, §12)
- **GPU-driven hierarchical frustum + block-level flood-fill occlusion embedded in SVDAG traversal** — one workgroup per chunk, frontier-synchronous, groupshared frontier, indirect dispatch (§11)
- **Raster rendering** of greedy quads: the mesher, vertex AO, backface cull and indirect draw are the pipeline; SVDAG remains the culling, lighting and picking structure

---

# Notes: Details That Must Be In the Spec

## 1. Voxel / Block / Chunk Math

### 1.1 Fixed

- 1 voxel = 0.0625 m → **16 voxels/m**
- 1 block = 16³ voxels = **1 m³** = **256 voxel columns per face** (16 × 16)
- `BLOCK_DIM = 16`, `BLOCK_BITS = 4`, `BLOCK_LEVEL = CHUNK_BITS − 4`

### 1.2 Chunk edge is a parameter

| Property | 64³ baseline (M1) | 128³ target |
|---|---|---|
| `CHUNK_BITS` | 6 | 7 |
| Voxels per edge | 64 | 128 |
| Blocks per edge | 4 | 8 |
| Blocks per chunk | 64 | 512 |
| Chunk extent | 4 m | 8 m |
| Chunk volume | 64 m³ | 512 m³ |
| Split levels (`ROOT_LEVEL`) | 6 (64 → … → 1) | 7 (128 → … → 1) |
| Block level | 2 (16³) | 3 (16³) |
| Leaf level | 6 (1³) | 7 (1³) |
| Node bands | 7 (levels 0–6) | 8 (levels 0–7) |

Level 0 is the chunk root. `ROOT_LEVEL = CHUNK_BITS`. A node at level *l* covers
`2^(CHUNK_BITS − l)` voxels per axis. The block boundary is always
`CHUNK_BITS − 4`, which is the natural cut for block-level occlusion, flood-fill
early-out, per-block ECS entities if added, and block-granular remeshing.

**No literal 64, 128, 6 or 7 appears in engine code outside the constants module
and its compile-time assertions.** Every formula below is written in `CHUNK_BITS`.

### 1.3 What changes with the edge

- Edit latency, per-chunk consolidation tail, per-chunk scratch and per-chunk mesh
  memory scale with chunk volume; chunk count at fixed view distance falls by 8×
  per edge doubling. Memory at fixed *loaded volume* is therefore roughly neutral;
  the forces that get worse are latency and granularity.
- The atlas, block size, probe spacing and AO are edge-independent.
- The 128³ upgrade is a `CHUNK_BITS` change plus a re-run of the M1 gate. It is not
  a rewrite, which is why the parameter is locked now.

## 2. Storage Tier Boundaries

- **GPU tier holds:** both roots per chunk (A/B) into the shared node pool, the
  single 256×256 material atlas, the probe grid at the chunk's current probe LOD,
  per-chunk `versionIndex`, per-frame culling state (block occlusion bits, chunk
  candidate list, frontier, append buffers, indirect args), frustum/occlusion
  buffers. AO is **not** here: it is a vertex attribute (§7).
- **CPU tier holds:** authoritative sparse-set ECS (lifecycle only), edit journal,
  host-side COW staging buffers, level-band arenas, LOD/streaming decisions,
  dirty-run set.
- **Disk tier holds:** per-chunk file = node bands + leaf band + header (root,
  `CHUNK_BITS`, version, level table, compression flag) + material IDs (the atlas
  is global, rebuilt from the registry) + block light cells + **dirty-region
  journal** so reopening does not require a full recompute.
- Rule: **the GPU tier is never authoritative.** Any GPU-resident state must be
  reconstructible from CPU + disk. Visibility is the designed exception: it is a
  per-frame render fact, lives only on the GPU, and is never used for correctness
  on the CPU (§12).

## 3. COW Consolidation Protocol

Per chunk, maintain **two roots (A/B)** into one shared, hash-consed pool, plus a
single `uint32 versionIndex` (0 = A, 1 = B). The pool is shared across chunks; the
A/B pair duplicates only the path nodes the chunk actually changed (§10.3).

1. **Edit phase (CPU):** edits mutate the chunk's COW path, producing a new root.
   Only the path from each modified leaf to the root is duplicated; untouched runs
   are shared by hash-consing. One edited path costs one new run per level
   (`ROOT_LEVEL` runs, ~64 B each, §10.4).
2. **Batch commit:** all chunks with a dirty root at the frame boundary are appended
   to their **inactive** root slot in one staging pass.
3. **Fence:** a single GPU fence is issued for the batch. The CPU must **not** flip
   `versionIndex` for any chunk in the batch until that fence signals.
4. **Flip:** on fence signal, write `versionIndex = 1 − versionIndex` for each chunk
   in the batch, then set lifecycle `READY` (traversal resumes on the new root).
   Visibility state is untouched by the flip.
5. **Slot recycling:** the vacated root cannot be overwritten until all GPU work
   that read it has completed. Track `inFlightFrame` per root; safe when
   `currentFrame − inFlightFrame >= 2`. With two roots that permits a flip every
   two frames; if measured completion lag is three frames, add a third root rather
   than weakening the rule.
6. **In-flight read consistency:** the traversal workgroup reads
   `versionIndex[chunk]` **exactly once at workgroup start** and uses that root for
   the whole traversal. Never re-read mid-traversal.

**Coordination surface:** the CPU schedules commits from the ECS; the GPU schedules
traversal from its own compacted chunk list. Neither reads the other's per-frame
state (§12).

## 4. Culling Traversal

### 4.1 Frustum

- **Bounding volume:** `AABB = [origin, origin + 2^(CHUNK_BITS − level) × voxelSize]`,
  with `origin` the node's world-space origin along the traversal path. Never a
  node-local AABB: SVDAG runs are shared, so placement, not content, defines the box.
- **Test:** conservative plane-vs-AABB against all 6 frustum planes. Cull only when
  entirely outside at least one plane. If the parent was fully inside all planes,
  skip the test for its children.
- **Ordering:** octants are visited near-to-far by camera-to-octant-centre distance
  when emitting the next frontier. Approximate front-to-back, not a full sort.
- **Depth cutoff:** stop at `depth == lodMaxDepth` or at a leaf. LOD is a cutoff of
  this same traversal, not a separate structure.
- **Early-out at block level (`CHUNK_BITS − 4`):** if all 8 block children are
  culled or occluded, emit nothing.
- **Emission:** surviving blocks are appended to an instance buffer. Never draw from
  inside the traversal kernel.

### 4.2 Occlusion (locked)

- **Primary occluder: block-level flood-fill.** A dedicated pass floods 6-connected
  from the camera's block through non-opaque blocks over a bounded horizon and
  writes **one visibility bit per block** (1 bit/block: 8 B per 64³ chunk, 64 B per
  128³ chunk).
- The traversal consults the bit at `BLOCK_LEVEL` and skips the entire 16³ block in
  one test. A chunk whose blocks are all invisible aggregates to `OCCLUDED`.
- **Conservative by construction:** the fill may over-include (visibility flows
  through any 1 m gap and around corners) but never under-includes. It composes
  with frustum culling and does not replace it.
- **Horizon:** default 128 m; beyond it, frustum only. Tunable. The fill cost is
  `O(horizon³)` at 1 m cells: ~2 M cells at 128 m, which is affordable per frame;
  512 m is not, which is why the horizon exists.
- **Semantics to pin in M0:** this is a *connectivity* fill, not a line-of-sight
  test. In open terrain it will cull little; its value is enclosed pockets, caves
  and built structures. If the M1 granularity gate demands more, the escalation is
  the ray-test, not a chunk-size change.
- **SVDAG ray-test: deferred to M2 behind a flag.** It is the accuracy fallback for
  far-field and the shadow-ray path. Not required for M1.

### 4.3 Dispatches

1. **Occlusion fill** — block grid, camera-seeded, bounded horizon, writes
   visibility bits.
2. **Broad phase** — one workgroup per resident chunk tests the chunk root against
   the frustum, compacts survivors into the candidate list.
3. **Traversal** — indirect, one workgroup per candidate chunk; frustum + block
   occlusion bits + LOD cutoff; appends surviving blocks.
4. **Draw** — indirect, consuming the append buffer.

Dispatch 2 must not be launched one-workgroup-per-world-chunk: culled chunks would
occupy idle workgroups.

## 5. State: Two Orthogonal Axes

**Visibility (GPU only, per frame, never read back):**

| State | Meaning |
| --- | --- |
| `FRUSTUM_VISIBLE` | Passed the frustum; survives until occlusion resolves |
| `OCCLUDED` | Passed the frustum, rejected by block-level flood fill |
| `CULLED` | Rejected by frustum |

**Lifecycle (CPU ECS, event-driven):**

| State | Traversal eligible? | Meaning |
| --- | --- | --- |
| `READY` | Yes | CPU and GPU agree; roots and meshes current |
| `EDITING` | No | Being mutated on the CPU |
| `PENDING_CONSOLIDATION` | Yes (reads old root) | Dirty root awaiting batch commit |
| `UPLOADING` | No | Inactive root written, fence not yet signalled |
| `EVICTING` | No | Leaving the resident set |

The sparse set keeps the **lifecycle** set dense, so commit and streaming loops
iterate only live chunks. A chunk with a dirty root stays traversable on its old
root until the flip, which is what keeps edits from stalling rendering.

## 6. AO-Safe Binary Greedy Mesher and the Material Atlas

**Mesher**

- **Merge key** = `(materialID, face orientation, 4 × per-vertex AO)`. Same
  material, same facing, same corner AO → merge. If any differ, quads split.
- **AO** is sampled on the prospective quad's shared corners before the merge
  decision, from the 1-voxel apron, so chunk borders agree. Stored as 4 × `u8`.
- **Backface culling at quad emit:** emit only the outward-facing quad of each
  exposed voxel face, never both.
- **UVs are not vertex data.** The atlas tile is selected by material ID and the
  tile-local UV is derived per fragment from the world position within the block
  (§6.1). Greedy quads spanning several blocks tile correctly because the
  derivation is positional, not interpolated.

**6.1 Atlas (locked)**

- One **256×256** atlas, always resident, one binding: a **16×16 grid of 16×16 px
  tiles = 256 material slots**.
- `tileIndex = materialID`; tile origin = `(tileIndex % 16, tileIndex / 16) × 16 px`.
- A tile covers **one block face (1 m)**. With 16 voxels per block edge, each voxel
  face reads **one texel** (6.25 cm) — a block-scale texture whose damage pattern is
  revealed voxel by voxel.
- UV: `uv = (tileOrigin + fract(worldPosOnFace / 1 m) × 16 + 0.5) / 256`, half-texel
  inset to reduce edge bleed, using the two axes perpendicular to the face normal.
- **Mips capped at level 4** (16×16 tile → 1×1 tile) so no mip averages across
  tiles. Cross-tile bilinear bleed at the coarsest levels is known and bounded; if
  it shows, pad tiles to 18×18 or 20×20 in a 320² atlas rather than re-enabling
  deeper mips.
- **Consequences, stated deliberately:** 256 material slots is a hard cap; per-face
  texture variation is expressed as material variants (grass_top / grass_side),
  each consuming a slot; texel density is 6.25 cm per voxel face. The trade buys
  trivial residency, one binding, no array limits and no per-chunk texture
  ownership.

**6.2 Registry impact**

`MaterialId` stays `u16` in the DAG but must be dense in `0..=255` for atlas
addressing. Freeze asserts the cap. `TextureSetId`'s six-face indirection collapses:
a face-textured block registers its variants as separate materials.

## 7. Lighting / GI Pipeline (per-chunk content timeline, not per-frame)

- **Direct light: one cell per block (16³).** 16 B/cell (RGB + intensity, or SH2):
  **64 cells = 1 KiB per 64³ chunk**, **512 cells = 8 KiB per 128³ chunk**.
  Flood-fill propagation runs over block adjacency across chunk borders with a
  1-block apron. Per-voxel light is rejected on cost (2.09 M cells per 128³ chunk);
  per-4³-cluster is rejected as needless complexity.
- **Sub-block light is vertex AO**, already produced by the mesher. This is the
  whole reason block-resolution light is acceptable.
- **Per-block vs per-2-block is a quality/cost dial in M2**, not an architecture
  gate.
- **Probes: multi-resolution.** Per-block near (1 m spacing, 12 B SH4 RGB per
  probe: 768 B per 64³ chunk, 6 KiB per 128³ chunk), half-res mid (2 m), none far.
  Ring radii are set in M0. A probe whose centre is in a solid block is marked
  invalid; shading falls back to the nearest valid probe in the 3×3×3 neighbourhood.
  Probe storage is allocated at the chunk's current probe LOD; changing LOD is a
  content event, not a frame event.
- **Voxel AO:** per-vertex, computed during meshing, part of the merge key, stored
  in vertex attributes — not a texture and not a per-chunk volume.
- All of these run on the **content timeline**, before the chunk returns to `READY`.
- **Budget:** probe update p95 ≤ 4 ms per edited chunk.

## 8. Sync Points (hard rules)

- **Consolidation commits only at a frame boundary** — never mid-frame.
- **No traversal of `EDITING`, `UPLOADING` or `EVICTING` chunks.** The broad phase
  reads ECS lifecycle state from a CPU-written table uploaded once per frame; it
  does not read back.
- **Frame timeline:** occlusion fill → broad phase + compact → per-chunk traversal
  (frustum + occlusion bits + LOD) → append → indirect draw.
- **Content timeline:** edit → COW runs → upload bands → fence → flip → block light
  → mesh/AO → probes → `READY`. The atlas is static and never participates.
- **Slot recycling tracks `inFlightFrame` per root**, not a global frame counter,
  because async compute can lag compute-queued traversal by one frame.
- **No per-frame GPU→CPU readback.** Visibility results never leave the GPU (§12).

## 9. Decision Status

**Closed**

| Decision | Resolution |
| --- | --- |
| Chunk edge | **64³ in M1; 128³ pre-registered upgrade**, gated by §9.1 |
| Render path | **Raster.** No raymarch in M0/M1; far-field/shadow revisit in M2+ |
| Occlusion | **Block-level flood-fill primary (M1); ray-test deferred (M2, flagged)** |
| Light resolution | **Per-block (16³) + vertex AO**; 512 cells at 128³, 64 at 64³ |
| Atlas | One 256×256 atlas, 16×16 tiles, 256 slots, UV from material ID |
| Probes | One per block near, half-res mid, none far |
| Node layout | Level bands, 8 B nodes, 8-sibling 64 B runs |
| Traversal | Workgroup-per-chunk, frontier-synchronous, groupshared |
| State ownership | GPU per-frame visibility, CPU event-driven lifecycle, no readback |

### 9.1 Chunk-edge pre-registration

The gate lives in the roadmap's M0 and is written **before** M1 measures. Summary:

- VRAM budget for voxel data: **40% of total VRAM** (a fraction, not an absolute).
- Gate: measured `p95 per-chunk memory × target loaded chunk count ≤ 40% VRAM × 1.5`.
- Also pass/fail at 64³: edit latency p95 ≤ **8 ms**, probe update p95 ≤ **4 ms**,
  culling granularity ≤ **15%** traversed-but-invisible at boundaries.
- Dataset: a real populated save; report median, p95 and max per chunk. Synthetic
  chunks are not admissible.
- **Fallback trigger:** any budget failure at 64³ → stay at 64³. No renegotiation
  inside M1. If 128³ is greenlit, the upgrade is a `CHUNK_BITS` flip in M2.

## 10. Node Layout (locked)

### 10.1 Node

- **8 B internal node**, 64-bit word: `mask: u8` (bits 0–7), `level: u3` (bits 8–10),
  `flags: u5` (bits 11–15), `base: u48` (bits 16–63).
- `mask` bit *c* set means child *c* is present. `base` is the index of the first
  child slot in the next level band.
- **Children are compacted**: child *c* lives at
  `base + popcount(mask & ((1 << c) − 1))`. Absent children cost nothing.
- **Leaf payloads are `u16` material IDs** (0 = air = absent), eight per leaf run =
  16 B. "8 B/node" applies to internal levels `0..ROOT_LEVEL`.
- A chunk stores one root node index; `ROOT_LEVEL` splits beneath it.

### 10.2 Bands

- Each level is a **contiguous band** in its own append-only arena;
  `levelStart[ROOT_LEVEL + 1]` is uploaded once per version. Build emits root →
  leaves (BFS), so a freshly built chunk is strictly level-order.
- Appends go to the end of the owning band, so pointers are stable and no
  relocation is needed. After the first edit a band is "level-ordered,
  append-ordered" rather than strictly BFS.
- A full sibling run (mask `0xFF`) is **64 B aligned**, so its 8 nodes occupy
  exactly one cache line. Partial runs are 8 B aligned and packed; padding is
  bounded by 56 B per full run and is measured in M1.
- **Compaction** (dead/padded bytes > 25%) renumbers a band and invalidates every
  pointer above it. It may only run on a chunk not referenced by either GPU root or
  any in-flight frame, and it bumps the chunk version. Never compact a
  GPU-resident chunk in place.

### 10.3 Sharing

- Hash-consing operates on **runs**, keyed by `(level, mask, ordered child ids)`,
  not on individual nodes.
- Two parents share a run only if the entire ordered child set matches. A single
  child cannot be shared independently of its siblings.
- Consequence: COW duplicates one run per level of the edited path —
  `ROOT_LEVEL` runs, up to 64 B each (384 B at 64³, 448 B at 128³) — reclaimed by
  the epoch retire. This is the price of the cache-line layout and is cheaper than
  a second full chunk copy.

### 10.4 Why this layout

- One fetch per sibling group: traversal reads 64 B and has all 8 children, no
  pointer chase.
- 8 B per internal node is denser than the previous variable `[header, children…]`
  `u32` encoding for typical masks.
- The `u48` base bounds the pool at 2^45 runs; far beyond any world this engine can
  stream.

## 11. Traversal Execution (locked)

- **One workgroup per chunk.** Workgroup = 128 threads = 4 warps.
- **Frontier-synchronous:** process one level of the chunk's tree at a time. The
  frontier of candidate nodes for the current level lives in **groupshared
  memory**; threads test, compact survivors into the next frontier,
  `workgroupBarrier()`, repeat.
- **Warp-per-node:** each warp claims one frontier node (lane 0 broadcasts
  mask/base). Lanes 0–7 each handle one child: lane *c* reads node
  `base + popcount(mask & ((1<<c)−1))`, a single 64 B coalesced fetch. Lanes 8–31
  idle or prefetch the next node. Lanes ballot survivors and lane 0 appends them
  to the next frontier.
- **Frontier capacity:** 1 024 nodes = 8 KiB groupshared, current and next
  frontier. Overflow policy: emit overflow nodes as conservative visible blocks
  (over-draw, never under-draw) and count it per chunk.
- **AABB:** from the node's world origin and level; the warp keeps the origin in
  registers and derives children by adding `2^(CHUNK_BITS − level − 1) × voxelSize`
  per set axis bit.
- **Version read:** `versionIndex` is read once by lane 0 at workgroup start and
  broadcast. The whole traversal uses that root.
- **Broad phase** is a separate dispatch writing the compacted candidate list;
  traversal is indirect-dispatched over it.
- **Load balance:** cost scales with surviving nodes; the indirect dispatch keeps
  empty chunks free.

## 12. State Ownership and Readback (locked)

- **GPU per frame:** block occlusion bits, chunk candidate list, per-chunk
  frontiers, visible-block append buffer and counter, indirect argument buffers,
  `versionIndex` table, per-chunk visibility bit.
- **CPU (authoritative, event-driven):** ECS sparse set of chunk entities with
  `pos`, `lifecycle`, `root_version`, `mesh_version`, `last_touched_frame`,
  streaming priority. Updated only on events: edit applied, consolidation
  committed, fence signalled, chunk loaded/unloaded, camera ring crossed, probe LOD
  change.
- **No per-frame readback.** The CPU never learns per-frame visibility; streaming is
  camera-predictive, not visibility-reactive.
- **Diagnostics only:** a stats readback every ≥ 30 frames, off the frame path,
  never consumed by correctness logic.
- **Consequence:** culling cannot be gated on CPU-known visibility, which is why §5
  splits the two axes. Chunks in `PENDING_CONSOLIDATION` remain traversable on their
  previous root; only `EDITING`, `UPLOADING` and `EVICTING` are excluded, via the
  CPU-uploaded lifecycle table.
