# Texgen backlog

Status: living document, 2026-10-31.

This is the gap list for `apps/texgen` and `crates/engine/texgraph`. It records
what the generator still needs, what is deliberately deferred, and why. When a
task is completed, move it to the **Done** section rather than deleting it.

Priority order is the order a single engineer should take it. An item marked
**next** blocks the milestone named next to it.

## 1. Must-have before the next texture sets

Nothing open: the data-driven face table and incremental generation are both in
the **Done** section. The MCP key-list form is item 14.

## 2. Editor and UX

| # | Item | Why it matters | Status |
| --- | --- | --- | --- |
| 4 | **Graph editing ergonomics** — box-select, multi-select, copy/paste, detach/reconnect wires, fit-to-view, autosave, unsaved-changes prompt on quit | Undo/redo and subgraph flattening exist, but the canvas is still single-node-oriented. | open |
| 5 | **Async preview evaluation** — evaluate previews on a worker with a generation counter | 256² graphs with Blur/Normal Map/Bevel currently evaluate on the UI thread and can stall. | open |
| 6 | **Better visual inspection** — 3D block/cube preview, RGB/A/luminance channel views, mip-level view | Current preview is 2D only; these views would speed art review. The M1.5-17 density pair is closed and does not block this. | open |
| 7 | **Node readability** — parameter summaries on nodes and per-node compile errors | Nodes show `Op #id` only. Larger graphs are hard to scan. | open |
| 8 | **Graph file watching** — reload an externally edited graph, or warn that it changed on disk | OpenCode and the GUI can edit the same file; today the GUI silently keeps its in-memory copy. | open |

## 3. Correctness and CI

| # | Item | Why it matters | Status |
| --- | --- | --- | --- |
| 9 | **Graph complexity limits** — node-count, chain-depth and fan-out caps; property/fuzz tests for validation and evaluation | A pathological graph can be very slow or recurse deeply. Current validation only checks topology and parameter ranges. | open |
| 10 | **Windows determinism job** — run `texgen check` on Linux and Windows | Spec 5.3 requires identical bytes on both. CI currently runs the texture job on Ubuntu only. | open |
| 11 | **Cutout mip alpha decision** — alpha-coverage rescale in the mip filter, or accept thinning | Still open in generator spec §12.3. Leaves will thin at distance either way. | open |
| 12 | **Lock the remaining spec open items** — face enum naming, tiling confirmation | `generator.spec.md` is Draft v0.2 with open questions. | open |
| 17 | **Worley periodicity** — feature-point distance is not wrapped across tile edges | `worley` wraps the hash cell indices but computes distance to the unwrapped neighbour, so a face built on Worley can show a seam even though it declares `Tiling::Both`. New M2.5 tileable faces avoid it; fix the distance or forbid Worley on tiling faces. | open |

## 4. MCP / OpenCode

| # | Item | Why it matters | Status |
| --- | --- | --- | --- |
| 13 | **Render arbitrary RON** — `render_graph` MCP tool for inline graph RON | `validate_graph` accepts inline RON but `render_preview` only accepts a slice key, so an agent cannot see a proposed graph before writing it. | open |
| 15 | **MCP resource for the atlas PNG** — expose `texgen://atlas.png` as an image resource | Currently only the hash is a resource; the image is returned by `render_atlas`. | open |

## 5. Art

| # | Item | Why it matters | Status |
| --- | --- | --- | --- |
| 16 | **Slice art review** — use the new pattern/effect nodes to improve cobblestone, sandstone, water, foliage | The slice set passes the technical tests but is still placeholder art in places. | open |

## 6. Not needed yet (deliberately deferred)

These are out of scope for M1.5 and the M2.5/M3.5 texture sets. Do not build them
without a milestone that consumes them.

- **GPU preview / GPU node evaluation.** The CPU evaluator is authoritative. A
  WGSL preview could come after the editor stabilises, with a tolerance parity
  test; it must never write committed bytes.
- **Water animation and flow textures.** `water_flow`, `foam_strip` and caustics
  belong to M3.5-15, after the static slice and the M2.5 sets.
- **External resource-pack ingestion.** The pipeline must never read a Minecraft
  install or third-party pack. Procedural graphs are the source of record.
- **320² atlas, BC7, mips beyond level 4, texture arrays as the final runtime
  format.** These are separate renderer decisions. The current 256² atlas and
  five mips are locked.
- **Damage, AO, light and transition textures.** They need engine-side consumers
  first.
- **UI sprites.** They live outside the block atlas.
- **Third-party node/plugin API.** The op set is internal until the graph format
  has survived the M2.5 and M3.5 sets.
- **Paint layers, image import, brush tools.** Texgen is procedural, not a
  raster paint app.
- **PBR channel packing (ORM), normal-map authoring as a runtime material
  system.** The engine does not consume those channels yet.
- **Animation timeline / animated textures.** No consumer exists.
- **Network or server-side generation.** Generation is a build-time tool.
- **Cloud asset library / collaborative editing.** Not a v0.1 feature.
- **A real-time 3D material sphere.** A cube/block preview is useful (item 6);
  a full PBR viewer is not needed yet.

## Done

| Item | Commit / note |
| --- | --- |
| Deterministic graph core, PNG encoder, manifest, atlas bake | `32da527` |
| 23 M1.5 graphs, source PNGs, `textures.ron`, `materials.gen.ron`, atlas | `32da527` |
| Standalone egui editor with live preview, node canvas, subgraphs, atlas/material tabs | `32da527` |
| `texgen-mcp` for OpenCode | `32da527` |
| Pattern, shape, transform, colour, blend and effect nodes | `f64babb` |
| `examples/render` for arbitrary graph RON | `f64babb` |
| M1.5-08 material registry core: `FaceMaterials`, atlas `MaterialId`, `materials.gen.ron` loading, `BlockRegistry::face_materials`, 64-block fixture | this change |
| M1.5-12 chunk shader reads atlas slots; legacy `TextureSetId` path is unused by the viewer | M1.5-12 |
| 17 post-M1.5 stone and tree faces (slots 24-40), 14 blocks, regenerated manifest, material table and atlas | this change |
| M1.5-15 screenshot set and M1.5-17 before/after texel-density pair | `m1.5-15-*` / `m1.5-17-*` |
| Backlog document with deferred work | this change |
| Data-driven face table: `data/faces.ron` drives slots in the pipeline, editor and MCP; `FaceTable` validation replaces `SLICES`/`SLICE_FACE_COUNT` | this change |
| Incremental generation: `texgen generate --key FACE` (repeatable) renders only the selected faces and writes PNG/manifest/materials/atlas only when their bytes change; `check --key` verifies the same subset | this change |
| M2.5-30/31 art data: 29 biome-surface, underground, structure and flora faces (slots 41–69), 22 block rows, regenerated manifest, material table and atlas | this change |
| MCP `generate`/`check` accept an optional `keys` array and route to the per-key pipeline | this change |
