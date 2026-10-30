# Voxel Viewer

The M1.5 vertical slice: a `winit` + `wgpu` window that draws one generated
terrain chunk with the committed material atlas. It is the base the M2 content
pipeline replaces the naive mesher on.

## Run

```sh
cargo run -p voxel-viewer
```

The viewer opens at 1920×1080 and loads
`crates/engine/viewer/assets/atlas/atlas.png`, verifying it against
`atlas.hash.ron`. Regenerate the atlas with:

```sh
cargo run -p voxel-texgen -- bake-atlas
cargo test -p voxel-block texture_manifest
```

Set `VOXEL_VIEWER_ATLAS` to an `atlas.png` path or a temporary atlas directory to
override the committed one.

## Controls

| Input | Action |
| --- | --- |
| `W` / `A` / `S` / `D` | Move |
| `Space` / `Shift` | Fly up / down |
| `Ctrl` | Sprint |
| Arrow keys | Look without moving the pointer |
| Mouse | Look while the pointer is captured |
| `Esc` | Free the pointer; press again to recapture |
| `F11` | Toggle borderless fullscreen |
| `Q` | Quit |
| Mouse wheel | Change fly speed |

Bindings can be overridden by pointing `VOXEL_BINDINGS` at a file; see
[`src/input`](src/input). `VOXEL_SENSITIVITY`, `VOXEL_SMOOTH_CAMERA`,
`VOXEL_INVERT_X` and `VOXEL_INVERT_Y` configure mouse look, and
`VOXEL_TOGGLE_SPRINT=1` makes sprint latch.

## Validation

```sh
cargo test -p voxel-viewer --test validation
```

The test validates `src/shaders/chunk.wgsl` with naga and records an offscreen
chunk draw inside a wgpu validation error scope. Without an adapter it skips,
unless `VOXEL_REQUIRE_GPU=1` is set; CI uses that with a software Vulkan driver.

## Screenshots

The M1.5-15 acceptance set lives in `docs/images/`:

- [`m1.5-15-face-sheet.png`](../../../docs/images/m1.5-15-face-sheet.png) — slots
  1–40 row-major on an 8×5 grid, 256² each at 1:1.
- [`m1.5-15-grass-block.png`](../../../docs/images/m1.5-15-grass-block.png) — top
  and side view plus a bottom view.
- [`m1.5-15-sandstone-block.png`](../../../docs/images/m1.5-15-sandstone-block.png)
- [`m1.5-15-log-and-leaves.png`](../../../docs/images/m1.5-15-log-and-leaves.png)
- [`m1.5-15-ores.png`](../../../docs/images/m1.5-15-ores.png) — stone, coal, iron,
  gold and diamond.
- [`m1.5-15-translucent.png`](../../../docs/images/m1.5-15-translucent.png) — water
  and ice over the dark background.
- [`m1.5-15-chunk-1080p.png`](../../../docs/images/m1.5-15-chunk-1080p.png) — one
  chunk at near, mid and far distances.

Regenerate them with:

```sh
cargo run -p voxel-viewer --example m1_5_screenshots
```

The same command writes the M1.5-17 texel-density pair: the 256² M1.5-14
sources at the historical per-face density (`m1.5-17-before-256px-<hash>.png`)
and the committed 16×16-tile atlas (`m1.5-17-after-16px-<hash>.png`). Both sides
sample level 0 with nearest filtering so the pair isolates density from the
runtime mip path. `<hash>` is the first eight hex characters of
`atlas_png_blake3`. The current pair is
[`m1.5-17-before-256px-851e3b6e.png`](../../../docs/images/m1.5-17-before-256px-851e3b6e.png)
and
[`m1.5-17-after-16px-851e3b6e.png`](../../../docs/images/m1.5-17-after-16px-851e3b6e.png).
