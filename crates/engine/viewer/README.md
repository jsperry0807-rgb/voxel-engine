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

## Screenshot

The committed slice screenshot is
[`docs/images/m1.5-13-chunk-1080p.png`](../../../docs/images/m1.5-13-chunk-1080p.png).
To reproduce it, run the viewer from the repository root at the default
1920×1080 window size and capture the generated chunk. This is one image of the
M1.5-15 set; the remaining face-sheet/material shots and the M1.5-17 texel-density
pair are still open.
