# Documentation

## Normative (read in this order)

| Document | Purpose |
|---|---|
| [`voxel-engine-architecture-spec.md`](voxel-engine-architecture-spec.md) | What the engine is: units, tiers, protocols, layout, traversal, state |
| [`voxel-engine-roadmap.md`](voxel-engine-roadmap.md) | Why the work is ordered this way; milestone gates |
| [`task-list.md`](task-list.md) | Executable breakdown: every task, its dependency, done condition and verification |
| [`worldgen-design.md`](worldgen-design.md) | World scale, macro map, biomes, blending, caves, rivers, far field, water |
| [`pre-registration.md`](pre-registration.md) | **Frozen 2026-10-26.** M1 gate: profiles, budgets, residency model, fallback rules |
| [`reference-machine.md`](reference-machine.md) | The machine every gate number is measured on; gated profiles |
| [`performance-targets.md`](performance-targets.md) | Frame, VRAM, RAM and CPU budgets; sacrifice ladder; CI policy |
| [`generator.spec.md`](generator.spec.md) | Material and texture pipeline: graphs, manifest, bake filter, atlas, material table |
| [`water-textures.md`](water-textures.md) | M3.5-15 water/current faces: flow axis, UV scroll, alpha policy, caustics decision |

When these disagree, the spec wins, then the roadmap, then the task list. Any change to a
binding number is made in the spec and the pre-registration, never only in a task.

## Current position

- **M0 and M1 are complete.** The M1 gate selected the **128³ Minimum** profile for
  M2, conditional on mesh compression and a measured 128³ run.
- **M1.5 is complete.** The viewer, atlas, naive mesher, full M1.5-15
  screenshot set and the M1.5-17 before/after texel-density pair landed. The
  zero-validation CI job (`M1.5-16`) is also in place.
- **M2 is complete.** The apron API (`M2-01`), its cost benchmark (`M2-02`),
  the block-granular driver (`M2-03`), the greedy core (`M2-04`), the merge key
  (`M2-05`), backface culling (`M2-06`), AO sampling (`M2-07`), the AO
  merge-ratio measurement (`M2-08`), the chunk-border AO seam (`M2-09`), the
  16 B vertex codec (`M2-10`), the five golden fixtures (`M2-11`), thread/order
  determinism (`M2-12`), the production full-pass block light (`M2-13`),
  cross-chunk apron seeding (`M2-14`), dirty-region incremental light (`M2-15`)
  and removal-front escalation (`M2-16`) landed. The `M2-17` light cost
  benchmark landed and the optimized path now measures 5.7% p95 at 128³, under
  the 20% target. **M3-00 (mesh compression), M3-01 (streaming shell), M3-02 (mesh
  buffer pool), M3-03 (upload queue), M3-04 (CPU chunk cull), M3-05 (draw
  loop), M3-06 (instrumentation), M3-07 (frame breakdown), M3-08 (stress scene),
  M3-09 (hitch test), M3-10 (60 Hz at the gated edge), M3-11 (memory report),
  M3-12 (visual regression) and M3-13 (the exit) are closed; `m3` is tagged. Two
  conditions are carried out of M3: the Radeon iGPU's 128³ 1% low (56.6–61.2 fps
  against 60) and 128³ being green only at the minimum residency profile (default
  11.93 GiB, high 95.4 GiB). M2.5 runs before M3.5.**
- **M2.5 has started.** `M2.5-01` created `voxel-worldgen` and froze the
  integer-hash, fixed-point noise contract with committed golden vectors; see
  [`m2.5-01-noise.md`](m2.5-01-noise.md). `M2.5-02` added the 128 m macro grid
  and the 16 km tile format (93.75 kB payload, CRC-32, format version 1); see
  [`m2.5-02-macro-tile.md`](m2.5-02-macro-tile.md). `M2.5-03` added the PCHIP
  shape splines: continentalness and erosion to a surface elevation, exact sea
  level at 6,600 m and a 12,000 m peak; see
  [`m2.5-03-splines.md`](m2.5-03-splines.md). `M2.5-04` added the climate
  fields — compressed latitude, an altitude lapse and an ocean-distance
  humidity curve with an 8 km variation; see
  [`m2.5-04-climate.md`](m2.5-04-climate.md). `M2.5-05` added the seeded macro
  tile generator, the `macro` CLI and the aggregated content hash: the whole
  4,096-tile world generates in **9.2 s** with a stable hash; see
  [`m2.5-05-macro-generator.md`](m2.5-05-macro-generator.md). `M2.5-06` added
  priority-flood hydrology with D8 flow and accumulation, plus the `hydro`
  command; see [`m2.5-06-hydrology.md`](m2.5-06-hydrology.md). It surfaced a
  metre/kilometre unit bug in the shape field; the follow-up shape fix added
  continents, per-seed ocean calibration and valley detail, raising the largest
  flow accumulation from 48 cells to **5.7M** and giving 1–2 continents per
  seed. See [`m2.5-shape-fix.md`](m2.5-shape-fix.md). `M2.5-07` extracted the
  river network — 48,722 segments, order 5, 10–200 m widths, 576 deltas, every
  river ending at sea or a lake; see
  [`m2.5-07-rivers.md`](m2.5-07-rivers.md). `M2.5-08` extracted 4,003 lakes
  with flat spill surfaces, spill cells and shorelines, and verified no basin
  leaks; see [`m2.5-08-lakes.md`](m2.5-08-lakes.md). `M2.5-09` added the
  streaming tile store: an LRU cache, a 3 × 3 view in 843,750 B, and seam-free
  bilinear sampling across tile borders; see
  [`m2.5-09-tile-store.md`](m2.5-09-tile-store.md). `M2.5-10` added the softmax
  biome weights over the 18 centres: exact sums, a 348 m transition band and no
  hard argmax switches; see [`m2.5-10-weights.md`](m2.5-10-weights.md).
  `M2.5-11` moved the 18 biome definitions into `data/biomes.ron` and validates
  every palette key against the block manifest; the weight centres come from
  the file. See [`m2.5-11-biomes.md`](m2.5-11-biomes.md). `M2.5-12` added the
  surface rules — 3–6 block default soil, cliff stone, underwater beds,
  universal rock and snow, and beaches that require a shore — with every
  emitted key checked against the manifest; see
  [`m2.5-12-surface.md`](m2.5-12-surface.md). `M2.5-13` added the 1 m weighted
  material dither and four canonical two-material + debris 16³ templates per
  pair, all reusing committed materials so they add zero atlas slots; see
  [`m2.5-13-transitions.md`](m2.5-13-transitions.md). `M2.5-14` added the 3D
  cave field — spaghetti, cheese, shafts, galleries and water/lava fills — with
  balanced density, a 2,000 m depth budget and a 2 m seal under water; see
  [`m2.5-14-caves.md`](m2.5-14-caves.md). `M2.5-15` made cave depth relative to
  the local surface and assigned aquifer/fungal/cavern/geothermal with the
  softmax, resolving every underground material key; see
  [`m2.5-15-cave-depth.md`](m2.5-15-cave-depth.md). `M2.5-16` added the 3D
  overhang density, occasional cell-hash arches and the support pass that
  removes floating rock; see
  [`m2.5-16-overhangs.md`](m2.5-16-overhangs.md). `M2.5-17` added the
  depth-tiered ore veins — coal, iron, gold and diamond, with no surface gems
  and connected capsules — at 1.0% ore by volume; see
  [`m2.5-17-ores.md`](m2.5-17-ores.md). `M2.5-18` added one terrain-fitted
  landmark per 32 km cell (massif, canyon, volcano, grove, monolith/arch) and
  the `map --landmarks` distribution dump; see
  [`m2.5-18-landmarks.md`](m2.5-18-landmarks.md). `M2.5-19` added 2 km
  structure anchors (90.3% of cells empty) and terrain-fitted ruin, tower,
  dungeon and bridge prefabs with no floating foundations; see
  [`m2.5-19-structures.md`](m2.5-19-structures.md). `M2.5-20` added the
  cross-chunk harness: 1,000 structure stamps and 1,000 tree fixtures are cut
  at a border, rebuilt from both sides with an apron, and compared byte for
  byte; see [`m2.5-20-cross-chunk.md`](m2.5-20-cross-chunk.md). `M2.5-21` added
  flora: biome-density plants, 8 m tree cells, seam-only birch/ecotone scrub
  and 30–60 m giants on a 32 km grid; see
  [`m2.5-21-flora.md`](m2.5-21-flora.md). Surface detail (`M2.5-22`) is next.

## Tools

- [`apps/launcher/README.md`](../apps/launcher/README.md) — the desktop launcher: viewer, texture tools, checks and the Linux/Windows builds.
- [`crates/engine/viewer/README.md`](../crates/engine/viewer/README.md) — the M1.5 viewer: run instructions, controls and the slice screenshot.
- [`apps/texgen/README.md`](../apps/texgen/README.md) — the standalone Material Maker-style texture and material editor.
- [`texgen-backlog.md`](texgen-backlog.md) — what texgen still needs, what is deferred, and what is done.

## Previews

- [`worldgen-preview.md`](worldgen-preview.md) — a real seed-1 macro map from
  the current generator and hydrology, with the measured elevation, land and
  lake statistics.

## Engineering notes

- [`wgpu-traps.md`](wgpu-traps.md) — API rules that cost time in M3: buffer
  usage combinations, feature gates, submission ordering, draw-binding
  alignment and the surface texture lifetime. Read before touching uploads,
  queries or buffer slices.

## Decision records

| ADR | Decision | Status |
|---|---|---|
| [`ADR-001`](adr/ADR-001-render-path-raster.md) | Render path: raster | Accepted |
| [`ADR-002`](adr/ADR-002-chunk-edge.md) | Chunk edge: 64³ baseline, 128³ pre-registered | Accepted |
| [`ADR-003`](adr/ADR-003-material-atlas.md) | Material atlas: 256², 16×16 tiles, capped mips | Accepted |
| [`ADR-012`](adr/ADR-012-traversal.md) | Traversal: workgroup-per-chunk, frontier-synchronous, warp-per-node | Accepted |
| [`ADR-004`](adr/ADR-004-light.md) | Light: per-block cells + vertex AO | Accepted |
| [`ADR-005`](adr/ADR-005-probes.md) | Probes: per-block near, half-res mid, none far | Accepted |
| [`ADR-006`](adr/ADR-006-occlusion.md) | Occlusion: block flood-fill primary, ray-test M2 flag | Accepted |
| [`ADR-007`](adr/ADR-007-ab-roots.md) | A/B roots over the shared node pool | Accepted |
| [`ADR-008`](adr/ADR-008-visibility-lifecycle.md) | Visibility × lifecycle: two axes, no readback | Accepted |
| [`ADR-009`](adr/ADR-009-render-distance-lod.md) | Render distance and LOD: 128 m LOD0, 512 m LOD2, surface shell | Accepted |
| [`ADR-010`](adr/ADR-010-mesh-granularity-vertex-format.md) | Mesh granularity and vertex format: 16³ block meshlets, 16 B vertex, 76 B indexed quad | Accepted |
| [`ADR-011`](adr/ADR-011-node-layout.md) | Node layout: level bands, 8 B nodes, run dedup, compaction | Accepted |
| [`ADR-013`](adr/ADR-013-state-ownership.md) | State ownership: GPU per-frame, event-driven CPU | Accepted |

## Reference (historical, non-normative)

| Document | Still valid | Superseded by |
|---|---|---|
| [`reference/adr-three-tier-store-v2.md`](reference/adr-three-tier-store-v2.md) | Three-tier split, edit overlay, batched consolidation, hash-consed dedup, per-chunk interners | 4 m chunks, GPU raymarch as the primary path, explicit LOD trees, per-voxel light |
| [`reference/material-spec-v1.md`](reference/material-spec-v1.md) | 6.25 cm voxels, 1 m blocks, 16³ blocks, material registry, templates | 64³ as the final chunk edge, 256²-per-material texture arrays, per-voxel light, the damage model as storage |
| [`reference/template-cost-model.md`](reference/template-cost-model.md) | Block tier cost reasoning, template rarity argument | Nothing yet; still reference |

Reference documents are kept for the reasoning that led here. Do not implement from them
where they conflict with the spec.

## Measurement

- [`m0-exit.md`](m0-exit.md) — M0 exit checklist, command evidence and tag `m0`
- [`m1-gate-decision.md`](m1-gate-decision.md) — M1 gate verdict, all twelve rows and the fallback decision
- [`m2-apron-cost.md`](m2-apron-cost.md) — M2-02 apron vs dense padded-copy measurement
- [`m2-exit.md`](m2-exit.md) — M2-30 measured 128³ Minimum exit: twelve rows, the mesh miss, rows 9/11
- [`m3-mesh-compression.md`](m3-mesh-compression.md) — proposed instanced-quad (8 B/quad) plan, with an 8 B vertex fallback, for the 128³ mesh cap
- [`m3-packed-8b.md`](m3-packed-8b.md) — landed 44 B/quad packed-vertex fallback and its 128³/64³ gate numbers
- [`m3-01-streaming.md`](m3-01-streaming.md) — 256-chunk streaming ring: three-stage pipeline, origin uniform and the 0.09 ms pump measurement
- [`reference-machine.md`](reference-machine.md) — the 60 Hz frame-time target and the viewer trace command
- [`../benchmarks/README.md`](../benchmarks/README.md) — run archive, lenses, caveats
- [`../benchmarks/HISTORY.md`](../benchmarks/HISTORY.md) — every recorded run with deltas
- [`../crates/bench/README.md`](../crates/bench/README.md) — harness internals

## Tooling

- [`../scripts/health.sh`](../scripts/health.sh) — one-shot health check: in-repo gates,
  external tools (coverage, mutants, deny, audit) and a PASS/FAIL/SKIP summary
- [`../scripts/bench.sh`](../scripts/bench.sh) — benchmark run and archive (`RECORD=0` keeps
  it out of `benchmarks/`)
- [`../scripts/check-constants.sh`](../scripts/check-constants.sh) — the CI constants gate
- [`../scripts/build-docs-site.py`](../scripts/build-docs-site.py) — builds the static
  documentation site (`site/`) from every Markdown file; `.github/workflows/docs.yml`
  publishes it to GitHub Pages

## Conventions

- Task IDs (`M1-07`) are stable. Use them in commit messages.
- A task is done when its **Done when** column is satisfied *and* its **Verify** command
  passes on the reference machine.
- Benchmarks that gate a milestone are recorded through `scripts/bench.sh`, never quoted
  from an ad-hoc run.
