# Documentation

Start with [the current position](status.md), then the normative documents
below. This directory is the source for the documentation site;
[`site.toml`](site.toml) curates its navigation and
[`scripts/build-docs-site.py`](../scripts/build-docs-site.py) renders it.

## Layout

The documentation has two areas. **Guides** explain the engine in plain
language; **Reference** is the dense source of truth.

| Directory | Contents |
|---|---|
| [`guides/`](guides) | Start here, architecture, worldgen, rendering, measurement, glossary |
| [`specs/`](specs) | Normative specifications: architecture, roadmap, task list, worldgen, budgets |
| [`design/`](design) | Game-level field specs and the worldgen preview |
| [`adr/`](adr) | Architecture decision records |
| [`milestones/`](milestones) | Measured reports, grouped by milestone (`m0` … `m3`) |
| [`notes/`](notes) | Engineering notes, backlog and interim scope |
| [`reference/`](reference) | Superseded designs kept for their reasoning |
| [`images/`](images) | Screenshots and figures used by the reports |

## Guides

If you are new, read these first. They summarise the reference documents and
link back to the exact rules.

- [`guides/README.md`](guides/README.md) — what this is and where to start.
- [`guides/architecture-tour.md`](guides/architecture-tour.md) — chunks, storage,
  meshing, light and state.
- [`guides/worldgen-tour.md`](guides/worldgen-tour.md) — how a seed becomes a
  1,024 km world.
- [`guides/rendering-and-streaming.md`](guides/rendering-and-streaming.md) — the
  ring, the pipeline and the frame.
- [`guides/performance-and-measurement.md`](guides/performance-and-measurement.md)
  — the gates and how to read them.
- [`guides/glossary.md`](guides/glossary.md) — the jargon.

## Normative (read in this order)

| Document | Purpose |
|---|---|
| [`voxel-engine-architecture-spec.md`](specs/voxel-engine-architecture-spec.md) | What the engine is: units, tiers, protocols, layout, traversal, state |
| [`voxel-engine-roadmap.md`](specs/voxel-engine-roadmap.md) | Why the work is ordered this way; milestone gates |
| [`task-list.md`](specs/task-list.md) | Executable breakdown: every task, its dependency, done condition and verification |
| [`worldgen-design.md`](specs/worldgen-design.md) | World scale, macro map, biomes, blending, caves, rivers, far field, water |
| [`pre-registration.md`](specs/pre-registration.md) | **Frozen 2026-10-26.** M1 gate: profiles, budgets, residency model, fallback rules |
| [`reference-machine.md`](specs/reference-machine.md) | The machine every gate number is measured on; gated profiles |
| [`performance-targets.md`](specs/performance-targets.md) | Frame, VRAM, RAM and CPU budgets; sacrifice ladder; CI policy |
| [`generator.spec.md`](specs/generator.spec.md) | Material and texture pipeline: graphs, manifest, bake filter, atlas, material table |
| [`water-textures.md`](specs/water-textures.md) | M3.5-15 water/current faces: flow axis, UV scroll, alpha policy, caustics decision |

When these disagree, the spec wins, then the roadmap, then the task list. Any change to a
binding number is made in the spec and the pre-registration, never only in a task.

## Current position

M0, M1, M1.5, M2 and M3 are complete; `m3` is tagged. M2.5 worldgen is in
progress through `M2.5-21` (flora), with surface detail (`M2.5-22`) next. Two
M3 conditions remain open: the Radeon iGPU's 128³ 1% low and 128³ being green
only at the minimum residency profile.

[`status.md`](status.md) has the per-milestone position, the measured evidence
and the open conditions.

## Tools

- [`apps/launcher/README.md`](../apps/launcher/README.md) — the desktop launcher: viewer, texture tools, checks and the Linux/Windows builds.
- [`crates/engine/viewer/README.md`](../crates/engine/viewer/README.md) — the M1.5 viewer: run instructions, controls and the slice screenshot.
- [`apps/texgen/README.md`](../apps/texgen/README.md) — the standalone Material Maker-style texture and material editor.
- [`texgen-backlog.md`](notes/texgen-backlog.md) — what texgen still needs, what is deferred, and what is done.

## Previews

- [`worldgen-preview.md`](design/worldgen-preview.md) — a real seed-1 macro map from
  the current generator and hydrology, with the measured elevation, land and
  lake statistics.

## Engineering notes

- [`wgpu-traps.md`](notes/wgpu-traps.md) — API rules that cost time in M3: buffer
  usage combinations, feature gates, submission ordering, draw-binding
  alignment and the surface texture lifetime. Read before touching uploads,
  queries or buffer slices.

## Game design

Game-level specifications for the technology tree. These define what the game
simulates, not how the engine works; they do not override the architecture spec.

- [`design/field-spec-fire-heat.md`](design/field-spec-fire-heat.md) — the root field: controlled heat, fuels and firing.
- [`design/field-spec-mining-prospecting.md`](design/field-spec-mining-prospecting.md) — ore, clay, stone and fuel extraction.
- [`design/field-spec-ceramics-clay.md`](design/field-spec-ceramics-clay.md) — containers, molds, bricks and furnace linings.
- [`design/field-spec-metallurgy-smithing.md`](design/field-spec-metallurgy-smithing.md) — metals, alloys, casting and smithing.

## Decision records

| ADR | Decision | Status |
|---|---|---|
| [`ADR-001`](adr/ADR-001-render-path-raster.md) | Render path: raster | Accepted |
| [`ADR-002`](adr/ADR-002-chunk-edge.md) | Chunk edge: 64³ baseline, 128³ pre-registered | Accepted |
| [`ADR-003`](adr/ADR-003-material-atlas.md) | Material atlas: 256², 16×16 tiles, capped mips | Accepted |
| [`ADR-004`](adr/ADR-004-light.md) | Light: per-block cells + vertex AO | Accepted |
| [`ADR-005`](adr/ADR-005-probes.md) | Probes: per-block near, half-res mid, none far | Accepted |
| [`ADR-006`](adr/ADR-006-occlusion.md) | Occlusion: block flood-fill primary, ray-test M2 flag | Accepted |
| [`ADR-007`](adr/ADR-007-ab-roots.md) | A/B roots over the shared node pool | Accepted |
| [`ADR-008`](adr/ADR-008-visibility-lifecycle.md) | Visibility × lifecycle: two axes, no readback | Accepted |
| [`ADR-009`](adr/ADR-009-render-distance-lod.md) | Render distance and LOD: 128 m LOD0, 512 m LOD2, surface shell | Accepted |
| [`ADR-010`](adr/ADR-010-mesh-granularity-vertex-format.md) | Mesh granularity and vertex format: 16³ block meshlets, 16 B vertex, 76 B indexed quad | Accepted |
| [`ADR-011`](adr/ADR-011-node-layout.md) | Node layout: level bands, 8 B nodes, run dedup, compaction | Accepted |
| [`ADR-012`](adr/ADR-012-traversal.md) | Traversal: workgroup-per-chunk, frontier-synchronous, warp-per-node | Accepted |
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

- [`m0-exit.md`](milestones/m0/m0-exit.md) — M0 exit checklist, command evidence and tag `m0`
- [`m1-gate-decision.md`](milestones/m1/m1-gate-decision.md) — M1 gate verdict, all twelve rows and the fallback decision
- [`m2-apron-cost.md`](milestones/m2/m2-apron-cost.md) — M2-02 apron vs dense padded-copy measurement
- [`m2-exit.md`](milestones/m2/m2-exit.md) — M2-30 measured 128³ Minimum exit: twelve rows, the mesh miss, rows 9/11
- [`m3-mesh-compression.md`](milestones/m3/m3-mesh-compression.md) — proposed instanced-quad (8 B/quad) plan, with an 8 B vertex fallback, for the 128³ mesh cap
- [`m3-packed-8b.md`](milestones/m3/m3-packed-8b.md) — landed 44 B/quad packed-vertex fallback and its 128³/64³ gate numbers
- [`m3-01-streaming.md`](milestones/m3/m3-01-streaming.md) — 256-chunk streaming ring: three-stage pipeline, origin uniform and the 0.09 ms pump measurement
- [`reference-machine.md`](specs/reference-machine.md) — the 60 Hz frame-time target and the viewer trace command
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
