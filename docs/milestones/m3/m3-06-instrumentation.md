# M3-06 frame instrumentation

**Status:** landed 2026-10-31. Each frame reports CPU submit time, GPU pass
time, visible/drawn chunks and the geometry they cover (blocks, quads, mesh
bytes), plus that frame's uploaded bytes. A once-per-second line summarises the
window; the raw frame is logged at `debug`.

## Acceptance

| Requirement | Measured |
|---|---|
| Overlay or log line per frame | `frame:` at `debug` every frame, and a once-per-second `info` line with the window's means. |
| CPU submit | 1.23–1.26 ms at 180 fps on the RTX 4060; 0.90 ms on the Radeon iGPU. |
| GPU pass | 1.64–1.66 ms (RTX 4060), 8.91–8.97 ms (Radeon iGPU), timestamp queries. |
| Visible chunks/blocks/quads/bytes | 93 of 256 chunks, **13,201 blocks**, **1,032,763 quads**, **43.3 MiB** — exactly 44 B/quad. |
| Upload bytes | 0 KiB in steady state; ~750–800 KiB per frame during the load, matching the 1 MiB M3-03 budget. |
| Values match the scene | the GPU draw test sums blocks/quads/bytes over the visible set and requires equality; `1,032,763 × 44 B = 43.3 MiB` is the packed format exactly. |

## Design

`crates/engine/viewer/src/stats.rs`:

### Per-frame counters

```rust
pub struct FrameStats {
    pub resident: usize, pub visible: usize, pub drawn: usize,
    pub blocks: u64, pub quads: u64, pub mesh_bytes: u64,
    pub upload_meshes: usize, pub upload_bytes: usize,
    pub cpu_submit: Duration, pub gpu_pass: Option<Duration>,
}
```

- **cpu_submit** is measured from the frame's command-encoder creation to
  `Queue::submit` returning, so it includes the upload copies, encoding and the
  submit call.
- **gpu_pass** comes from two in-pass timestamps (below).
- **blocks/quads/bytes** come from the draw loop: `streaming::DrawCost` is
  implemented by `ChunkResources` (`blocks` from the mesher, `quads =
  index_count / 6`, `bytes = vertex + index bytes`), and `draw_visible` sums it
  for the accepted set. `ChunkContent` now reports how many blocks emitted at
  least one visible face; a fully enclosed block contributes nothing.
- **upload bytes** are the meshes installed this frame.

### GPU pass timer

Two timestamp queries bracket the render pass. `CommandEncoder::write_timestamp`
needs `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which the Dozen adapters do not expose;
they do expose `TIMESTAMP_QUERY_INSIDE_PASSES`, so the timestamps go inside the
pass (`RenderPass::write_timestamp`). The pair is resolved into a
`QUERY_RESOLVE | COPY_DST | COPY_SRC` buffer and copied into a `COPY_DST |
MAP_READ` buffer — `MAP_READ` cannot be combined with `QUERY_RESOLVE`. The copy
is mapped asynchronously and read on a later frame; while it is in flight the
previous value is reported, so the timer never blocks. A device without the
features logs one line and reports `gpu pass n/a`.

### Reporting

`StatsWindow` accumulates frames and, once a second, logs:

```
frame: 180 fps | cpu submit 1.23 ms, gpu pass 1.66 ms | 256: 93 visible, 93 drawn | 13201 blocks, 1032763 quads, 43.3 MiB | upload 0 KiB
```

The per-frame `debug` line carries the same fields without the averaging. An
in-viewer overlay was not built: the millestone's acceptance allows either, and
a text line is what the reference machine and the CI logs can actually keep.

## Measured

Release viewer, default camera, `--features chunk128`, ring fully loaded:

| GPU | fps | CPU submit | GPU pass | Visible | Blocks | Quads | Mesh bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| RTX 4060 | 180 | 1.23–1.26 ms | 1.64–1.66 ms | 93 / 256 | 13,201 | 1,032,763 | 43.3 MiB |
| Radeon iGPU | 73–74 | 0.90 ms | 8.91–8.97 ms | 93 / 256 | 13,201 | 1,032,763 | 43.3 MiB |

During the load the line shows the upload in flight: ~750–800 KiB per frame,
CPU submit 20–33 ms (the 1 MiB staging copy on this machine), GPU pass rising
0.3 → 4.6 ms as chunks appear. Steady state drops the upload to 0 KiB and the
CPU submit to ~1.2 ms.

Sanity: 1,032,763 quads × 44 B = 45,441,572 B = 43.34 MiB, exactly the packed
format, and 13,201 blocks over 93 chunks is 142 of the 512 blocks per 128³
chunk.

The usage and feature rules behind this are collected in
[`wgpu-traps.md`](../../notes/wgpu-traps.md).

## Tests

```bash
cargo test -p voxel-viewer stats
cargo test -p voxel-viewer --test draw_loop
cargo test -p voxel-viewer --features chunk128
```

`stats::tests` pin the window's means (CPU, GPU, visible, quads, bytes), the
`gpu pass n/a` path and the reset after a summary. `tests/draw_loop.rs` sums
blocks/quads/bytes over the frustum-visible set and requires the loop's counters
to match, and requires culling to reduce the drawn geometry.

## Limits

- **GPU pass excludes the upload copies and present.** It is the render-pass
  time only; the frame total comes from fps.
- **In-pass timestamps can serialise a pass** on some hardware. Both reference
  adapters accept them; if a future adapter only has encoder timestamps, add the
  `TIMESTAMP_QUERY_INSIDE_ENCODERS` path.
- **The timer reports the previous frame** while a readback is in flight (one
  frame of lag, no stall).
- **No overlay, no history, no CSV.** M3-07 reads these counters for the frame
  breakdown; a UI overlay is later work.
- **Blocks are "blocks with visible faces",** not all non-empty blocks. An
  enclosed block has no geometry and no draw cost.

## Next

M3-07 read these counters for the frame breakdown; M3-08 is the stress scene.
