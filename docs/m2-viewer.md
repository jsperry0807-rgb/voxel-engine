# M2 viewer wiring (M2-27)

**Status:** complete. The runtime viewer draws the production M2 mesh, shades
with production AO + block light, and uploads the near probe volume. The M1.5
naive mesher remains as the reference, the tests and the historical screenshot
example.

## Content path

One demo chunk (the M1.5 terrain patch) is built in `chunk_content`:

1. **Mesh** — every 16³ block is meshed by `greedy_block` (production AO in the
   merge key) and `mesh_block`, concatenated into one chunk-wide stream of
   ADR-010 16 B vertices and `u16` indices.
2. **Light** — `voxel_mesh::light::full_pass` over the chunk's block cells
   (partial blocks get opacity 8, fully solid blocks 15, sky from the column
   above); the cells are serialized as 16 B records (four `u16` per channel).
3. **Probes** — `voxel_mesh::probe::bake` on the near grid: 12 B SH4 records
   plus the packed validity mask.

`chunk_lighting` uploads light and probes as read-only storage buffers in bind
group 2 and creates the bind group; `chunk_render` binds it. The chunk shader:

- selects the vertex's AO corner with `vertex_index % 4` (each quad's four
  vertices are consecutive in the index stream) and interpolates it;
- reads `max(sky, block)` from the fragment's light cell and multiplies the face
  shade (a 0.25 ambient floor; sky-lit surfaces stay at exactly 1.0);
- declares the probe buffer as a placeholder for the later indirect pass. It is
  uploaded and bound but not sampled yet.

The atlas, material slots and face-local UV mapping are unchanged; the demo
volume already stores atlas ids, so no `resolve_materials` step is needed (a
real chunk stores block ids and will resolve them in M3).

## Merge ratio and parity

The demo chunk has **2,208 production quads covering the same 8,226 exposed
faces as the naive mesh** — 73.2% fewer quads, 167,808 B of mesh (plus 1,024 B
light and 776 B probes). `tests/chunk_content.rs` asserts that the covered face
count equals the naive quad count, so geometry and material coverage are
identical; the merge removes only interior seams. AO and block light are the
intended M2 shading difference from M1.5.

## Frame rate

`cargo run -p voxel-viewer` with `RUST_LOG=voxel_viewer=debug` on the
development host (llvmpipe software Vulkan, 1920×1080):

```text
uploaded M2 chunk: 2208 quads cover 8226 faces (73.2% fewer than naive 8226),
                   167808 B mesh, 1024 B light, 776 B probes
frame timing: 165.7 fps (6.03 ms/frame)
frame timing: 166-172 fps (5.8-6.0 ms/frame)
```

That is comfortably above 60 Hz even on the software rasterizer; the
reference-GPU run remains the manual acceptance step.

## Verification

```sh
cargo test -p voxel-viewer
cargo test -p voxel-viewer --features chunk128
cargo run -p voxel-viewer
```

The offscreen validation test compiles the substituted shader with naga and
draws the production pipeline inside a wgpu validation error scope; the content
tests pin the face coverage, the index pattern the AO selection relies on, the
light word layout and determinism.

## Limits (M3)

- One chunk; streaming, eviction and budgeted remeshing are M3.
- The chunk-wide `u16` index stream caps a mesh at 16,384 quads. The 48×48 demo
  patch fits at both edges; a fully populated 128³ chunk needs per-block
  meshlets, which M3/M2-30 will schedule.
- Probes are uploaded but not sampled; light is sampled at the fragment's own
  cell (one step along the face normal is later polish).
