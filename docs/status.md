# Status

**Last updated:** 2026-11-02

Where the project actually is. The normative documents are
[the architecture spec](specs/voxel-engine-architecture-spec.md),
[the roadmap](specs/voxel-engine-roadmap.md) and
[the task list](specs/task-list.md); the milestone reports linked below hold the
measurements this page summarises.

| Milestone | State | Evidence |
|---|---|---|
| M0 | Complete — tag `m0` | [`m0-exit.md`](milestones/m0/m0-exit.md) |
| M1 | Complete — **128³ Minimum** selected for M2 | [`m1-gate-decision.md`](milestones/m1/m1-gate-decision.md) |
| M1.5 | Complete — viewer, atlas, screenshots, zero-validation CI | [`task-list.md`](specs/task-list.md) |
| M2 | Complete — measured 128³ Minimum exit; mesh compression carried into M3 | [`m2-exit.md`](milestones/m2/m2-exit.md) |
| M3 | Complete — tag `m3`; two conditions carried forward | [`m3-exit.md`](milestones/m3/m3-exit.md) |
| M2.5 | In progress — landed through `M2.5-21`; surface detail (`M2.5-22`) is next | [`m2.5-21-flora.md`](milestones/m2.5/m2.5-21-flora.md) |

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
  the 20% target.
- **M3 is complete and `m3` is tagged.** M3-00 (mesh compression), M3-01
  (streaming shell), M3-02 (mesh buffer pool), M3-03 (upload queue), M3-04
  (CPU chunk cull), M3-05 (draw loop), M3-06 (instrumentation), M3-07 (frame
  breakdown), M3-08 (stress scene), M3-09 (hitch test), M3-10 (60 Hz at the
  gated edge), M3-11 (memory report), M3-12 (visual regression) and M3-13 (the
  exit) are closed.
- **M2.5 has started.** `M2.5-01` created `voxel-worldgen` and froze the
  integer-hash, fixed-point noise contract with committed golden vectors; see
  [`m2.5-01-noise.md`](milestones/m2.5/m2.5-01-noise.md). `M2.5-02` added the 128 m macro grid
  and the 16 km tile format (93.75 kB payload, CRC-32, format version 1); see
  [`m2.5-02-macro-tile.md`](milestones/m2.5/m2.5-02-macro-tile.md). `M2.5-03` added the PCHIP
  shape splines: continentalness and erosion to a surface elevation, exact sea
  level at 6,600 m and a 12,000 m peak; see
  [`m2.5-03-splines.md`](milestones/m2.5/m2.5-03-splines.md). `M2.5-04` added the climate
  fields — compressed latitude, an altitude lapse and an ocean-distance
  humidity curve with an 8 km variation; see
  [`m2.5-04-climate.md`](milestones/m2.5/m2.5-04-climate.md). `M2.5-05` added the seeded macro
  tile generator, the `macro` CLI and the aggregated content hash: the whole
  4,096-tile world generates in **9.2 s** with a stable hash; see
  [`m2.5-05-macro-generator.md`](milestones/m2.5/m2.5-05-macro-generator.md). `M2.5-06` added
  priority-flood hydrology with D8 flow and accumulation, plus the `hydro`
  command; see [`m2.5-06-hydrology.md`](milestones/m2.5/m2.5-06-hydrology.md). It surfaced a
  metre/kilometre unit bug in the shape field; the follow-up shape fix added
  continents, per-seed ocean calibration and valley detail, raising the largest
  flow accumulation from 48 cells to **5.7M** and giving 1–2 continents per
  seed. See [`m2.5-shape-fix.md`](milestones/m2.5/m2.5-shape-fix.md). `M2.5-07` extracted the
  river network — 48,722 segments, order 5, 10–200 m widths, 576 deltas, every
  river ending at sea or a lake; see
  [`m2.5-07-rivers.md`](milestones/m2.5/m2.5-07-rivers.md). `M2.5-08` extracted 4,003 lakes
  with flat spill surfaces, spill cells and shorelines, and verified no basin
  leaks; see [`m2.5-08-lakes.md`](milestones/m2.5/m2.5-08-lakes.md). `M2.5-09` added the
  streaming tile store: an LRU cache, a 3 × 3 view in 843,750 B, and seam-free
  bilinear sampling across tile borders; see
  [`m2.5-09-tile-store.md`](milestones/m2.5/m2.5-09-tile-store.md). `M2.5-10` added the softmax
  biome weights over the 18 centres: exact sums, a 348 m transition band and no
  hard argmax switches; see [`m2.5-10-weights.md`](milestones/m2.5/m2.5-10-weights.md).
  `M2.5-11` moved the 18 biome definitions into `data/biomes.ron` and validates
  every palette key against the block manifest; the weight centres come from
  the file. See [`m2.5-11-biomes.md`](milestones/m2.5/m2.5-11-biomes.md). `M2.5-12` added the
  surface rules — 3–6 block default soil, cliff stone, underwater beds,
  universal rock and snow, and beaches that require a shore — with every
  emitted key checked against the manifest; see
  [`m2.5-12-surface.md`](milestones/m2.5/m2.5-12-surface.md). `M2.5-13` added the 1 m weighted
  material dither and four canonical two-material + debris 16³ templates per
  pair, all reusing committed materials so they add zero atlas slots; see
  [`m2.5-13-transitions.md`](milestones/m2.5/m2.5-13-transitions.md). `M2.5-14` added the 3D
  cave field — spaghetti, cheese, shafts, galleries and water/lava fills — with
  balanced density, a 2,000 m depth budget and a 2 m seal under water; see
  [`m2.5-14-caves.md`](milestones/m2.5/m2.5-14-caves.md). `M2.5-15` made cave depth relative to
  the local surface and assigned aquifer/fungal/cavern/geothermal with the
  softmax, resolving every underground material key; see
  [`m2.5-15-cave-depth.md`](milestones/m2.5/m2.5-15-cave-depth.md). `M2.5-16` added the 3D
  overhang density, occasional cell-hash arches and the support pass that
  removes floating rock; see
  [`m2.5-16-overhangs.md`](milestones/m2.5/m2.5-16-overhangs.md). `M2.5-17` added the
  depth-tiered ore veins — coal, iron, gold and diamond, with no surface gems
  and connected capsules — at 1.0% ore by volume; see
  [`m2.5-17-ores.md`](milestones/m2.5/m2.5-17-ores.md). `M2.5-18` added one terrain-fitted
  landmark per 32 km cell (massif, canyon, volcano, grove, monolith/arch) and
  the `map --landmarks` distribution dump; see
  [`m2.5-18-landmarks.md`](milestones/m2.5/m2.5-18-landmarks.md). `M2.5-19` added 2 km
  structure anchors (90.3% of cells empty) and terrain-fitted ruin, tower,
  dungeon and bridge prefabs with no floating foundations; see
  [`m2.5-19-structures.md`](milestones/m2.5/m2.5-19-structures.md). `M2.5-20` added the
  cross-chunk harness: 1,000 structure stamps and 1,000 tree fixtures are cut
  at a border, rebuilt from both sides with an apron, and compared byte for
  byte; see [`m2.5-20-cross-chunk.md`](milestones/m2.5/m2.5-20-cross-chunk.md). `M2.5-21` added
  flora: biome-density plants, 8 m tree cells, seam-only birch/ecotone scrub
  and 30–60 m giants on a 32 km grid; see
  [`m2.5-21-flora.md`](milestones/m2.5/m2.5-21-flora.md).

## Open conditions

Two conditions were carried out of M3 and are not closed:

- The Radeon iGPU records a 128³ 1% low of 56.6–61.2 fps against the 60 fps
  target. See [`m3-10-60hz.md`](milestones/m3/m3-10-60hz.md).
- 128³ is green only at the minimum residency profile (default 11.93 GiB,
  high 95.4 GiB). See [`m3-11-memory.md`](milestones/m3/m3-11-memory.md).

**Next:** surface detail (`M2.5-22`), then M3.5. M2.5 runs before M3.5.
