# Start here

This is the human-readable side of the documentation. The guides explain how
the engine works in plain language; the **Reference** keeps the precise specs,
measurements and decision records that the code is held to.

If a guide and a spec disagree, the spec wins. Every guide links to the spec it
is summarising.

## What this engine is

A voxel world with a simple rule:

> **One block is one metre. One block is made of 16 × 16 × 16 tiny voxels.**

That gives a 6.25 cm voxel. You never see the tiny voxels directly — they exist
so a block can hold texture detail and wear, while the world stays cheap to
generate, mesh and stream.

The world is finite but very large: **1,024 km on a side and 12 km tall**, with
sea level at 6.6 km and peaks at 12 km. It has 18 biomes, rivers, lakes, caves,
ores, landmarks, ruins and forests, all generated from a seed and reproducible
bit for bit.

The renderer draws **1 m blocks**, not individual voxels. A chunk holds
**128³ voxels** (8 m on a side) at the target edge, and the current slice keeps
a 16 × 16 ring of those chunks around the camera.

## The one idea worth remembering

Voxel engines are usually limited by memory, not by triangles. This one spends
its complexity on three things:

1. **Don't store empty space.** The world is a sparse tree, so a cave or the sky
   costs almost nothing.
2. **Don't store every tiny voxel.** Generation produces a block grid plus a
   feature list; the mesher turns exposed block faces into merged quads.
3. **Don't stall the frame.** Generation, meshing and uploading happen off the
   frame thread, and the frame thread only installs finished work under a small
   time budget.

Everything else in the architecture follows from those three choices.

## The pieces, in one picture

```
seed ──> macro map ──> chunk generator ──> block grid
                                              │
                                              ▼
                                         greedy mesher ──> mesh + light + probes
                                              │
                                              ▼
camera ──> ring planner ──> workers ──> upload queue ──> frame thread ──> GPU
                                              ▲
                                         mesh pool
```

The **ring planner** decides which chunks should exist. Workers generate and
mesh them. The **upload queue** copies finished meshes into a pool a little at a
time. The frame thread installs them and draws the visible ones.

## What to read next

| If you want to… | Read |
|---|---|
| Understand the whole design | [Architecture tour](architecture-tour.md) |
| Understand the world generator | [Worldgen tour](worldgen-tour.md) |
| Understand the frame and streaming | [Rendering and streaming](rendering-and-streaming.md) |
| Understand the numbers and gates | [Performance and measurement](performance-and-measurement.md) |
| Look up a term | [Glossary](glossary.md) |
| See exactly where the project is | [Status](../status.md) |

## Guides vs Reference

**Guides** (this directory) are for learning: narrative, diagrams, analogies,
and links to the precise source.

**Reference** is everything else:

- [`specs/`](../specs) — the normative architecture, roadmap, task list,
  worldgen design, budgets and pre-registration.
- [`adr/`](../adr) — why each big decision was made.
- [`milestones/`](../milestones) — the measured evidence for every milestone.
- [`notes/`](../notes) — wgpu traps, backlog and interim scope.
- [`reference/`](../reference) — superseded designs, kept for their reasoning.
- [`../benchmarks/`](../../benchmarks) — the run archive and history.

The reference documents are dense on purpose. They are the source of truth for
a solo engineering project; the guides exist so you do not have to start there.

## A note on the numbers

The project uses **pre-registered gates**: the pass/fail numbers are written
down *before* a milestone is measured, and they are not renegotiated afterwards.
That is why a report can say "measured, but not confirmed" — the measurement
happened, but an open row or a later milestone still has to decide.

When a guide quotes a number, it names the document it came from. The
[glossary](glossary.md) explains p95, 1% low, residency and the other jargon.
