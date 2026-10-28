# Voxel Texgen

A standalone Material Maker-style node editor for Voxel-Stuff textures and
material data. It uses the engine's deterministic fixed-point evaluator, so what
you see in the editor is exactly what `texgen check` reproduces in CI.

```sh
cargo run -p voxel-texgen          # open the editor
cargo run -p voxel-texgen -- check # verify every committed byte
```

## What it does

- Edits the committed face graphs under
  `crates/engine/block/textures/graph/`.
- Live previews the 256² source, the 16² atlas tile, and a 2×2 wrap.
- Generates the committed source PNGs, `textures.ron`, `materials.gen.ron`, the
  256² atlas and `atlas.hash.ron`.
- Browses the atlas slots and the block → six-face material table.
- Shares every byte-producing step with the viewer's `bake-atlas` command.
  There is no second encoder or filter in the app.

## Editor basics

| Action | Input |
| --- | --- |
| Pan | Middle-drag, or drag empty canvas |
| Zoom | Mouse wheel |
| Select | Left-click a node |
| Move node | Drag a node body |
| Connect | Drag from an output port (right) to an input port (left) |
| Add node | Right-click the canvas → category → node |
| Delete node | Select it, press `Delete` (dependents are removed too) |
| Duplicate | `Ctrl+D` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` |
| Save | `Ctrl+S` (saves the graph and starts Update all by default) |

The inspector edits graph properties, the output colour/alpha, and every op
parameter. Ramp stops and palette colours are editable in place. Positions are
stored in the graph RON under `ui`; the evaluator ignores them.

### Node library

| Family | Nodes |
| --- | --- |
| Source | Const, UV, Value Noise, FBM, Worley, Rings, Stripes |
| Pattern | Rect, H Band, Checker, Bricks, Wave, Spiral, Radial Gradient |
| Shape | Circle, Star, Polygon |
| Math | Add, Multiply, Minimum, Maximum, Invert, Remap, Smoothstep, Threshold, Quantize, Mix, Warp |
| Transform | Rotate, Mirror, Tile, Offset, Polar, Cartesian, Luminance |
| Colour | Ramp, Palette, Multiply, Overlay, Mix Colour, Brightness/Contrast, Saturate, Hue Shift |
| Blend | Screen, Colour Add, Colour Subtract, Difference, Lighten, Darken |
| Effect | Blur, Emboss, Normal Map, Bevel |
| Alpha | Alpha From Mask, Alpha Binary, Alpha Band |

`Circle`, `Star`, `Polygon`, `Rotate` and `Mirror` break both tiling axes;
`Polar` and `Cartesian` break the vertical axis. Use `Tiling: None` or
`Horizontal` in the graph header when they are in the output path. Blur,
emboss, normal map and bevel re-sample their input at fixed texel offsets, so
they cost more than pointwise nodes; preview at 128² while editing.

### Subgraphs

`File → Insert subgraph…` inlines another graph's nodes (with remapped ids) at
the current canvas position. `File → Save selection as subgraph…` writes the
selected nodes and their dependencies to
`crates/engine/block/textures/graph/_subgraphs/<name>.ron`. Inserting a
subgraph is flattening, not a reference: the committed PNG has no external
runtime dependency.

## OpenCode MCP server

The generator also ships `texgen-mcp`, a local MCP stdio server that OpenCode
can call as tools. It exposes the same core as the GUI and CLI:

| Tool | Purpose |
| --- | --- |
| `list_faces` | every face: slot, alpha/tiling, paths, hashes, staleness |
| `read_graph` | graph RON plus parse/compile status |
| `validate_graph` | validate a key or inline RON without writing |
| `render_preview` | source PNG at 64/128/256 plus the 16² tile, as images |
| `render_tile` | committed tile as a 128² PNG |
| `render_atlas` | 256² atlas plus hashes |
| `list_materials` | material rows and block six-face rows |
| `read_manifest` | committed `textures.ron` |
| `check` | verify every committed byte |
| `generate` | write PNGs, manifest, materials and atlas |
| `bake_atlas` | write only the atlas and its hash |

The server is read/preview/generate only. It never writes graph RON: edit
`textures/graph/*.ron` with normal file tools, call `validate_graph`, then
`generate`.

Project configuration (`opencode.jsonc` at the repository root):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "texgen": {
        "type": "local",
        "command": ["cargo", "run", "--quiet", "-p", "voxel-texgraph", "--bin", "texgen-mcp"],
        "cwd": ".",
        "timeout": { "startup": 120000, "catalog": 30000, "execution": 600000 }
      }
    }
  },
  "permissions": [
    { "action": "texgen_*", "resource": "*", "effect": "allow" }
  ]
}
```

Verify the connection:

```sh
opencode mcp list
```

The tool names OpenCode exposes are `texgen_list_faces`, `texgen_read_graph`,
and so on. The server also publishes `texgen://faces`, `texgen://manifest`,
`texgen://materials`, `texgen://atlas/hash` and `texgen://graph/<name>` as MCP
resources.

## CLI

```sh
cargo run -p voxel-texgen -- list
cargo run -p voxel-texgen -- generate          # PNGs + textures.ron + materials.gen.ron
cargo run -p voxel-texgen -- materials         # material table only
cargo run -p voxel-texgen -- bake-atlas        # atlas.png + atlas.hash.ron
cargo run -p voxel-texgen -- update-all        # generate + bake-atlas
cargo run -p voxel-texgen -- check             # fail on any byte difference
```

`--root PATH` selects the repository explicitly; otherwise the command walks up
from the current directory until it finds `crates/engine/block/data/blocks.ron`.

The viewer exposes the same bake:

```sh
cargo run -p voxel-viewer -- bake-atlas --check
```

Render any standalone graph RON without touching the committed set:

```sh
cargo run -p voxel-texgraph --example render -- graph.ron /tmp/preview.png 256
```

## Layout

```text
apps/texgen                     egui application + texgen binary
crates/engine/texgraph          deterministic core (no GUI dependency)
  graph.rs                      RON model, validation, canonical writer
  eval.rs                       Q16.16 evaluator
  png.rs                        the one RGBA8 encoder
  bake.rs                       box16-linear-premul-v1 + atlas + hashes
  pipeline.rs                   graphs → PNGs → manifest → materials → atlas
crates/engine/block/textures    committed graphs and source PNGs
crates/engine/block/data        committed manifest and material table
crates/engine/viewer/assets     committed atlas and atlas hash
```

## Contract

- No floats in the evaluator value path. The editor may use `f32` for layout
  and inspector display; that never reaches hashed bytes.
- Seeds come from the graph key through BLAKE3, or an explicit `seed`.
- Every spatial op wraps its lattice coordinates; `Rect` and `HBand` are
  tagged as breaking periodicity and are rejected under `Both` tiling.
- `texgen check` regenerates everything in memory and fails on one changed byte.
