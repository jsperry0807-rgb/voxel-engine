# M3-00 packed 8 B vertex fallback

**Status:** landed 2026-10-31. Gate C rejected the instanced-record path; this
is the indexed 44 B/quad fallback.

## Format

The logical `Vertex` stays 16 B for goldens and determinism. Upload uses
`PackedVertex` (8 B):

| Bytes | Field |
|---:|---|
| 0–2 | chunk-local `x/y/z` (`u8`) |
| 3 | reserved zero |
| 4–5 | material (`u16`) |
| 6–7 | face 3 bits + four 2-bit AO levels + 5 reserved bits |

Four vertices + six `u16` indices = **44 B/quad**. The viewer shader unpacks the
material/packed word and selects AO with `vertex_index % 4`, matching the
logical vertex order.

## Measured gate

| Edge / profile | Row 6 mesh | Row 1 VRAM |
|---|---:|---:|
| 128³ Minimum | **2,115 KiB PASS** (+284.5) | **2.98 GiB PASS** (+219 MiB vs target, +1.81 GiB vs 4.80) |
| 64³ Default | **536 KiB PASS** (+63.7) | 24.84 GiB FAIL |
| 64³ Minimum | 536 KiB PASS | **6.22 GiB FAIL** (−1.42 GiB to 4.80) |

The 64³ Minimum total is ~5.2 GiB even after the M6-09a v1-pool removal, so the
44 B fallback does **not** revive 64³. The 32 B/quad 4-vertex strip is the only
variant that reaches ~4.3 GiB after the pool removal; it is deferred to a
separate time-boxed spike because the attribute path is unmeasured.

Runs:
`benchmarks/runs/20261031T181211Z-m3-packed-8b-128` and
`benchmarks/runs/20261031T181227Z-m3-packed-8b-64`.

## Code

- `voxel-mesh`: `PackedVertex` encode/decode/from_vertex/to_vertex, exhaustive
  round-trip tests.
- `voxel-viewer`: `chunk_content` emits packed vertices; `ChunkMesh::from_parts`
  is 8 B, `ChunkMesh::new` remains the naive 16 B path; `chunk_render` uses the
  8 B layout and exposes `NAIVE_VERTEX_LAYOUT` for the M1.5 screenshot example;
  `chunk.wgsl` unpacks material/face/AO.
- `voxel-bench`: gate memory default is `PACKED_8B` (44 B/quad); the 76 B
  reference remains informational.
