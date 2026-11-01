# M3-05 draw loop over visible chunks

**Status:** landed 2026-10-31. The frame's draw loop is one pass over the
resident map, a conservative M3-04 filter, and one indexed draw per accepted
chunk with a mesh. A culled frame is byte-identical to a draw-everything frame
on real geometry, and no chunk is drawn twice.

## Acceptance

| Requirement | Measured |
|---|---|
| Correct image | `tests/draw_loop.rs` renders a 5×5 chunk patch twice — once with the M3-04 frustum, once with every chunk accepted — **byte-identical** at 512². |
| No double-draw | the loop iterates a map of chunk positions once; both GPU passes record every visited position and assert **all unique** (25 visits for 25 chunks, 8 for the culled pose). |
| Run | the viewer logs `draw: 93 visible, 93 drawn, 256 resident` on the RTX 4060 and Radeon iGPU with no validation errors. |

The culled pose keeps **8 of 25** chunks (32%); the acceptance only requires
that it keeps some but not all, so the comparison cannot pass by drawing the
same set both times.

## Design

`streaming::draw_visible` is the whole loop, generic over the per-chunk payload:

```rust
pub fn draw_visible<'a, T: 'a>(
    chunks: impl IntoIterator<Item = (ChunkPos, &'a T)>,
    keep: impl Fn(ChunkPos) -> bool,
    draw: impl FnMut(ChunkPos, &'a T) -> bool,
) -> DrawStats
```

- One pass over the input. Chunks rejected by `keep` are never visited by the
  draw callback, so an invisible chunk costs one plane test and nothing else.
- The callback returns whether it actually submitted a draw, so `DrawStats`
  separates **visible** chunks (inside the frustum, including empty ones) from
  **drawn** meshes. M3-06's overlay uses both.
- Position-keyed input makes a double draw structurally impossible; the tests
  pin it anyway, because a future visible-list cache could reintroduce one.

`Streaming::draw` is a thin adapter: it passes `self.chunks`, the M3-04 frustum
and a callback that binds the pooled mesh and the chunk's light group.
`Gpu::render` builds the frustum once per frame and logs the counts at trace.

Order stays the resident map's iteration order. The image does not depend on it:
every draw is opaque geometry into the same depth-tested pass, so swapping two
chunks' draws cannot change a pixel. Sorting would only add a per-frame
allocation.

## Tests

```bash
cargo test -p voxel-viewer --test draw_loop
cargo test -p voxel-viewer draw_loop
cargo test -p voxel-viewer --features chunk128
```

`tests/draw_loop.rs`:

- **pure:** accepted chunks are visited once each, in order, with the right
  counts; a chunk whose callback declines counts as visible but not drawn; a
  rejected set never calls the callback.
- **GPU:** 25 chunks of production greedy mesh and light through the M3-02
  pool, drawn with the real `ChunkPipeline`; the culled and draw-everything
  frames must match exactly, both visit lists must be duplicate-free, and the
  validation error scope must stay empty. Skips without an adapter and honours
  `VOXEL_REQUIRE_GPU=1`.

## Measured

| GPU | Visible | Drawn | Resident |
|---|---:|---:|---:|
| RTX 4060 | 93 | 93 | 256 |
| Radeon iGPU | 93 | 93 | 256 |

## Limits

- **One draw per visible chunk.** No indirect draw, no instancing, no
  visible-block append; M5 replaces the whole loop with indirect draws from the
  GPU traversal.
- **No occlusion.** The M1-33 reference is not applied, so a chunk behind a
  mountain is still drawn if its AABB intersects the frustum.
- **The counts are per frame, not cumulative.** M3-06 adds the overlay and the
  per-second line.

## Next

M3-06 added the counters (`stats::FrameStats`) and the once-per-second line;
M3-07 builds the frame breakdown from them.
