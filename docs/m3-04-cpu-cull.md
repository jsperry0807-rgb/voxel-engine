# M3-04 CPU frustum cull of chunk AABBs

**Status:** landed 2026-10-31. The frame thread tests each resident chunk's
world AABB against six planes extracted from the camera matrix and draws only
the chunks that are not entirely outside the view. On the M1-32 reference it is
exact on 100 poses; the stopgap viewer draws **93 of 256** chunks at the default
camera.

## Acceptance

| Requirement | Measured |
|---|---|
| Equals the M1-32 reference set on 100 poses | `cargo test -p voxel-viewer --test cpu_cull`: **100/100 poses, exact set equality**, 52–138 of 256 chunks kept (mean **95.7**). |
| A real reduction, not a keep-everything cull | smallest and largest pose both keep fewer than 256; the live viewer reports **93 visible, 93 drawn, 256 resident**. |
| Conservative, so no holes | a box touching a plane survives in both implementations; the reference parity test is the oracle. |

## Design

`crates/engine/viewer/src/cull.rs`:

### Planes from the matrix

A point is inside the frustum when its clip coordinates satisfy
`-w <= x <= w`, `-w <= y <= w`, `0 <= z <= w` (WebGPU depth `0..=1`). With
`clip = vp · p` and `vp` rows `r0..r3`, each constraint is a plane:

| Plane | Coefficients |
|---|---|
| near | `r2` |
| far | `r3 - r2` |
| left | `r3 + r0` |
| right | `r3 - r0` |
| bottom | `r3 + r1` |
| top | `r3 - r1` |

Each plane is normalized so `PLANE_EPSILON` is a world-space distance. The test
is the centre/extents form: `radius = extents · |normal|`, outside only when
`distance + radius < -PLANE_EPSILON`.

Extracting from the matrix rather than rebuilding planes from
`eye`/`target`/`fov` is deliberate: it is the path the GPU broad phase will
take (M5-04/M5-12), and it keeps the production cull and the M1-32 reference as
two independent implementations of the same contract.

### Chunk boxes

```text
AABB = [pos × CHUNK_DIM × VOXEL_SIZE,
        (pos + 1) × CHUNK_DIM × VOXEL_SIZE]
```

with `CHUNK_EXTENT = CHUNK_DIM × VOXEL_SIZE` (8 m at 128³, 4 m at 64³). This is
the same box the chunk shader builds from `local + chunk_origin`, so the cull
and the drawn geometry agree on world coordinates.

### Wiring

`Gpu::render` builds the frustum once per frame from
`camera.view_projection()`. `Streaming::draw` skips chunks whose AABB is outside
all six planes and returns `DrawStats { visible, drawn }`; the counts are logged
at trace. The pass otherwise draws per chunk exactly as before.

## Measured

Release viewer, default camera, `--features chunk128`:

| GPU | Draw calls | Frame rate | M3-01 (256 draws) |
|---|---:|---:|---:|
| RTX 4060 | **93 of 256** | 182–184 fps (5.4–5.5 ms) | 119 fps (8.4 ms) |
| Radeon iGPU | **93 of 256** | 72–75 fps (13.3–13.9 ms) | 39 fps (25.5 ms) |

The iGPU is the clearer signal: the stopgap's cost was 256 draw calls of raster,
and the cull removes 64% of them. `VOXEL_TRACK=1` produced the frame rates; the
M3-01 column is the frame average recorded in
[`m3-01-streaming.md`](m3-01-streaming.md). M3-10 owns the formal 60 Hz
measurement.

The parity sweep keeps 52–138 chunks depending on pose (mean 95.7), which is
32–54% of the ring — consistent with a 70° vertical field of view looking across
a 16×16 ring.

## Tests

```bash
cargo test -p voxel-viewer --test cpu_cull
cargo test -p voxel-viewer cpu_cull
cargo test -p voxel-viewer --features chunk128
```

`tests/cpu_cull.rs`:

- **100 poses** (position circles the ring at 3–15 m and 0.5–3 m of height, the
  target sweeps at a different rate) compare the production matrix-extracted
  frustum with `voxel_storage::reference::cull::Frustum::from_camera`, a `f64`
  implementation built from camera parameters. Every pose must produce exactly
  the same chunk set.
- the camera's own chunk is never culled; a chunk two edges behind the default
  pose is; a chunk 20,000 chunks beyond the 1,024 m far plane is culled while
  one edge ahead is kept.

`cull::tests` pin the plane classification (inside, outside, straddling, a face
exactly on the plane surviving), plane normalization, chunk AABB placement and
the default-camera cases.

## Limits

- **No hierarchy and no occlusion.** This is the M1-32 oracle's per-chunk,
  six-plane form. M5's GPU broad phase replaces it and must match it; M1-33
  block occlusion is a separate reference and is not applied here.
- **Conservative over-inclusion.** A box near a frustum edge is kept even if the
  true frustum misses it. That is intentional: a culler may draw more, never
  fewer.
- **Chunk granularity only.** A chunk whose AABB intersects the frustum is drawn
  whole, including the blocks behind the camera.
- **`f32` planes.** The epsilon is 1e-5 m; the parity test exercises generic
  poses rather than exact plane contact, where `f32`/`f64` rounding could
  disagree by an epsilon.

## Next

M3-05: the draw loop over visible chunks as its own task — correct image and no
double-draw on the stopped-and-moving camera.
