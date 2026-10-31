# M3-01 streaming shell

**Status:** landed 2026-10-31. The viewer keeps a 256-chunk ring resident and
loads it without a frame-thread streaming hitch over 4 ms.

## Acceptance

| Requirement | Measured |
|---|---|
| 256 chunks load | **256/256 loaded, 0 pending** on the RTX 4060 and Radeon iGPU |
| No streaming hitch > 4 ms | **max pump 0.09 ms, 0 hitches** on both GPUs |

The measured quantity is the **frame-thread pump**: ring recenter plus
installing finished resources. It excludes draw/submit/present and the
off-thread uploader. A whole-frame reading of the gate is M3-09/M3-10; the
load-time 4060 frame average is 10–15 ms against an 8.4 ms steady state (draw
and worker CPU contention), not a pump hitch. The ring, its log line and the exact
commands are below.

## Design

### Ring

`streaming::ring_chunks(center)` returns a 16×16 ring of `ChunkPos` at `y = 0`,
nearest first. The ring is centered on the camera's horizontal chunk position
(`center_for(camera.uniform().position)`), so moving the camera evicts the
trailing column and queues the leading one. `ring_delta` is the pure load/evict
planner:

- **load:** desired, not resident, not already requested;
- **evict:** resident or requested, outside the ring.

The ring is horizontal by design. Vertical streaming is later work; the M3-01
shell only has to prove the resident set and the frame budget.

### Terrain

`stream_volume(chunk)` is deterministic, world-coherent value noise plus a
shallow basin around the default camera's start column. Neighbouring chunks
share the same surface function, so the greedy mesher finds long runs.
`fill_stream_volume` lets a worker reuse one volume buffer instead of
allocating 4 MiB per chunk.

`stream_apron(chunk)` samples the same height field into the six 1-voxel face
planes, so border faces are culled against the real neighbour and border AO
does not treat the chunk edge as air. Corner AO samples still read air, the
documented one-voxel-apron limitation. The measured 256-chunk ring is
**3,679–14,920 quads/chunk** (mean 11,198), inside the chunk-wide `u16` index
cap of 16,384 quads; the maximum leaves ~9% headroom, which is why per-block
meshlets remain M3 work.

### Three stages

```
frame thread ──pump──> install finished resources (map insert)
      ▲
      │ bounded channel
uploader thread ──> create buffers/bind groups, write mapped data
      ▲
      │ bounded channel
generator/mesher workers ──> stream_volume + chunk_content::build
```

The **generator/mesher pool** runs `chunk_content::build_with_apron` (greedy
mesh + AO, light full pass, probe bake) off the frame thread. The default pool
is `min(available_parallelism(), 4)`; `VOXEL_STREAM_WORKERS` overrides it up to
8 for experiments.

The **uploader thread** creates the per-chunk vertex/index/light/probe/origin
buffers and the data bind group, then sends the finished `ChunkResources` to the
frame thread. It is deliberately not on the frame thread. The 256-chunk trace
measures one upload at **6.2 ms mean and 20.9 ms max**; on the frame thread that
is a guaranteed hitch, so no frame-thread upload-budget scheme could hold 4 ms.
Moving the allocation stage off-thread removes the hitch; M3-02's arena is the
place to make the uploads cheap enough to return to the frame thread.

The **frame thread** calls `Streaming::pump(center, PUMP_BUDGET)` before
acquiring the surface texture. `pump` re-centers the ring if needed and installs
finished resources until the 4 ms budget or the channel is empty. Installing a
resource is a `HashMap` insert, so the pump is normally microseconds. The
budget check cannot interrupt a single install or a recenter; both are measured
in `max_pump`/`hitches`.

### Origin

Every chunk carries a 16 B `chunk_origin` uniform (group 2, binding 2), in world
voxels. The vertex shader computes

```wgsl
world = (local + chunk_origin.xyz) * 0.0625
```

so one mesh pipeline draws all chunks. `chunk_origin(pos)` is
`pos * CHUNK_DIM`, and `tests/validation.rs` renders the same chunk at origin 0
and origin 64 with the camera shifted by the matching 4 m and requires the
images to match within 2 LSB.

## Measured run

Release viewer, `--features chunk128`, 1920×1080, default camera:

| GPU | 256 loaded | Max pump | Hitches > 4 ms | Steady frame |
|---|---:|---:|---:|---:|
| RTX 4060 (Vulkan/Dozen) | ~5 s | **0.09 ms** | **0** | 8.4 ms (5.6–8.4 across runs) |
| Radeon iGPU (Vulkan/Dozen) | ~5 s | **0.09 ms** | **0** | 25.5 ms |

The iGPU frame time is the 256-draw-call raster cost, not streaming work;
M3-10 owns 60 Hz. The load log is:

```
stream: 256 loaded, 0 pending; max pump 0.09 ms, 0 hitches over 4 ms
```

Commands:

```bash
cargo build --release -p voxel-viewer --features chunk128
WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
  RUST_LOG=voxel_viewer=debug timeout 25 ./target/release/voxel-viewer

# Radeon iGPU
WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
  VK_LOADER_DEVICE_SELECT=1002:164e \
  RUST_LOG=voxel_viewer=debug timeout 30 ./target/release/voxel-viewer
```

`RUST_LOG=voxel_viewer=trace` also prints per-chunk upload and buffer-stage
timings.

## Tests

```bash
cargo test -p voxel-viewer --test streaming
cargo test -p voxel-viewer --test streaming --features chunk128
cargo test -p voxel-viewer --test validation
WGPU_BACKEND=vulkan WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
  VOXEL_REQUIRE_GPU=1 cargo test -p voxel-viewer --test validation
```

`tests/streaming.rs` pins the 16×16 ring, nearest-first order, load/evict delta,
deduplication of in-flight requests, generator determinism, terrain height
continuity across negative and positive chunk edges, the analytic apron against
the neighbour border and its border-face culling, and the `u16` index bound on
full-relief positions. `tests/validation.rs` covers positive and negative
origins and the production draw on both GPUs.

## Limits

- Horizontal ring only; vertical streaming is later.
- Per-chunk buffers until M3-02. The uploader hides their allocation cost from
  the frame thread, but the driver stalls still exist off-thread.
- The worker pool does not cancel queued or in-flight work when the camera
  moves; a recenter drops stale results at install time, and a position that
  leaves and re-enters the ring can be generated twice. Cancellation is
  M2.5-24 / later streaming work.
- The static acceptance run never moves the camera, so runtime eviction is
  covered by the pure planner tests here, not by the load log. M3-09's camera
  motion is the runtime eviction/hitch test.
- Whole-frame hitches from draw/submit/worker contention are not the pump
  metric; M3-09/M3-10 own them.

## Next

M3-02 (mesh buffer pool / arena) on the packed 8 B stride and the `MeshFormat`
descriptor seam. Then M3-03 adds the upload-side per-frame byte budget.
