# SPEC: M1.5 Material and Texture Pipeline

Status: Draft v0.1
Input: "Requirements brief: M1.5 material and texture pipeline" (authoritative; this SPEC locks the decisions in its section 13).
Tasks: M1.5-06 (done), -07, -08, -09, -12, -13, -14, -15, -17.

---

## 0. Decision summary (section 13 lock list)

| # | Decision | Locked value |
| --- | ---------- | -------------- |
| D1 | Manifest fields / enums | `textures.ron` schema 1, fields in 4.2; `Opaque`, `Cutout`, `Translucent` |
| D2 | Licence/provenance | Top-level, one each (`CC0-1.0`); per-entry override allowed later |
| D3 | Bake outputs | `crates/engine/viewer/assets/atlas/atlas.png` + `atlas.hash.ron` |
| D4 | Downsample filter | 16x16 box, linear light, alpha-weighted, integer math, round-half-up (6.2) |
| D5 | Material table file | `crates/engine/block/data/materials.gen.ron` (7.3) |
| D6 | Block vs material | `BlockId` = identity; `MaterialId(u8)` = face variant; `FaceMaterials` = 6 ids (7.1) |
| D7 | PNG encoder | `png` crate pinned with `=`, RGBA8, no ancillary chunks, fixed compression/filter (5.4) |
| D8 | Seed/algorithm | Node-graph evaluator, integer/fixed-point only, per-texture seed from BLAKE3 of key (8) |
| D9 | Screenshots | Fixed set in section 11 |

Architecture in one line: **RON node graphs -> (CPU deterministic evaluator) -> 256² PNGs -> manifest -> bake-atlas -> 256² atlas + hash -> GPU.**

---

## 1. Scope

In: the committed face set (23 M1.5 faces plus 17 stone and tree faces added later), manifest, material variants, atlas bake, determinism, node-graph generator (CPU).
Out (unchanged from brief): damage/AO/light textures, transition variants, UI sprites, water animation, 320² atlas, BC7, mips beyond level 4, new block keys, runtime reading of resource-pack layouts.
Also out for M1.5: the GUI node editor itself and GPU preview (hook defined in section 9).

Hard rules:

- Nothing in the repo reads the "Default HD 256x Demo" pack or any Minecraft install. It is private visual reference only.
- A clean checkout builds, tests, and bakes without any file outside the repo.
- `voxel-core::atlas` is the only place atlas constants are defined. Everyone else imports.

## 2. Existing contract (imported, not redefined)

From `voxel-core::atlas`: `ATLAS_SIZE=256`, `TILE_SIZE=16`, `TILES_PER_EDGE=16`, `MAX_MATERIALS=256`, `MAX_MIP=4`, `MIP_LEVELS=5`, `tile_origin(tile)`, `uv(tile, face_pos)`.
From `voxel-viewer::atlas`: `AtlasBuilder` (`set_tile`, `build`), `TILE_BYTES=1024`, per-tile mips.
The bake tool and the generator must use these; a grep test (`no_atlas_constant_redefinition`) fails if `256`/`16` atlas constants are redeclared outside `voxel-core::atlas`.

## 3. Face set

Slots 1..=40 exactly as in the committed slice table. Slot 0 = air, never sampled. Slots 41..=255 transparent black. New keys are appended by extending `pipeline::SLICES` and bumping `texture_manifest::SLICE_FACE_COUNT`.

Tiling requirement (clarification of brief section 3, see 12.1):

| Tiling | Meaning | Faces |
| -------- | --------- | ------- |
| `Both` | wraps left-right and top-bottom | all faces except below |
| `Horizontal` | wraps left-right only | `grass_block_side`, `sandstone_side` |
| `None` | no wrap required; content keeps >= 8 px margin on left/right | `tall_grass`, `red_flower` |

Reason: a side face with a green fringe on top and dirt on the bottom cannot wrap vertically without a seam, and a plant sprite rises from the bottom edge.

## 4. Source PNGs and manifest

### 4.1 Source PNGs

- Path: `crates/engine/block/textures/block/<name>.png`, `<name>` = key without `voxel:block/`.
- 256x256, 8-bit RGBA, PNG colour type 6, sRGB by convention (no colour chunks written).
- Must be readable at 16x16 and at 1x1 per tile (mip 4): low-frequency colour identity first, detail second. Generator graphs must be authored against the 16² preview, not the 256² one.
- Source of record is the committed PNG. A graph (section 8) is the source of the PNG but PNG bytes are what get hashed.

### 4.2 Manifest: `crates/engine/block/data/textures.ron`

```ron
(
    schema: 1,
    license: "CC0-1.0",
    provenance: "Original artwork procedurally generated for Voxel-Stuff by texgen (graphs in textures/graph/)",
    textures: [
        (
            key: "voxel:block/stone",
            slot: 1,
            source: "textures/block/stone.png",
            alpha: Opaque,          // Opaque | Cutout | Translucent
            tiling: Both,           // Both | Horizontal | None
            hash: "<64 lowercase hex, BLAKE3 of PNG file bytes>",
            graph: Some("textures/graph/stone.ron"),
            graph_hash: Some("<64 lowercase hex, BLAKE3 of graph file bytes>"),
            placeholder: false,
        ),
        // ... 22 more, sorted by slot ascending
    ],
)
```

Rules:

- Entries sorted by `slot`; file is emitted by tooling in canonical RON formatting (stable field order, 4-space indent, trailing newline) so diffs are minimal.
- Paths are relative to `crates/engine/block/`, forward slashes, no `..`, must canonicalize inside the crate.
- `placeholder: true` is allowed during development but fails `texture_manifest` in the final set.
- `graph`/`graph_hash` are `None` only for hand-authored faces (none in M1.5).

### 4.3 `cargo test -p voxel-block texture_manifest` checks

1. Parses; `schema == 1`.
2. Key set == distinct `voxel:block/*` keys in `blocks.ron` (no missing, no extra); exactly `SLICE_FACE_COUNT` (currently 40).
3. Slots unique, exactly `1..=40`, all <= 255.
4. File exists, resolves inside crate, decodes as 256x256 RGBA8 colour type 6.
5. BLAKE3(file bytes) == `hash`.
6. `alpha` equals the strictest alpha class required by every block using the face (all users of a key must agree on RenderType; disagreement = error `RenderTypeConflict`).
7. Alpha invariants on pixels (section 4.4).
8. Tiling invariant (4.5).
9. `license`, `provenance` non-empty; no `placeholder: true`.
10. If `graph` is set: file exists and `graph_hash` matches.

### 4.4 Alpha invariants (testable)

- Opaque: all texels alpha == 255.
- Cutout: all texels alpha in {0, 255}; at least one of each (leaves/grass/flower must really have gaps and body).
- Translucent: at least one texel alpha in 1..=254, not all 255. Generator target band: 140..=220.

### 4.5 Tiling invariant

Seam metric: mean absolute RGBA difference between column 0 and column 255 (and row 0 / row 255 for `Both`) compared with the mean difference between adjacent interior columns/rows. Pass if seam <= 1.5x interior adjacent difference. Because generator graphs are periodic by construction (section 8.3), this is a regression guard, not the mechanism.

### 4.6 Sandstone agreement

`sandstone_side` top band (rows 0..16) mean colour within delta (per-channel <= 6/255) of `sandstone_top` mean colour; bottom band (rows 240..256) vs `sandstone_bottom`. Implemented by sharing palette constants in the three graphs and verified by test `sandstone_faces_agree`.

## 5. Generation tooling

### 5.1 Crate layout (implemented)

- `crates/engine/texgraph` (lib `voxel-texgraph`): graph model, RON (de)serialization, validation, CPU evaluator, PNG encoder, bake pipeline. No GPU or UI dependencies.
- `apps/texgen` (app `voxel-texgen`): the standalone Material Maker-style egui editor and the `texgen` binary:
  - `texgen` with no command opens the editor.
  - `texgen generate` writes the committed PNGs, `textures.ron` and `materials.gen.ron`.
  - `texgen update-manifest` is an alias for `generate`; `texgen update-all` also bakes the atlas.
  - `texgen check` regenerates in memory and fails if any committed PNG, hash, material row or atlas byte differs.
  - `texgen bake-atlas [--check]` writes or verifies `atlas.png` and `atlas.hash.ron`.
- Graphs: `crates/engine/block/textures/graph/<name>.ron`. Reusable subgraphs are flattened into the graph and live under `textures/graph/_subgraphs/`. Docs: `apps/texgen/README.md`.

### 5.2 Rules

- Fixed seeds, no clock, no env, no threads that affect output ordering (parallel by row is fine if each pixel is pure).
- Never reads outside the repo. Never touches the ARR pack.
- Emits exactly the committed face files, at the manifest paths, nothing else.
- Committed outputs: graphs, PNGs, manifest. Clean checkout needs no generator run.

### 5.3 Determinism

No `f32`/`f64` anywhere in the evaluator's value path. Fixed-point `Q16.16` (`i32`, `i64` intermediates), integer hashing, integer sqrt, integer smoothstep. CI runs `texgen check` on Linux and Windows; outputs must match.

### 5.4 PNG encoder settings

- `png` crate pinned with `=x.y.z` in `Cargo.toml` (version chosen at implementation, recorded in `Cargo.lock`; changing it requires regenerating hashes in the same PR).
- Colour type RGBA, depth 8, non-interlaced, `Compression::Best`, `FilterType::Paeth`, adaptive filtering off.
- No `tIME`, `tEXt`, `gAMA`, `sRGB`, `iCCP`, or other ancillary chunks.
- The same encoder function (in `voxel-texgraph`) is used by `texgen` and `bake-atlas`.
- If an encoder bump changes bytes, `texgen update-manifest` is the single sanctioned fix.

## 6. Bake tool

Command: `cargo run -p voxel-viewer -- bake-atlas [--check]`

### 6.1 Pipeline

1. Load and validate `textures.ron` fully (same validator as `texture_manifest`, shared code in `voxel-block`) before any write.
2. Load each source; reject missing file, wrong size, wrong colour type, hash mismatch, duplicate/out-of-range slot, gap in 1..=N, count > 256.
3. Downsample each 256² source to a 16x16 tile (6.2).
4. `AtlasBuilder::set_tile(slot, tile)`; call `build()`; assert no tile crosses a mip boundary (checked via `tile_origin` alignment at all 5 levels).
5. Write outputs (6.3). Write is atomic: temp file + rename; nothing is written if any step fails.

`--check`: do everything except write; compare to committed `atlas.png` and `atlas.hash.ron`; non-zero exit on difference. This is the "clean checkout reproduces the hash" test.

### 6.2 Downsample filter `box16-linear-premul-v1`

- Each output texel = exact 16x16 box of source texels (256/16). No resampling kernel, no float.
- Decode sRGB8 -> linear using a const 256-entry `u16` LUT (0..=65535).
- Weight each texel by its alpha: accumulate `sum(lin * a)` and `sum(a)` in `u64`.
- Colour = `sum(lin*a) / sum(a)` with round-half-up; re-encode to sRGB8 by const inverse LUT (nearest entry; tie goes up). If `sum(a)==0`, rgb = 0.
- Alpha by class:
  - Opaque: 255.
  - Cutout: coverage = `sum(a) / (256*255)`; alpha = 255 if coverage >= 0.5 (integer compare `2*sum(a) >= 256*255`) else 0. Colour taken from opaque-weighted average as above, also for alpha-0 texels (limits dark fringes).
  - Translucent: mean alpha, round-half-up.
- The filter id string is written into `atlas.hash.ron`; changing the filter requires a new id.

### 6.3 Outputs (D3)

- `crates/engine/viewer/assets/atlas/atlas.png`: 256x256 RGBA8 base level, encoder per 5.4. Mips are not stored; `AtlasBuilder` regenerates them at load (per tile, never across tiles).
- `crates/engine/viewer/assets/atlas/atlas.hash.ron`:

```ron
(
    schema: 1,
    filter: "box16-linear-premul-v1",
    tile_count: 40,
    manifest_blake3: "<hex of textures.ron bytes>",
    atlas_png_blake3: "<hex of atlas.png bytes>",
    mip_chain_blake3: "<hex of level0..level4 RGBA bytes concatenated, level 0 first>",
)
```

- Both files are committed. Running the command twice (or on a clean checkout) must produce byte-identical files.

### 6.4 Runtime guard

The viewer, at startup, verifies `atlas.hash.ron` against `atlas.png` and the manifest hash. Mismatch or a missing slot in 1..=40 is a hard error (`AtlasError::...`). There is no magenta/placeholder fallback.

## 7. Material model

### 7.1 Types (D6)

```rust
pub struct BlockId(u16);           // identity, dense, from blocks.ron
pub struct MaterialId(u8);         // 0 = air, 1..=40 drawable; equals atlas slot
pub struct FaceMaterials { pub top: MaterialId, pub bottom: MaterialId,
                           pub north: MaterialId, pub south: MaterialId,
                           pub east: MaterialId, pub west: MaterialId }
```

- `MaterialId` equals the atlas slot, so `tile_origin(material.0 as u32)` is the lookup; no extra table at render time.
- Registry holds `faces: Vec<FaceMaterials>` indexed by `BlockId`. The per-block six-face `TextureSet` indirection is removed from the render path.
- Face names follow the existing `Face` enum in `voxel-block`; if its axis naming differs, rename fields to match, the semantic stays.
- Examples: `grass_block` top=`grass_block_top` (3), sides=`grass_block_side` (4), bottom=`dirt` (2). `oak_log` caps=9, sides=10. `sandstone` top=21, bottom=22, sides=23.

### 7.2 Registry rules

- Material table is dense and `len <= MAX_MATERIALS (256)`.
- Typed errors (`thiserror`):
  `MaterialRegistryError::{ CapExceeded { attempted: usize, cap: usize }, DuplicateKey(String), DuplicateSlot(u8), SlotGap { expected: u8, found: u8 }, MissingManifestEntry(String), UnusedManifestEntry(String) }`.
- 257th registration returns `CapExceeded`; no truncation, no renumbering.
- No variants for damage, AO, light, transition, UI.
- The existing 64-block fixture must still freeze; test `fixture_64_blocks_freeze_after_m1_5_08`.

### 7.3 Generated table (D5)

- File: `crates/engine/block/data/materials.gen.ron`, committed.
- Generator: `cargo run -p voxel-block --bin gen-materials` (reads `textures.ron` + `blocks.ron`, writes the file, canonical formatting).
- Representation:

```ron
(
    schema: 1,
    manifest_blake3: "<hex of textures.ron>",
    blocks_blake3: "<hex of blocks.ron>",
    materials: [
        (id: 1, key: "voxel:block/stone", alpha: Opaque),
        // ... through id 40
    ],
    faces: [
        (block: "voxel:stone", top: 1, bottom: 1, north: 1, south: 1, east: 1, west: 1),
        // ... one per block in blocks.ron order (33 blocks)
    ],
)
```

- Test `materials_gen_is_current`: regenerate in memory, byte-compare with committed file.
- Generator rejects: face key with no manifest entry, manifest entry with no face key, duplicate slot or key, gap, count > 256.

## 8. Node graph format and evaluator

### 8.1 Graph file (`textures/graph/<name>.ron`)

```ron
(
    schema: 1,
    key: "voxel:block/stone",
    size: 256,
    seed: 0,                       // 0 = derive from key (8.2); nonzero overrides
    nodes: [
        (id: 0, op: ValueNoise(freq: 8, octaves: 3), inputs: []),
        (id: 1, op: Ramp(stops: [(0, "#6b6b6b"), (65535, "#8a8a8a")]), inputs: [0]),
        // ...
    ],
    output: Rgba(color: 4, alpha: Const(65535)),
)
```

- Directed acyclic graph, nodes listed in topological order, ids unique and referenced only backwards. Validator rejects cycles, dangling inputs, type mismatches, unknown ops. The editor stores optional `ui: (x, y)` node positions; the evaluator ignores them.
- Values are fixed-point scalars (`Q16.16`, nominal 0..=65535 for masks) or RGBA8 colours.

### 8.2 Seed

`seed = u64 from first 8 bytes (LE) of BLAKE3("voxel-stuff/texgen/v1/" + key)` unless overridden. Per-node stream = `splitmix64(seed ^ node_id * 0x9E3779B97F4A7C15)`. Integer lattice hash for noise; no `rand` crate in the evaluator.

### 8.3 Periodicity

Every spatial op takes a period in lattice cells and wraps lattice coordinates modulo that period, so a texture built only from periodic ops tiles on both axes. Ops that break periodicity (e.g. `HBand`, `Rect`) are tagged `breaks_v` / `breaks_h`; the validator checks that a graph's declared `tiling` is consistent with the ops it uses.

### 8.4 Initial op set (enough for all 40 faces)

- Sources: `Const`, `Uv`, `ValueNoise`, `Worley` (F1/F2/edge), `Fbm`, `Rings` (integer sqrt), `Stripes` (axis, count, wobble), `Rect`, `HBand`.
- Patterns and shapes: `Checker`, `Bricks`, `Wave`, `Spiral`, `RadialGradient`, `Circle`, `Star`, `Polygon`.
- Domain transforms: `Rotate`, `Mirror`, `Tile`, `Offset`, `Polar`, `Cartesian`, `Luminance`.
- Math: `Add`, `Mul`, `Min`, `Max`, `Invert`, `Remap`, `Smoothstep`, `Threshold`, `Quantize`, `Mix(mask)`, `MixColor(mask)`.
- Warp: `Warp(by: node, amount)`.
- Colour: `Ramp` (gradient to RGBA), `Palette`, `Multiply`, `Overlay`, `MixColor`, `BrightnessContrast`, `Saturate`, `HueShift`.
- Blend: `Screen`, `AddColor`, `Subtract`, `Difference`, `Lighten`, `Darken`.
- Effects: `Blur`, `Emboss`, `NormalMap`, `Bevel` (all re-sample their colour input at fixed texel offsets).
- Alpha: `AlphaFromMask`, `AlphaBinary(threshold)` (guarantees Cutout), `AlphaBand(min, max)` (Translucent).
- Output: `Rgba`.

### 8.5 Recipes per face (art direction, adjustable)

| Slot | Face | Recipe | Anchor colours (sRGB) |
| ------ | ------ | -------- | ---------------------- |
| 1 | stone | Fbm + sparse Worley-edge cracks, 2-tone ramp | #7d7d7d #5e5e5e |
| 2 | dirt | Fbm + speck Threshold + a few Worley pebbles | #7a5638 #4a3322 #9b7a58 |
| 3 | grass_block_top | Fbm + fine Stripes blade variation | #5fa83a #3f7a28 |
| 4 | grass_block_side | dirt graph + Warp'd HBand fringe whose mean colour matches slot 3 | dirt + #5fa83a |
| 5 | cobblestone | Worley cells, edge -> mortar | #8a8a8a #3a3a3a |
| 6 | sand | fine noise + sparse dark Threshold grains | #e2d49a #c9b97a |
| 7 | gravel | Worley small cells, mixed grey ramp, dark gaps | #8c8885 #4a4846 |
| 8 | water_still | low-contrast Worley ripple, alpha 150..190 | #2f7fa8 #3a93b8 |
| 9 | oak_log_top | Rings + dark bark rim (Rect-free, radial) | #a98550 #6b4f2a |
| 10 | oak_log | vertical Stripes + Warp, groove Threshold | #6b4f2a #4a361c |
| 11 | oak_leaves | clustered Worley, AlphaBinary gaps | #3f8a2c #2a6a1c |
| 12 | bedrock | Fbm high contrast, patch Threshold | #2b2b2b #0f0f0f |
| 13 | coal_ore | stone graph + Worley clusters | stone + #1c1c1c |
| 14 | iron_ore | stone graph + clusters | stone + #c99a6a |
| 15 | gold_ore | stone graph + clusters | stone + #f2d23c |
| 16 | diamond_ore | stone graph + clusters | stone + #4fe3e0 |
| 17 | tall_grass | blades via Stripes from bottom, AlphaBinary | #5fa83a #3f7a28 |
| 18 | red_flower | stem + leaves + radial head, AlphaBinary | #c8282a #3f7a28 |
| 19 | snow | very low-amplitude noise | #f4f8fc #dde6ef |
| 20 | ice | pale blue + crack lines (Worley edge) + bubble specks, alpha 160..210 | #b8dcf0 #8fc3e4 |
| 21 | sandstone_top | sand-grain noise | #dccf94 |
| 22 | sandstone_bottom | rougher/darker sand-grain | #c4b57a |
| 23 | sandstone_side | horizontal strata; top/bottom bands use 21/22 palette | #d2c58a #bfae70 |

Ore faces share the `stone` subgraph (same seed-derived noise on purpose) so they read as stone variants. Subgraph reuse is by an `Include("textures/graph/_stone_base.ron")` op; included files are hashed into `graph_hash`.

### 8.6 Post-M1.5 stone and tree extension (slots 24-40)

Added after the M1.5 slice. Every face tiles on both axes; leaves use `AlphaBinary` cutout gaps. The three wood types each contribute a top, a side and a leaf face.

| Slot | Face | Recipe | Anchor colours (sRGB) |
| ------ | ------ | -------- | ---------------------- |
| 24 | stone_bricks | 4x4 offset `Bricks`, mortar mask, grained brick faces | #8e8e8e #6a6a6a #4a4a4a |
| 25 | mossy_cobblestone | cobblestone base + smoothstep Fbm moss patches | #8a8a8a #3a3a3a #4e6a2c |
| 26 | smooth_stone | low-contrast Fbm + faint `Stripes` banding | #a4a4a4 #707070 |
| 27 | granite | pink Fbm base, fine Worley grain, sparse light specks | #b08070 #8a5a4a |
| 28 | diorite | pale grey Fbm base, fine Worley grain, dark specks | #eeeeee #c0c0c0 #5a5a5a |
| 29 | andesite | mid-grey Fbm base, fine Worley grain, dark specks | #9e9e9e #747474 #4e4e4e |
| 30 | deepslate | dark vertical grain + soft mineral highlight | #4a4a52 #222226 |
| 31 | tuff | olive-grey base + Worley-edge pores | #8a8f7a #6a6f60 #565b4a |
| 32 | birch_log_top | warped Rings + pale grain | #f0e8d4 #d0c19c |
| 33 | birch_log | wavy vertical Stripes + sparse horizontal lenticels | #efe6cc #d8cba8 #5a5240 |
| 34 | birch_leaves | clustered Worley, `AlphaBinary` gaps | #8ab84a #5a8a2c |
| 35 | spruce_log_top | warped Rings, dark brown | #6b4f2a #4a3320 |
| 36 | spruce_log | tight vertical grain, sparse highlights | #5a4028 #3a2a1a |
| 37 | spruce_leaves | clustered Worley, `AlphaBinary` gaps | #35704a #1f4a2a |
| 38 | dark_oak_log_top | warped Rings, very dark brown | #5a4028 #3a2a18 |
| 39 | dark_oak_log | loose vertical grain, sparse highlights | #42301c #2a1e12 |
| 40 | dark_oak_leaves | clustered Worley, `AlphaBinary` gaps | #2e5c2a #1a3d1a |

The three leaf faces reuse the oak-leaves structure (Worley F1 + ValueNoise + `AlphaBinary`) with different periods and palettes, which keeps their cutout coverage and seam behaviour comparable.

## 9. Node editor and GPU hook (not built in M1.5)

Recommendation: **the CPU evaluator is the authority; the GPU is a preview.**

- The future node editor reads and writes the same graph RON as `voxel-texgraph`. No second format.
- A GPU (WGSL) evaluator may render live previews, but is non-authoritative: it is never used by `texgen` or `bake-atlas`, because GPU float math is not byte-identical across drivers.
- "Save" in the editor = write graph RON, run CPU evaluator, run `update-manifest`. Hashes always come from the CPU path.
- Optional later: a parity test comparing the GPU preview to the CPU output with a tolerance (e.g. max per-channel error <= 2) to catch editor drift. Not an M1.5 gate.
- To keep this door open, `voxel-texgraph` must have no dependency on `wgpu`/UI crates, and the op set is defined declaratively (op enum + fixed-point semantics doc) so a WGSL port can follow it.

## 10. Verification

| Command | Must prove |
| --------- | ------------ |
| `cargo test -p voxel-block texture_manifest` | 4.3 list |
| `cargo test -p voxel-block materials_gen_is_current` | generated table matches |
| `cargo test -p voxel-block material_registry` | cap error, dup/gap/missing errors, 64-block fixture freeze |
| `cargo test -p voxel-texgraph` | validator, determinism (generate twice, same bytes), op unit tests |
| `cargo run -p voxel-texgraph --bin texgen -- check` | committed PNGs and hashes == regenerated |
| `cargo test -p voxel-viewer atlas` | 256 tiles addressable, 5 mips, per-tile filtering, no tile crosses mip boundary, no cross-tile bleed |
| `cargo run -p voxel-viewer -- bake-atlas` x2 and `--check` | identical bytes and hash, matches committed |

No-bleed test: fill tile A with solid red and tile B (neighbour) with solid blue; at every mip level A's texels stay pure red.
Clean-checkout CI job: fresh clone, no generator run, run all of the above.

## 11. Visual acceptance (D9)

M1.5-12/13 gates: one 1 m face shows exactly one 16x16 tile; one chunk at 60 Hz at 1080p.

M1.5-15 screenshots (committed next to `atlas.hash.ron`, filename includes first 8 hex of `atlas_png_blake3`):

1. `face-sheet`: all 40 faces on an 8x5 grid, nearest filtering, 1:1 (the M1.5 review sheet is the 23 M1.5 faces on a 6x4 grid).
2. `grass-block`: top, side and bottom visible, side fringe matches top.
3. `sandstone-block`: three faces agree at edges.
4. `log-and-leaves`: oak log + leaves cutout gaps.
5. `ores`: four ores beside stone.
6. `translucent`: water and ice over a dark background.
7. `chunk-1080p`: one chunk, three camera distances (near, mid, far) to show mips.

M1.5-17: before/after texel-density screenshots. Proposed: before = same chunk with level-0 only and nearest, after = trilinear with full mip chain. Needs your confirmation (12.4).

## 12. Open items for you

1. **Tiling vs "all four edges".** Brief says every source tiles on all four edges; that is impossible for `grass_block_side`, `sandstone_side` (vertical), and plants. This SPEC uses the `tiling` field (section 3). Confirm.
2. **Face enum naming.** Section 7.1 assumes names `top/bottom/north/south/east/west`; match to the real `Face` enum.
3. **Cutout mip alpha.** `AtlasBuilder` averages alpha at mips (assumed). Alpha-tested leaves will thin out at distance; options: alpha-coverage rescale in the mip filter (changes M1.5-06 code) or accept. Not decided here.
4. **M1.5-17 definition** (11) is a guess.
5. **Crate names/paths** for `voxel-texgraph` and `gen-materials` are proposals.
6. **Palette/recipes** (8.5) are my art direction; tweak freely before the generator is written.

---

## 13. Implementation status (2026-10-27)

The generator is implemented and runnable:

- `cargo run -p voxel-texgen` opens the editor; `cargo run -p voxel-texgen -- check` verifies the committed set.
- The deterministic core is `voxel-texgraph`; the app is `voxel-texgen` and contains no byte-producing logic of its own.
- All 40 graphs, PNGs, `textures.ron`, `materials.gen.ron`, `atlas.png` and `atlas.hash.ron` are committed.
- The evaluator adds `MixColor`, the pattern/shape nodes, domain transforms and the colour/effect nodes listed in section 8.4.
- Neighbourhood effects (`Blur`, `Emboss`, `NormalMap`, `Bevel`) re-sample their input at fixed 256-grid texel offsets. They preserve periodicity because the offsets wrap, but cost more than pointwise ops; preview at 128² while authoring.
- `Circle`, `Star`, `Polygon`, `Rotate` and `Mirror` are tagged as breaking both axes; `Polar` and `Cartesian` break the vertical axis. The validator rejects them under incompatible tiling.
- The bake filter is `box16-linear-premul-v1` and the PNG encoder is `png =0.17.16` with the section 5.4 settings.
- The material table is now consumable at runtime: `BlockRegistryBuilder::load_ron_with_materials` parses `materials.gen.ron`, `BlockRegistry::face_materials(block)` returns the six atlas slots, and `BlockRecord` carries them. `cargo test -p voxel-block cap_256`, `material_registry` and `fixture_64_blocks_freeze_after_m1_5_08` cover the cap, typed errors and the 64-block freeze. The legacy `TextureSetId` path remains for the texture-array renderer until M1.5-12 switches the chunk shader to atlas slots.
- `texgen-mcp` exposes the pipeline to OpenCode over MCP stdio (`list_faces`, `read_graph`, `validate_graph`, `render_preview`, `render_tile`, `render_atlas`, `list_materials`, `read_manifest`, `check`, `generate`, `bake_atlas`). It never writes graph RON.
- Screenshots (section 11) and the remaining M1.5 renderer tasks are not part of this generator change.
