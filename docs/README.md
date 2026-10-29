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

When these disagree, the spec wins, then the roadmap, then the task list. Any change to a
binding number is made in the spec and the pre-registration, never only in a task.

## Current position

- **M0 and M1 are complete.** The M1 gate selected the **128³ Minimum** profile for
  M2, conditional on mesh compression and a measured 128³ run.
- **M1.5 is code-complete.** The viewer, atlas, naive mesher and full M1.5-15
  screenshot set landed; only `M1.5-17` (before/after texel-density pair) remains
  open. The zero-validation CI job (`M1.5-16`) is also in place.
- **M2 is in progress.** The apron API (`M2-01`), its cost benchmark (`M2-02`) and
  the block-granular driver (`M2-03`) landed; `M2-04` greedy core is next. The
  production mesher, AO, light and probes are still to come.

## Tools

- [`apps/launcher/README.md`](../apps/launcher/README.md) — the desktop launcher: viewer, texture tools, checks and the Linux/Windows builds.
- [`crates/engine/viewer/README.md`](../crates/engine/viewer/README.md) — the M1.5 viewer: run instructions, controls and the slice screenshot.
- [`apps/texgen/README.md`](../apps/texgen/README.md) — the standalone Material Maker-style texture and material editor.
- [`texgen-backlog.md`](texgen-backlog.md) — what texgen still needs, what is deferred, and what is done.

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
- [`../benchmarks/README.md`](../benchmarks/README.md) — run archive, lenses, caveats
- [`../benchmarks/HISTORY.md`](../benchmarks/HISTORY.md) — every recorded run with deltas
- [`../crates/bench/README.md`](../crates/bench/README.md) — harness internals

## Conventions

- Task IDs (`M1-07`) are stable. Use them in commit messages.
- A task is done when its **Done when** column is satisfied *and* its **Verify** command
  passes on the reference machine.
- Benchmarks that gate a milestone are recorded through `scripts/bench.sh`, never quoted
  from an ad-hoc run.
