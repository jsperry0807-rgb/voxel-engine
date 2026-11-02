# Architecture tour

This is the “how does it fit together?” page. It follows one edit from the
player to the GPU, then zooms out to the world. For exact rules, follow the
links into [`specs/`](../specs) and [`adr/`](../adr).

## 1. Three sizes, one grid

| Thing | Size | What it is |
|---|---|---|
| Voxel | 6.25 cm | The smallest cell; a block face is 16 × 16 of them |
| Block | 1 m | 16³ voxels; the unit the player edits and sees |
| Chunk | 8 m | 128³ voxels = 512 blocks on the target edge |

The **chunk edge is a parameter**, not a magic number. The M1 gate measured
64³; the 128³ *Minimum* profile was greenlit for M2 and is the target. Engine
code names the edge once, in `CHUNK_BITS`, and derives everything else from it.
The 64³ product edge was later abandoned, but 64³ remains the fast test build.

Why this matters: at the same view distance, doubling the chunk edge cuts the
chunk count by 8×. Bigger chunks are cheaper to stream but slower to edit, so
the edge was a measured decision rather than a preference.

See [spec §1](../specs/voxel-engine-architecture-spec.md) and
[ADR-002](../adr/ADR-002-chunk-edge.md).

## 2. A chunk is a tree that skips empty space

A chunk is stored as a **sparse voxel tree** (an SVDAG). Imagine a filing
cabinet where a drawer only exists if something is inside it. A solid stone
chunk compresses to a few nodes; a cave costs only the nodes around its walls.

The tree has one level per halving of the chunk. Each **internal node is 8
bytes**: a mask saying which of its eight children exist, plus a base index into
the next level. Children are packed, so an absent child costs nothing. A full
set of eight siblings is laid out as **one 64-byte cache line** — one memory
fetch gives the traversal all eight children.

In numbers at 128³:

- 8 levels (root at level 0, 1³ voxels at level 7).
- 8 B per internal node; leaves are `u16` material ids.
- The gate's hard cap is 1.5 M shared nodes per chunk; the measured maximum was
  138,636.

See [spec §10](../specs/voxel-engine-architecture-spec.md) and
[ADR-011](../adr/ADR-011-node-layout.md).

## 3. Editing without stopping the frame

The GPU may be reading a chunk while the player edits it. The engine cannot
mutate that tree in place without risking a half-written frame, and it refuses
to stop the world to wait.

So each chunk has **two roots, A and B**, over one shared node pool. An edit
copies only the path it touches — that is **copy-on-write** — and writes the new
path into the inactive root. At a frame boundary the CPU uploads the change,
waits for one GPU fence, then flips a single `versionIndex` from A to B. The
traversal reads that index once at the start and uses the same root for the
whole traversal.

The old root is recycled only after every frame that could have read it has
finished. Two roots are enough for a flip every two frames; if measurement ever
showed a three-frame lag, the rule is to add a third root, not to relax the
wait.

See [spec §3](../specs/voxel-engine-architecture-spec.md) and
[ADR-007](../adr/ADR-007-ab-roots.md).

## 4. One texture for every material

There is exactly one **256 × 256 material atlas**, always resident. It is a
16 × 16 grid of 16 × 16-pixel tiles, so there are **256 material slots**. A
material's id *is* its tile index; there is no per-chunk texture ownership and
no texture array.

Each tile covers one block face (1 m). Because a block face is 16 × 16 voxels,
one texel is one voxel face (6.25 cm). That is how a single small atlas still
shows block-scale detail — including wear revealed voxel by voxel.

The trade is deliberate: 256 slots is a hard cap, so variants such as
`grass_top` and `grass_side` consume slots. Mips stop at level 4 so they never
average across tiles.

See [spec §6](../specs/voxel-engine-architecture-spec.md) and
[ADR-003](../adr/ADR-003-material-atlas.md).

## 5. Turning blocks into triangles

The mesher only emits **exposed** faces. Two neighbouring stone blocks hide the
face between them, so it is simply never created. It then **greedily merges**
neighbouring faces that look the same.

“Look the same” is the merge key:

```
(material, face direction, 4 × corner ambient occlusion)
```

If any of those differ, the quad splits. AO is sampled from a one-voxel
**apron** around the chunk, so a quad at a chunk border agrees with its
neighbour and the seam does not appear.

Two other rules keep the mesh small:

- **Backface culling at emit:** only the outward face, never both sides.
- **No UVs in the vertex.** The atlas tile comes from the material id, and the
  tile-local UV is computed in the fragment shader from the world position. A
  greedy quad spanning several blocks therefore still tiles correctly.

At 128³ the gate mesh is **44 bytes per quad** (packed 8 B vertex). The M2
production mesh measured 3,650 KiB p95 against a 2,400 KiB cap; the M3 packed
path brought it to **2,115 KiB**, which is why mesh compression was the first M3
task.

See [spec §6](../specs/voxel-engine-architecture-spec.md) and
[ADR-010](../adr/ADR-010-mesh-granularity-vertex-format.md).

## 6. Light without a per-voxel flood fill

Lighting is split into three cheap pieces:

- **Block light:** one cell per block, not per voxel. At 128³ that is 512 cells
  (8 KiB). It flood-fills across chunk borders and is updated incrementally for
  dirty regions.
- **Ambient occlusion (AO):** baked per vertex during meshing. It gives the
  soft contact shadow in corners without a light volume at voxel resolution.
- **Probes:** a small grid of irradiance samples for broader indirect light —
  one per block near the camera, half-resolution further out, none far away.

The rejected alternative was per-voxel light: 2.09 M cells per 128³ chunk. The
combination above buys most of the look for a fraction of the cost.

See [spec §7](../specs/voxel-engine-architecture-spec.md),
[ADR-004](../adr/ADR-004-light.md) and [ADR-005](../adr/ADR-005-probes.md).

## 7. Two kinds of state, never mixed

There are two independent axes, and confusing them is the classic voxel-engine
bug:

**Visibility** is a per-frame GPU fact: frustum, occlusion, and what to draw. It
is never read back to the CPU. The CPU does not need to know what was visible
last frame, and waiting for that answer would stall the pipeline.

**Lifecycle** is CPU state, changed only by events: `READY`, `EDITING`,
`PENDING_CONSOLIDATION`, `UPLOADING`, `EVICTING`. The CPU owns it; the GPU
receives a compact lifecycle table once per frame.

A chunk being edited stays drawable on its old root until the flip. Only
`EDITING`, `UPLOADING` and `EVICTING` chunks are excluded from traversal.

See [spec §5 and §12](../specs/voxel-engine-architecture-spec.md) and
[ADR-008](../adr/ADR-008-visibility-lifecycle.md).

## 8. Streaming a 12 km world

Holding a full vertical column would be absurd: 12 km of chunks above and below
the player. Instead the engine streams a **surface-relative shell** — a band
around the terrain surface — plus a **cave cache** when the player goes
underground.

The near ring is 128 m with a ±16 m vertical shell (about 3,200 chunks at 128³),
then coarser LOD rings at 256 m and 512 m, then the macro heightfield far field.
The 256-chunk M3 slice is the measured proof of the mechanism, not the final
view distance.

See [spec §13.5](../specs/voxel-engine-architecture-spec.md) and
[Rendering and streaming](rendering-and-streaming.md).

## 9. What is deliberately *not* here

The design closes some doors on purpose:

- **No raymarching.** The render path is raster. A ray-test exists behind a flag
  for occlusion, but it is not the primary path.
- **No per-voxel light.**
- **No real-time shadows at launch.** Sun, AO, block light and probes only.
- **No fluid simulation.** Water is a static body with a visual current.
- **No GPU readback per frame.**
- **No invisible wall at the world edge.** The last 20–50 km is storm and ocean.

Each of these is a decision record, not an oversight. Start at
[`adr/`](../adr) if you want the reasoning.

## Where to go deeper

- [Architecture spec](../specs/voxel-engine-architecture-spec.md) — the full
  normative design.
- [Roadmap](../specs/voxel-engine-roadmap.md) — why the work is ordered this way.
- [Worldgen tour](worldgen-tour.md) — how the terrain gets made.
- [Rendering and streaming](rendering-and-streaming.md) — how a frame works.
- [Performance and measurement](performance-and-measurement.md) — the gates.
