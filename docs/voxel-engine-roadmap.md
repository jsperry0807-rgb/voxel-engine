# Voxel Engine Roadmap

**Status:** v3.2 — M1 gate decided, M1.5 slice landed, M2 in progress
**Companion to:** [`voxel-engine-architecture-spec.md`](voxel-engine-architecture-spec.md), [`worldgen-design.md`](worldgen-design.md)
**Date:** 2026-10-29
**Scope:** storage, content pipeline, worldgen, renderer, far field, water, GPU sync, culling, LOD, streaming
**Not in scope:** gameplay, editor, networking, audio, physics, damage rendering, content authoring

The spec's four gates are now closed: raster, block-level flood-fill occlusion,
per-block light, and a chunk edge that is **64³ in M1 with 128³ pre-registered**.
This roadmap turns those into an ordered plan whose first milestone is a
pass/fail measurement, not a build-out.

M0 and M1 are complete. The M1 gate selected the **128³ Minimum** profile for M2,
conditional on mesh compression and a measured 128³ run. The M1.5 vertical slice
is code-complete; only `M1.5-17` (before/after texel-density screenshots) remains
open. The full M1.5-15 screenshot set and zero-validation CI (M1.5-16) are in
place. M2 has started: the apron API (M2-01) landed on 2026-10-28.

Two consequences shape the schedule:

1. **The gate needs oracles.** Edit latency, probe update and culling granularity
   cannot be measured without a reference light fill, a reference probe bake and a
   reference cull. M1 builds them slow and correct; M2 and M5 productionise them.
2. **M2 implements and measures the 128³ Minimum edge.** The 64³ Default gate
   failed on memory. The 128³ Minimum projection clears the hard-fail line
   optimistically, so the upgrade is now a measured decision, not a theoretical
   one: mesh compression and the 128³ gate run are part of M2.

---

## 1. Where the code is today

| Crate | What exists | State |
|---|---|---|
| `voxel-core` | Canonical `Face` orientation, neighbour offsets, face planes | Complete |
| `voxel-block` | Registry, materials, texture slices, material variants, palettes, tags, RON | Complete; atlas-backed material ids in use |
| `voxel-storage` | Parameterized 64³/128³ 8 B-node SVDAG, COW `apply_edits`, overlays, region log, global deduped GPU pool, consolidation policy, six-face apron | **M1 layout complete; M2-01 apron landed** |
| `voxel-mesh` | Block-granular driver over 16³ blocks with a reusable 18³ scratch; greedy rectangle core with the `(material, orientation, 4×AO)` merge key, backface culling, four-level AO sampling and chunk-border AO equality; 16 B vertex codec, golden fixtures, thread/order determinism, full-pass two-channel block light, cross-chunk apron seeding, dirty-region incremental light and removal-front escalation | **M2-03…M2-16 landed**; light benchmark and probes pending |
| `voxel-render` | `WgpuSink`, `MaterialBuffers`, `GpuWorld`, `svdag.wgsl` | Upload path; the viewer owns the draw loop |
| `voxel-viewer` | `winit` + `wgpu` window, fly camera, mouse look, atlas sampling, depth pass, naive per-face mesher, input layer | **M1.5 code complete**; production mesh/light/probes pending |
| `voxel-texgraph` / `voxel-texgen` | Deterministic node-graph evaluator, manifest, material table, atlas bake, editor and MCP tools | Complete for M1.5 |
| `voxel-launcher` / `xtask` | Desktop launcher, Linux/Windows builds, toolchain preflight and on-demand install | Complete |
| `voxel-bench` | Memory/CPU/latency/counter suites, archived history, `iai` counters | Complete; the gate depends on it |

The M1 layout is in place at both edges; the 128³ variant compiles in CI. M1.5
added the window, camera, atlas and naive mesher. M2-01 added the apron. The
production content path is now the work in front of the project.

Still missing: AO, production block light and probes, sparse-set ECS, culling,
LOD, streaming, persistence, and the measured 128³ gate run. CI, the
window/camera app, the naive mesher, the apron/driver/greedy core and the
launcher are in place.

---

## 2. Gates and their consequences

| Gate | Answer | What it changes in this plan |
|---|---|---|
| Chunk edge | 64³ measured in M1; 128³ Minimum greenlit for M2 | M2 implements and measures the 128³ Minimum edge; Default is not greenlit |
| Render path | Raster | M1.5 draws meshes; the node pool serves culling/lighting/picking |
| Occlusion | Block-level flood-fill (M1); ray-test deferred (M2) | Extra dispatch before broad phase; ray-test is an M2 flag |
| Light resolution | Per-block (16³) + vertex AO | Light volume is 1 KiB at 64³, 8 KiB at 128³; no per-voxel BFS |

No architectural decision remains open. The 128³ measurement can still reject
the upgrade and trigger the pre-registered fallback.

---

## 3. Sequencing rules

1. **Measure before building out.** M1's gate runs before M2's production pipeline.
2. **Pre-registration is binding.** The M1 table is written in M0 and not
   renegotiated during M1.
3. **Reference before optimised.** M1's slow implementations become M2/M5's test
   oracles.
4. **The layout is an invariant.** If a later milestone wants a node outside its
   band, the design is wrong.
5. **No per-frame GPU→CPU readback.** Visibility never leaves the GPU.
6. **Every milestone ends with numbers** recorded through `scripts/bench.sh`.
7. **One source of truth for constants.** `CHUNK_BITS` is the only place the edge
   is named; CI compiles both 6 and 7.

---

## 4. Milestones

Effort is one experienced graphics engineer including tests and review. A milestone
is done only when its gate passes on the reference machine.

### M0 — Pre-registration, constants, ADRs (1 week)

**Status:** Complete — tagged `m0` on 2026-10-26. Exit checklist:
[`m0-exit.md`](m0-exit.md).

Write the gate before anything is measured.

**4.1 Pre-registration table** (binding; estimates to be replaced by M1 numbers):

The frozen version, including the residency model and the fallback rules, is
[`pre-registration.md`](pre-registration.md) (2026-10-26). The summary below is for
orientation only; the pre-registration wins.

| Parameter | Pre-registered value | Basis | Implied / check |
|---|---|---|---|
| VRAM budget for voxel data | **40% of total VRAM** (fraction, not absolute) | Survives hardware changes; leaves room for framebuffer, mesh, atlas, streaming | 8,188 MiB → 3,275 MiB target; gate at 4,913 MiB |
| Target loaded chunk count @ max view distance | Computed **per chunk edge**; 64³ loads **8×** the chunks of 128³ at the same view distance | One chunk count for both edges makes the report meaningless | M0 fixes the ring config; M1 measures the count on the save |
| SVDAG node budget/chunk | 64³ ≤ **200k**; 128³ ≤ **1.5M** unique nodes (sharing-adjusted) | 8 B/node × 2 roots = 16 B/node | 3.2 MB/chunk at 64³, 24 MB/chunk at 128³: **hard caps, not p95** |
| Light volume/chunk | 64³: **64 cells**; 128³: **512 cells**; ~**16 B/cell** | Per-block, RGB+intensity or SH2 | 1 KiB / 8 KiB |
| Probe grid/chunk | 64³: **64**; 128³: **512** near-residency, multi-res LOD | Per-block near, half-res mid, none far | 768 B / 6 KiB |
| Mesh buffer/chunk (raster) | 64³: **150–600 KB**; 128³: **600 KB–2.4 MB** | Surface-area scaling; exposed faces from gameplay content | 76 B/indexed quad |
| Edit latency | p95 ≤ **8 ms** (consolidate + upload + fence + flip + dirty-region light) | Responsiveness target | Measured on the real save |
| Probe-update budget | p95 ≤ **4 ms** per edited chunk | Keeps probe work off the critical path | Measured on the real save |
| Culling granularity | ≤ **15%** traversed-but-invisible volume at chunk boundaries @ max view distance | 64³ passes easily; 128³ is the question | Reference traversal at block resolution |
| Safety factor | **1.5×** headroom on the memory budget | Absorbs chunky content, fragmentation, driver reserve | Gate is budget × 1.5 |
| Measurement dataset | **Real populated save from actual gameplay**; report median, p95, max per chunk | Synthetic dense chunks lie; empty ones lie worse | Provisional fixture until the first gameplay save exists |
| Fallback trigger | Memory `p95 × N > budget × 1.5`, **or** edit p95 > 8 ms, **or** probe p95 > 4 ms → **stay at 64³** | Pre-registered; no renegotiation in M1 | Applies to the 128³ projection too |

**4.2 Residency model.** Define `N(edge, view, band)` = the chunk count the streaming
policy holds for the configured view distance and surface band, so the two edges are
compared at equal loaded *volume*, not equal chunk count. N is measured on the save
in M1.

**4.3 Constants and contracts.** `CHUNK_BITS`, `ROOT_LEVEL = CHUNK_BITS`,
`BLOCK_LEVEL = CHUNK_BITS − 4`, `AABB(level)`, node bit fields, band count, atlas
tile math, probe rings, occlusion horizon. CI builds both edges.

**4.4 ADRs:** raster (001), chunk edge + gate (002), atlas (003), per-block light
(004), probes multi-res (005), flood-fill occlusion + M2 ray-test (006), A/B roots
(007), lifecycle (008), render distance (009), mesh/vertex format (010), node layout
(011), traversal (012), state ownership (013).

**Exit:** table committed; every new content buffer size derives from constants;
the 7-bit variant is selectable and fails loudly behind the M0-08 guard until
M1-24; ADR-006 states the escalation path if flood-fill under-culls.

---

### M1 — Layout at 64³ + measurement gate (7 weeks)

**Status:** Complete — tagged `m1` on 2026-10-27. The 64³ Default gate failed on
memory; the 128³ Default projection also missed, but the **128³ Minimum** projection
was projected to clear the hard-fail line at 3.88 GiB optimistic, but the model
excluded the resident v1 pool: counted, it projects **4.89 GiB**, above the line.
M2 is greenlit to implement and measure 128³ Minimum conditional on mesh
compression and removing the v1 pool (M6-09a):
[`m1-gate-decision.md`](m1-gate-decision.md).

**Goal:** the layout is the spec's, and the pre-registered table has real numbers.

Work:

1. **Layout rewrite** at 64³: 8 B nodes, compacted children, popcount addressing,
   append-only bands (7), leaf band, run-level hash-consing, COW run append,
   64 B alignment for full runs, compaction behind a guarded precondition.
2. **Parameterize**, do not migrate: replace every 64/6 literal with `CHUNK_BITS`
   and `ROOT_LEVEL`; add the 128³ compile-only CI job.
3. **Invariant checker** and property tests: refcounts, dedup bijection, band
   bounds, alignment, compaction equals rebuild, 10 000 random edit batches equal a
   dense reference, byte round-trip fuzz.
4. **Real-save fixture.** Freeze a representative world (terrain, caves, overhangs,
   structures, mixed materials) under `benchmarks/fixtures/`, with a documented
   limitation: it is provisional until the first real gameplay save exists.
5. **Reference implementations** (slow, correct, reused as M2/M5 oracles):
   - block-level light flood fill and a probe bake;
   - count-only greedy mesh (quads and bytes without vertices);
   - CPU hierarchical frustum + block-occlusion traversal with granularity
     instrumentation.
6. **Gate harness:** per-chunk p50/p95/max memory, edit latency broken into fixed
   (upload/fence/flip) and dirty-region terms, probe update, granularity.
7. **Analytic scale-up** to 128³ using fixed-overhead correction and N/8, not a
   naive ×8.

**Exit — pass/fail, no renegotiation:**

- **Layout:** average ≤ 9 B/internal node including padding; one 64 B line per full
  sibling run (`iai` cache-miss check); dedup within 10% of the old encoding;
  COW ≤ 512 B per edited path.
- **Memory:** `p95 per-chunk × N ≤ 40% VRAM × 1.5`.
- **Edit latency:** p95 ≤ 8 ms.
- **Probe update:** p95 ≤ 4 ms.
- **Culling granularity:** ≤ 15% traversed-but-invisible at chunk boundaries at
  max view distance.
- **128³ projection:** memory clears the same budget at N/8 with the measured
  per-chunk growth; edit latency projected from fixed + dirty terms ≤ 8 ms; probe
  update projected ≤ 4 ms. If any misses, **stay at 64³** and record it.

**Risk:** the gate's weakest input is the provisional save. Label the first result
provisional and re-run the gate when a real save exists. Do not upgrade on a
provisional pass without the re-run.

---

### M1.5 — Vertical slice: one chunk on screen (2 weeks)

**Status:** Code complete — viewer, atlas, naive mesher, full M1.5-15 screenshot
set and zero-validation CI landed. Only `M1.5-17` (before/after texel-density
screenshots) remains open.

At 64³, so the slice is cheap.

- `crates/engine/viewer`: `winit` + `wgpu`, camera, depth pass, present.
- **Atlas builder:** one 256×256 texture, mips capped at 4; first pass sources the
  23 slice faces (M1.5-14), then downsamples them into 16×16 tiles.
- **Registry cap:** freeze asserts `len() <= 256`; per-face blocks become material
  variants; `TextureSetId` collapses.
- Naive per-face mesher with the M0 vertex format (no greedy, no AO), UVs derived
  in the fragment shader.
- Draw one generated chunk; move the camera.

**Exit:** screenshot in the repo; zero validation errors; 60 Hz at 1080p with one
chunk; vertex format and UV derivation frozen. **Known regression:** texel density
drops to 6.25 cm per voxel face; before/after screenshots required.

---

### M2 — Production content pipeline (6 weeks)

**Status:** In progress — M2-16 removal-front escalation passes the adversarial
test. Next: M2-17 light cost benchmark.

Runs at the **128³ Minimum** edge (ADR-002). Default is not greenlit.

- **Apron API:** six 1-voxel face planes, block-granular access; no dense chunk
  expands.
- **Greedy mesher** (`crates/engine/mesh`): 16³ + apron, bitmask merge, key
  `(material, orientation, 4×AO)`, backface cull, 16 B vertex, no UV attribute.
- **AO:** 3-neighbour corners before merge; rendered and tested against the M1
  reference.
- **Block light:** flood fill over block cells, 1-block apron, incremental dirty
  region with escalation to full-chunk relight at the boundary.
- **Probes:** multi-resolution — per-block near, half-res mid, none far; SH4 RGB8;
  invalid-in-solid fallback; LOD transition as a content event.
- **Ray-test occlusion behind a flag** (`occlusion.raytest`), evaluated against the
  flood-fill result on the M1 fixtures; not enabled by default.
- **Dirty tracking:** per-block bitset from the COW diff.

**Exit:**
- Golden mesh hashes across 5 fixtures equal to the M1 reference; AO seam test
  passes; light incremental == full on 1 000 edit sets; probe determinism across
  runs and thread counts.
- ≥ 20 terrain chunks/s single-threaded; scrub ≤ 4 ms; scratch ≤ 64 KiB/worker.
- Ray-test flag produces a documented delta (accuracy and cost) versus flood-fill,
  without changing default behaviour.
- **128³ Minimum gate run** recorded after M2-29: memory, edit, probe and
  throughput measured on the reference machine. If it misses the 4.80 GiB
  hard-fail line, fall back to 64³ Minimum with the pre-registered sacrifice
  ladder.
- Gate rows 9 and 11 closed: the M1-34 granularity reference is wired into the
  report, and a real populated save replaces the provisional fixture.

---

### M3 — Raster scale-up (3 weeks)

- Viewer lifecycle: stream in, mesh, upload, draw.
- CPU frustum cull of chunk AABBs — the stopgap and the M5 production cull's
  correctness reference (the M1 reference traversal is the granularity oracle).
- Mesh buffer pool; the chosen scheme becomes M4's versioned resource.
- Instrumentation: CPU submit, GPU pass, visible chunks/blocks, quads, bytes.

**Exit:** 256 chunks at 60 Hz at 1080p at the gated edge; frame breakdown recorded;
no upload hitch over 4 ms.

---

### M2.5 — Worldgen (9 weeks)

Runs in parallel with M2–M3 with a second engineer; otherwise insert after M2.
Full design and numbers: [`worldgen-design.md`](worldgen-design.md).

- 128 m tiled macro map: continentalness, erosion, climate, height.
- Hydrology: depression fill, D8 flow, accumulation, vector rivers, lakes.
- 18 biomes in RON; softmax weights; 300–500 m gradients; canonical transitions.
- Caves (five types), ores, landmarks, structures, flora, surface detail.
- Block-first chunk pipeline at 2 ms; workers, cancellation, prefetch.
- Bit-exact determinism; generator frozen per save.

**Exit:** a 4 × 4 km map and a 1 km walkthrough that look like the intended world;
p95 chunk generation ≤ 2 ms; identical bytes across 1/2/8 workers; full creation
≤ 10 min.

### M3.5 — Far field, water and map (4 weeks)

- Macro heightfield mesh with LOD rings; 32 km view.
- Near/far seam at 512 m (blend band plus skirt).
- Transparent water, depth sorting, underwater fog, river current.
- Map atlas data and item; menu orbit view.

**Exit:** 60 Hz at 1080p at 16–32 km view; no visible seam over a 2 km flight;
p99 hitch ≤ 4 ms.

---

### M4 — Versioned GPU resources and COW sync (4 weeks)

- Chunk table slot gains `rootA`, `rootB`, `versionIndex`. Root pair over the shared
  pool; retirement through the existing epoch mechanism.
- **Versioned resource** `{ active, inactive, inFlightFrame[2], fence }`, reused for
  roots and mesh buffers.
- **Level-band uploads:** dirty run ranges per band; one staging pass, one fence,
  flip at the boundary.
- **Compaction hazard:** compaction is forbidden while any GPU root or in-flight
  frame references the chunk; the precondition is asserted and fuzz-tested.
- Soak: 1 000 edits/s for 10 000 frames, per-frame readback hash versus CPU.

**Exit:** zero torn frames, zero invariant failures, flat RSS/VRAM; commit ≤ 0.1 ms
CPU/frame; ≤ 1 fence wait/frame; root-pair overhead ≤ 5% of chunk DAG.

---

### M5 — GPU-driven culling (5 weeks)

- **Dispatch 0 — occlusion fill:** port the M1 block-level flood fill to compute;
  camera-seeded, bounded horizon, one bit per block.
- **Dispatch 1 — broad phase:** one workgroup per resident chunk, chunk-root AABB
  vs 6 planes, compact survivors.
- **Dispatch 2 — traversal:** indirect, one workgroup (4 warps) per candidate chunk;
  orbit-ordered frontier; warp-per-node, lanes 0–7; 64 B run fetch; popcount child
  addressing; block bit test at `BLOCK_LEVEL`; `lodMaxDepth` cutoff; append blocks.
- **Dispatch 3 — draw:** indirect over the append buffer.
- **Overflow:** ≥ 1 024-node frontier spills as conservative visible blocks.
- **Debug:** CPU reference parity, "draw everything" false-negative detector,
  frontier/overflow visualiser.

**Exit:** block-set equality with the CPU reference for 500 camera poses across 3
fixtures; zero false negatives; fill + traversal ≤ 1.5 ms at 4 096 chunks; ≥ 3×
draw-work reduction; overflow < 0.1%. **Kill criterion:** if the reduction is < 1.5×
after the timebox, keep the CPU cull and record why in ADR-006.

---

### M6 — LOD, streaming, persistence (6 weeks)

- LOD policy at the gated edge; measure the cost of holding full-res chunks against
  explicit LOD meshes; decide with data.
- Streaming: prefetch by camera velocity, eviction by distance, per-frame budget.
- Disk: header version + `CHUNK_BITS`, node bands, leaf band, overlay diff, light
  cells, probes, dirty-region journal; reopen without full relight.
- Chunk and LOD seam policy (stitch or skirt).

**Exit:** the D9 rings fit the 40% budget at the gated edge; 60 Hz at 1080p;
streaming hitch p99 ≤ 4 ms; reopen ≤ 50 ms/chunk; journal light == recomputed;
no cracks on the seam fixture.

---

### M7 — Hardening and budgets (4 weeks, then continuous)

- All new benches in `benchmarks/HISTORY.md`; gate cases become permanent.
- CI: fmt, clippy, tests, `naga` validation, both-edge compile, GPU tests on a
  self-hosted runner; wall-clock benches advisory until stable.
- Regression policy: > 10% on a stable metric fails review; over-budget memory
  fails build.
- Tools: chunk inspector, band/layout viewer, cull visualiser, light debug view.

**Exit:** §6 budgets enforced; overnight soak passes; the viewer runs from a clean
checkout.

---

## 6. Budgets

Assumptions: block 16³, 16 B vertex, 76 B indexed quad, per-block light at 16 B/cell,
per-block probes at 12 B, gated chunk edge.

### Per chunk

| Item | 64³ | 128³ | Notes |
|---|---:|---:|---|
| SVDAG (8 B nodes + bands) | 60–90 KiB | 0.5–0.7 MiB | Hard caps: 3.2 MB / 24 MB |
| A/B root overhead | ~2 KiB | ~4 KiB | Duplicated path runs |
| Overlay | 32 KiB | 32–64 KiB | Requires shrinking the 8 704 B block slot |
| Light (64 / 512 cells × 16 B) | 1 KiB | 8 KiB | Per-block |
| Probes (64 / 512 × 12 B) | 768 B | 6 KiB | Near LOD |
| Mesh | 150–600 KB | 600 KB–2.4 MB | Pre-registered range |
| **Typical total** | **0.2–0.7 MiB** | **1.2–3.2 MiB** | Surface-dominated |

### Global

| Item | Size |
|---|---:|
| Material atlas (256², BC7 + mips) | ~85 KiB, always resident |
| Block occlusion bits (horizon 128 m) | 2 MB at 1 bit/block |
| Candidate + append + indirect buffers | ≤ 1 MiB |
| **VRAM budget for voxel data** | **40% of device** (gate at 60%) |

### Frame (16.6 ms at 60 Hz)

| Stage | Budget |
|---|---:|
| Occlusion fill (128 m horizon) | 0.5 ms |
| Broad phase + traversal | 1.0 ms |
| Content (amortised, budgeted) | 2.0 ms CPU |
| Upload / flip | 0.5 ms |
| Draw + present | 12.0 ms |

**Illustrative only, not the gate:** at 8 GB, a 128 m view with a 32 m surface band
gives roughly 25k chunks at 64³ and 3.2k at 128³. At 0.25 MiB and 1.2 MiB per chunk
that is ~6.4 GB (fails the 4.8 GB gate) versus ~3.9 GB (passes). The lesson is that
surface-dominated worlds favour larger chunks because mesh and overlay scale with
area, not volume — but M1's measured numbers decide, and the residency model in M0
fixes the actual count.

---

## 7. Dependency graph and staffing

```
M0 ── M1 ──┬── M1.5 ── M2 ── M2.5 ── M3 ── M3.5 ──┐
            │                                      ├── M5 ── M6 ── M7
            └── M4 ────────────────────────────────┘
```

Critical path (one engineer): M0 → M1 → M2 → M3 → M4 → M5 → M6 = **32 weeks** before
worldgen. M2.5 and M3.5 add 63 and 27 planning days, taking the one-engineer plan to
**~59 weeks (~14 months)**. With two engineers M2.5 runs beside M2–M3 and the effort
drops to **~9–10 months**. The bottom-up task list puts the M1 gate at the end of
week 8; see `docs/task-list.md` §2 for the sprint map.

- **A — Storage and sync:** M1 layout, M4, M6 persistence.
- **B — Content and renderer:** M1 references, M1.5, M2, M3, M5.
- **C — Worldgen:** M2.5, then M3.5 with B.
- **Both:** M0, the M1 gate, M5 parity, M6 LOD, M7 budgets.

---

## 8. Risk register

| # | Risk | Trigger | Mitigation |
|---|---|---|---|
| R1 | Per-chunk tail latency (8× at 128³) | Edit p95 > 8 ms | Pre-registered fallback to 64³; block-granular dirty work |
| R2 | Occlusion stalls | M5 > 3 weeks without a win | Ship frustum-only (still ≥ 3×); ray-test flag exists |
| R3 | Mesh memory exceeds DAG | M3 over pre-registered range | LOD meshes, block meshes, quantise harder |
| R4 | Incremental light wrong at borders | M2 equality fails | Expand radius; escalate to full relight |
| R5 | Readback sneaks in | Profiler stalls | Visibility GPU-only; lifecycle events only |
| R6 | Root starvation | Same chunk skipped 3+ frames | Coalesce edits; third root |
| R7 | Buffer resize invalidates bind groups | Validation error | Binding-lifetime rule + regression test |
| R8 | Bench flakiness | Deltas > 20% | Machine-tagged runs; advisory CI; counters |
| R9 | Scope creep | New feature mid-milestone | Out of scope by §0 |
| R10 | Band compaction invalidates GPU pointers | Torn geometry | Precondition + assert; never compact resident chunks |
| R11 | 256 material slots too few | Content hits the cap | Reserve ~64 slots for face variants; error at freeze |
| R12 | Atlas bleed / quality regression | Seams or blur | Half-texel inset; mips capped at 4; pad to 20×20 if needed |
| R13 | Run-granular dedup loses sharing | Dedup > 10% worse | Measured in M1; revisit the 8-sibling constraint before M2 if it regresses |
| R14 | Workgroup-per-chunk imbalance | Few chunks dominate the fill/traversal | Early retirement; per-launch frontier cap |
| R15 | **Flood-fill occlusion is connectivity, not line-of-sight** | Granularity gate fails outdoors (open terrain stays reachable) | Expected; escalation is the M2 ray-test, not a chunk-size change. Do not tune the chunk edge to compensate |
| R16 | **The gate lacks a real save** | Only synthetic fixtures exist | Freeze a representative world, label the result provisional, re-run on the first gameplay save before committing the upgrade |
| R17 | **128³ projection is optimistic** | Fixed-overhead correction mis-modelled | Model fixed and dirty terms separately; project pessimistic (dirty region spans the chunk) and optimistic; gate on the pessimistic |

---

## 9. Measurement plan

Add to `voxel-bench`; record every milestone through `scripts/bench.sh`.

| Case | Metric |
|---|---|
| `dag/bfs_build/{fixture}` | ms/chunk, nodes, runs, padding ratio |
| `dag/cow_path/{edits}` | bytes/edit, runs created |
| `dag/run_dedup/{fixture}` | dedup ratio vs old encoding |
| `dag/compaction/{fixture}` | ms, pointer-invalidation assertion |
| `dag/traversal/{fixture}` | steps/block, **iai D-cache misses per sibling run** |
| `gate/memory_p95` | per-chunk p50/p95/max, × N, versus budget |
| `gate/edit_p95` | fixed vs dirty-region terms |
| `gate/probe_p95` | per edited chunk |
| `gate/cull_granularity` | traversed-but-invisible % at boundaries |
| `mesh/build/{fixture}` | ms/chunk, quads, merge ratio, bytes |
| `mesh/remesh_block` | µs per edited block |
| `light/full/{fixture}`, `light/incremental` | ms/chunk, µs, equality vs full |
| `probe/bake/{fixture}` | µs/probe, SH determinism |
| `cull/cpu_ref` | µs for the resident set |
| `cull/gpu` | µs, visible/total, false positives, overflow |
| `upload/commit_batch` | µs, bytes/frame, fence waits |
| `stream/load`, `stream/reopen` | ms/chunk, hitches |
| `world/{chunks}_budget` | VRAM + host bytes at the gated edge |

---

## 10. Decision records (M0)

- ADR-001 Render path: raster (closed)
- ADR-002 Chunk edge: 64³ baseline, 128³ pre-registered upgrade + gate
- ADR-003 Atlas: single 256², 16×16 tiles, UV derivation, mip cap
- ADR-004 Light: per-block 16³ cells + vertex AO
- ADR-005 Probes: per-block near, half-res mid, none far
- ADR-006 Occlusion: block flood-fill primary; ray-test M2, flagged
- ADR-007 A/B roots over the shared pool
- ADR-008 Lifecycle: visibility × lifecycle, no readback
- ADR-009 Render distance and LOD policy
- ADR-010 Mesh granularity and vertex format
- ADR-011 Node layout: bands, 8 B nodes, run dedup, compaction
- ADR-012 Traversal: workgroup-per-chunk, frontier-synchronous, warp-per-node
- ADR-013 State ownership: GPU per-frame state, event-driven CPU ECS

---

## 11. Deferred

Far-field raymarching, GPU-side remeshing, bindless materials, damage crack
rendering, editor, networking, audio, physics, content authoring, console/mobile
targets.

---

## 12. Definition of done

The roadmap is complete when: the M1 gate has passed or fallen back with a written
record; the layout invariants hold (≤ 9 B/node, one line per sibling run, dedup
within 10%); a world at the gated edge runs at 60 Hz within the 40% VRAM budget;
GPU culling beats the CPU reference by ≥ 3× with zero false negatives and < 0.1%
frontier overflow; an overnight edit soak is artifact-free and leak-free; and
`benchmarks/HISTORY.md` shows every §6 budget met on the reference machine.
