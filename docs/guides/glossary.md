# Glossary

Short definitions for the terms that show up in the specs, reports and the other
guides. Each entry links to the page that owns the detail.

## Space and scale

**Voxel** — the smallest cell, **6.25 cm**. A block face is 16 × 16 voxels.
Voxels hold texture detail and wear; the renderer draws blocks.

**Block** — **1 m**, made of 16³ voxels. The unit the player edits and sees.

**Chunk** — a cubic group of blocks. At the target edge it is **128³ voxels =
8 m**; the older baseline was 64³ = 4 m. Chunks are the unit of storage, meshing
and streaming.
See [spec §1](../specs/voxel-engine-architecture-spec.md).

**Chunk edge / `CHUNK_BITS`** — the number of voxels per chunk side. It is a
compile-time parameter, not a scattered literal. `CHUNK_BITS = 7` means 128³.
See [ADR-002](../adr/ADR-002-chunk-edge.md).

**LOD** — level of detail. LOD0 is full-detail voxels near the camera; LOD1 and
LOD2 are coarser rings further out; beyond that the macro heightfield takes over.

**Near shell** — the band of chunks around the terrain surface that stays
resident, rather than a full 12 km vertical column.
See [spec §13.5](../specs/voxel-engine-architecture-spec.md).

**Cave cache** — the extra chunk box streamed when the player goes below the
surface, evicted on ascent.

**Far field** — the distant landscape drawn from the 128 m macro heightfield out
to 16–32 km, with a blend seam at 512 m.

**Macro map** — the 128 m-per-cell world map created once per seed: height,
temperature, humidity, erosion and flags, stored in 16 km tiles.

## Storage

**SVDAG** — sparse voxel directed acyclic graph. A tree (with shared subtrees)
that stores only non-empty space. It is the chunk's storage format.

**Node** — one 8-byte internal tree node: an 8-bit child mask, a level, flags,
and a 48-bit base index into the next level. A full set of eight children is one
64-byte cache line.
See [ADR-011](../adr/ADR-011-node-layout.md).

**Run** — the eight sibling nodes under one parent, stored contiguously.

**Band** — one contiguous append-only arena per tree level. Pointers stay stable
because appends go to the end.

**Root** — the single node index at the top of a chunk's tree.

**A/B roots** — two roots per chunk. Edits write the inactive one; a fence and a
version flip publish it. This is how editing never races the GPU.
See [ADR-007](../adr/ADR-007-ab-roots.md).

**COW / copy-on-write** — an edit copies only the path from the changed leaf to
the root, sharing everything else. It costs about one run per level, not a full
chunk copy.

**Hash-consing** — deduplicating identical runs through a shared pool. Two
parents share a run only if their whole ordered child set matches.

**`versionIndex`** — the per-chunk bit that says whether A or B is current. The
GPU reads it once per traversal.

**Overlay** — the per-chunk layer of recent edits, kept separate from the base
tree until consolidation.

**Compaction** — renumbering a band to reclaim dead or padded space. It
invalidates pointers, so it is forbidden while the chunk is GPU-resident.

## Meshing and materials

**Meshlet** — a small mesh piece. The design target is block-granular 16³
meshlets; the M3 slice still draws one mesh per chunk.

**Greedy meshing** — merging adjacent exposed faces into larger quads, which
cuts triangle count dramatically.

**Merge key** — the tuple that decides whether two faces can merge:
`(material, face direction, 4 × corner AO)`.

**Apron** — a one-voxel border sampled around a chunk so border faces and AO
agree with the neighbouring chunk instead of treating the edge as air.

**Backface culling** — emitting only the outward-facing side of an exposed
voxel face, never both.

**AO / ambient occlusion** — a per-vertex darkening in corners and creases. It
is baked during meshing and is part of the merge key.

**Material atlas** — one 256 × 256 texture holding every material in a 16 × 16
grid of 16 × 16-pixel tiles, so there are 256 slots.
See [ADR-003](../adr/ADR-003-material-atlas.md).

**Tile** — one atlas cell, covering one block face (1 m). One texel is one voxel
face (6.25 cm).

**Vertex format** — the packed per-vertex data. The current gate format is a
packed **8-byte vertex**, or **44 bytes per quad** including indices.
See [ADR-010](../adr/ADR-010-mesh-granularity-vertex-format.md).

## Light and visibility

**Block light** — one light cell per block, not per voxel. At 128³ that is 512
cells = 8 KiB per chunk.
See [ADR-004](../adr/ADR-004-light.md).

**Probe** — a sample of indirect irradiance. One per block near the camera,
half-resolution in the mid ring, none far out. 512 probes = 6 KiB at 128³.
See [ADR-005](../adr/ADR-005-probes.md).

**Frustum culling** — rejecting whole chunks whose box is outside the camera's
six view planes.

**Occlusion culling** — rejecting chunks hidden behind other geometry. The
primary design is a block-level flood-fill; a ray-test is a flagged fallback.
See [ADR-006](../adr/ADR-006-occlusion.md).

**Broad phase** — the coarse pass that tests chunk roots and compacts the
survivors into a candidate list.

**Traversal** — the GPU pass that walks a chunk's tree, applying frustum,
occlusion and LOD, and appends visible blocks.

**Indirect draw** — a draw whose arguments are produced on the GPU, so the CPU
does not know or care how many chunks survived culling.

**Draw call** — one submitted draw. The M3 slice uses one per visible chunk
(93 of 256 at the default pose); M5 replaces that with indirect draws.

**Present** — handing the finished frame to the swapchain. On the RTX 4060 the
frame is often **present-bound**, not geometry-bound.

## Streaming

**Ring** — the 16 × 16 = 256-chunk resident set that follows the camera.

**Residency** — the set of chunks the engine chooses to keep resident for a view
profile. The gate's model is `N(edge, R, V)`.

**Pump** — the frame-thread step that recentres the ring and installs finished
resources under a **4 ms** budget. Measured max: 0.09 ms.

**Worker** — a background thread that generates and meshes chunks.

**Preparer / uploader** — the background stage that reserves pool space and
writes finished mesh bytes off the frame thread.

**Mesh pool / arena** — one paged GPU buffer (8 MiB pages) that every chunk mesh
suballocates from, so the frame thread never creates a buffer after warm-up.

**Staging belt** — the reusable mapped buffer used to copy bytes into the pool.

**Upload queue** — a byte-budgeted FIFO that limits how many mesh bytes the
frame copies (currently **512 KiB/frame**).

**Backlog** — finished meshes waiting to be uploaded. Capped at 32 MiB.

**Hitch** — a frame that exceeds its budget. A hitch over 16 ms fails a 60 Hz
profile.

## Generation

**Seed** — the integer that deterministically selects a world.

**Integer-hash noise** — noise computed with integer hashes and 16.16
fixed-point interpolation. It is bit-exact across platforms; no `libm` is
involved.

**Deterministic** — the same seed produces the same bytes on every platform and
thread count. Golden chunk hashes enforce it in CI.

**Priority-flood** — the hydrology step that fills depressions to find lake
surfaces.

**D8 flow** — each cell drains to its lowest of eight neighbours; used to build
flow direction and accumulation.

**Flow accumulation** — the upstream area draining through a cell. Large values
become rivers.

**Biome weights** — a softmax over biome centres, blended over a 300–500 m band
instead of a hard border.

**Softmax** — a smooth normalisation that turns distances into weights summing
to 1. It is why biome borders fade rather than snap.

**Ecotone** — a border habitat. A few species (birch, scrub) appear only between
biomes.

**Transition block** — a canonical two-material-plus-debris block used at a
material boundary. It reuses parent materials, so it costs no atlas slot.

**Structure anchor / landmark anchor** — deterministic grid cells that may hold
a prefab structure or a landmark. Most structure cells are empty by hash.

**Macro tile** — a 16 km × 16 km piece of the macro map: 125 × 125 cells,
93.75 KB raw. The world has 4,096 of them.

## Measurement

**p50 / p95 / p99** — percentiles. The p95 is the value at or below which 95% of
samples fall; it is used instead of the mean so a heavy tail is visible.

**1% low** — the average frame rate of the slowest 1% of frames, inverted. It
includes the worst frame, unlike a p99 over 100 samples.

**Frame time** — the interval between two presents. It is what the player feels.

**VRAM target / hard fail** — 40% of device memory (3,275 MiB) is the target;
60% (4,913 MiB) is the hard fail. The gate still multiplies by a 1.5× safety
factor.

**Pre-registration** — the pass/fail table written and frozen **before** the
measurement. It stops the finish line from moving after the result is known.

**Gate** — a milestone's pass/fail measurement against that table.

**Profile** — a named view configuration (Minimum, Default, High) with its own
resolution, far clip, near radius, shell and LOD rings.

**Reference machine** — the RTX 4060 / Ryzen 5 7600 / 32 GB machine every gate
number belongs to. A run elsewhere is not a gate run.

**Synthetic fixture** — a generated “save” used before a real gameplay save
exists. It is labelled provisional and cannot close the dataset row.

**Provisional** — a measurement made on a synthetic fixture or an unconfirmed
build. It is evidence, not a green light.

**Erratum** — a dated correction appended to a report. Archived runs are
append-only; the correction never rewrites the original.
See [the M2 erratum](../milestones/m2/m2-exit.md).
