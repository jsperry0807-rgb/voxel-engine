# wgpu API traps

Engineering notes from the M3 work: rules that cost time because the API
accepts the call and rejects the *frame*, or because a call silently does
something more expensive than it looks. Each entry names where the fix lives so
the next person does not rediscover it.

These are properties of the wgpu 30 / WebGPU surface we target. The reference
machine's Vulkan driver is **Dozen** under WSL (`Microsoft Direct3D12 (NVIDIA
RTX 4060)` / `(AMD Radeon(TM) Graphics)`), which supports a subset of wgpu's
features; notes say when that matters.

## Buffer usage

### `MAP_READ` may only be combined with `COPY_DST`

```text
Usage flags BufferUsages(COPY_DST | QUERY_RESOLVE) of Buffer ... do not contain
required usage flags BufferUsages(COPY_SRC)
```

Actually the rule (wgpu-core `validate_buffer_usage`): when `MAP_READ` is set,
the usage must be a **subset** of `MAP_READ | COPY_DST`. `QUERY_RESOLVE`,
`COPY_SRC`, `UNIFORM`, `STORAGE` and `VERTEX` are all rejected.

**Consequence:** a query result cannot be resolved straight into a mappable
buffer. Use two buffers:

```text
resolve  : QUERY_RESOLVE | COPY_DST | COPY_SRC
readback : COPY_DST | MAP_READ
```

`encode.resolve_query_set(.., &resolve, 0)`, then
`encode.copy_buffer_to_buffer(&resolve, 0, &readback, 0, 16)`, then map
`readback`.

Pinned in: `crates/engine/viewer/src/stats.rs` (`GpuPassTimer::new`).

### A `QUERY_RESOLVE` destination needs `COPY_SRC` if you copy out of it

Resolving into `QUERY_RESOLVE | COPY_DST` and then copying to the map buffer
fails with:

```text
Usage flags BufferUsages(COPY_DST | QUERY_RESOLVE) of Buffer ... do not contain
required usage flags BufferUsages(COPY_SRC)
```

Add `COPY_SRC` to the resolve buffer. Pinned in the same function.

### `Queue::write_buffer` requires a 4-byte size *and* offset

The copy size must be a multiple of `COPY_BUFFER_ALIGNMENT` (4); a `u16` index
stream with an odd index count is 2 mod 4. Either write an aligned prefix and a
4 B padded tail, or reserve the pad in the destination and fill it:

```rust
let mut view = belt.write_buffer(encoder, &page, offset, padded_size);
view.slice(..data.len()).copy_from_slice(data);
view.slice(data.len()..).fill(0);
```

Pinned in: `crates/engine/viewer/src/mesh_pool.rs` (`write_padded`),
`crates/engine/viewer/src/gpu.rs` (`stage_upload`).

## Submission ordering

### `write_buffer` is pending until the next submit

`Queue::write_buffer` copies into staging immediately but the GPU copy runs
*just before the next `Queue::submit`*. A resource installed after the call can
be drawn in that submission; the write is ordered ahead of it. What is **not**
allowed is drawing the resource in a submission that does not carry the write.

Pinned in: `crates/engine/viewer/src/streaming.rs` + `gpu.rs` (install happens
in the frame whose encoder carries the copy).

### Copies and draws in one encoder need no fence

`copy_buffer_to_buffer` into a pool page followed by a render pass reading the
same page as vertex/index data works: wgpu-core transitions the buffer from
`COPY_DST` to the read use inside the encoder. This is what lets M3-03 upload
and draw in the same submission.

### `StagingBelt`: finish before `finish()`, recall after submit

```text
belt.write_buffer(&mut encoder, ..) -> BufferViewMut   // memcpy here
belt.finish_and_recall_on_submit(&encoder)             // unmaps, schedules remap
queue.submit([encoder.finish()]);
```

`finish_and_recall_on_submit` must be called **before** `encoder.finish()`
(it needs the encoder) and the encoder must then be submitted. Calling
`recall()` after submit also works but only reclaims buffers the GPU has
finished with.

Pinned in: `crates/engine/viewer/src/gpu.rs`.

### `Queue::write_buffer` can stall on the staging belt

Each call allocates a staging buffer and, when the belt is full, waits for an
in-flight submission: measured p50 1.7 ms but ~15 of 256 writes at 110–360 ms
(M3-02). Off the frame thread it is hidden; on the frame thread it is a hitch.
Use one `StagingBelt` in the frame encoder with a per-frame byte budget instead
(M3-03).

## Features and limits

### Adapter features are not granted by default

`adapter.features()` is only what the *adapter* supports. `device.features()`
contains what was requested in `DeviceDescriptor::required_features`. Asking
for an unsupported feature makes `request_device` fail; forgetting to ask leaves
the device without it and the validation error appears at the *command*, e.g.:

```text
Features FeaturesWGPU(TIMESTAMP_QUERY_INSIDE_ENCODERS) are required but not
enabled on the device
```

### Timestamps have two distinct features

| Call | Feature |
|---|---|
| `CommandEncoder::write_timestamp` | `TIMESTAMP_QUERY` + `TIMESTAMP_QUERY_INSIDE_ENCODERS` |
| `RenderPass::write_timestamp` | `TIMESTAMP_QUERY` + `TIMESTAMP_QUERY_INSIDE_PASSES` |

Dozen under WSL exposes `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_PASSES`
but **not** `TIMESTAMP_QUERY_INSIDE_ENCODERS`. Bracket the pass from inside it,
and request both features when the adapter has them.

Pinned in: `crates/engine/viewer/src/stats.rs`, `gpu.rs`.

### `Queue::get_timestamp_period` is `f32` nanoseconds per tick

Multiply `(end - begin)` ticks by the period and divide by `1e9` for seconds.
Not all backends report a meaningful value; treat a non-finite or zero period as
"no timing".

### Pages and buffers must respect the adapter limits

The record path binds one page as a storage buffer, so a page must stay under
`max_storage_buffer_binding_size` (WebGPU default **128 MiB**) and under
`max_buffer_size` (default 256 MiB; Dozen advertises ~1.9 GiB, but never rely on
it). The pool asserts `page_bytes <= max_buffer_size` and caps at 128 MiB.

Pinned in: `crates/engine/viewer/src/mesh_pool.rs`.

## Draw binding

### Vertex and index offsets have their own alignment rules

- `set_vertex_buffer` offset must be 4-byte aligned; the slice size must be a
  multiple of 4.
- `set_index_buffer` offset must be a multiple of the index size; the binding
  size must be a multiple of the index size.
- A `u16` index buffer of odd length therefore needs a padded destination
  range, not a padded source.

Pinned in: `crates/engine/viewer/src/mesh_pool.rs` (4 B range alignment),
`chunk_render.rs` (`draw_ranges`).

## Surface

### Drop the frame before reconfiguring

A `SurfaceTexture` must not be alive across `Surface::configure`. On
`Suboptimal`, drop the frame and reconfigure; on `Lost`, recreate the surface.
Pinned in: `crates/engine/viewer/src/gpu.rs`.

### Map callbacks need a poll

`map_async` completions are delivered by the device poll. A frame loop that
never calls `device.poll` never sees the callback. For a non-blocking read use
`device.poll(PollType::Poll)` once per frame and only read the buffer when the
callback's flag is set; never `poll(Wait)` on the frame thread for telemetry.

Pinned in: `crates/engine/viewer/src/stats.rs` (`GpuPassTimer::poll`).
