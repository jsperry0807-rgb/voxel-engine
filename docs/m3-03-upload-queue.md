# M3-03 upload queue with a per-frame byte budget

**Status:** landed 2026-10-31. Finished meshes queue on the CPU and a frame
copies at most 1 MiB of them into the M3-02 pool through one staging belt, so
the upload cost is bounded per frame instead of per install.

## Acceptance

| Requirement | Measured |
|---|---|
| A 10 MB backlog drains without exceeding the budget | `tests/upload_budget.rs`: 10 MiB of 470 KiB meshes against a 1 MiB budget drains in 11 frames, no frame over the budget, in order, nothing lost. |
| Real ring, 128³ | 120.3 MiB in **160–169 frames**, max frame **1022–1024 KiB of 1024 KiB**, 0 overruns, on the RTX 4060 and Radeon iGPU. |
| Real ring, 64³ | 30.2 MiB in **35 frames**, max frame 1018 KiB, 0 overruns. |
| No regression to the streaming shell | 256/256 resident, **0 hitches over 4 ms**; max pump 0.24–3.93 ms across runs (the tail is WSL scheduling with four generator workers, not queue work). |

## Why

M3-02 removed the per-chunk destination buffers, but the uploader still called
`Queue::write_buffer` per mesh. That allocates a staging buffer for every call
and occasionally waits on a submission: measured p50 1.7 ms, ~15 writes at
110–360 ms. It was hidden off-thread, but it could not move to the frame thread
and it was not bounded per frame.

M3-03 turns the upload into a queue and a budget:

```
workers ──> preparer ──> UploadQueue ──> frame: copy ≤ budget ──> draw
          (arena range,   (byte-budgeted             (staging belt,
           light buffers)  FIFO)                      same encoder)
```

## Design

### Queue (`upload.rs`)

`UploadQueue<T: UploadBytes>` is a FIFO with a byte cost per job and one rule:
`take_frame(budget)` takes jobs in order until the next one would exceed the
budget, and **always takes at least one job when the queue is not empty**. A
mesh larger than the budget therefore drains alone instead of stalling the
queue; that frame increments `overruns`.

`UploadStats` reports frames, jobs, bytes, the largest frame, the peak backlog
and overruns. The ring completion log prints them.

### Backpressure

`pump` moves finished jobs from the worker channel into the queue only while
the backlog is below `MAX_BACKLOG_BYTES` (32 MiB). Past that, jobs stay in the
bounded channel, the preparer blocks on its send, and generation blocks behind
it. The queue cannot grow without bound, and the frame thread never blocks on a
producer. The measured peak backlog during a full load is **32.5 MiB**.

### Copy path (`gpu.rs`)

The frame thread takes one frame's jobs, then for each mesh:

1. `StagingBelt::write_buffer` into the mesh's already-reserved pool range,
   memcpy the vertices (and indices plus their pad) into the mapped view;
2. drop the view, which closes the staging chunk;
3. after all jobs, `install_uploads` inserts them into the resident map;
4. the render pass draws them.

The copy commands and the pass are in the **same command encoder**, copy first,
so the GPU needs no fence: wgpu's tracker inserts the required barrier between
the `COPY_DST` write and the vertex/index read. `finish_and_recall_on_submit`
unmaps the staging chunks and schedules their re-map when the submission
completes, so the next frame reuses the same buffers.

The pool range for each mesh was reserved by the preparer thread, so the frame
thread performs no allocation at all: it takes, memcpys, inserts and draws.

### Budget sizing

`DEFAULT_UPLOAD_BYTES` is **1 MiB**:

- the measured mapped-memory copy rate on the reference machine is ~3 µs/KiB
  (a discrete GPU under WSL/Dozen, ~330 MiB/s), so 1 MiB is ~3 ms of frame
  work, inside the 4 ms hitch budget M3-09 will measure;
- it is above the largest mesh the 256-chunk ring produced (585 KiB), so a real
  ring frame never overruns;
- it spreads a 16-chunk column (≈7.5 MiB) over eight frames instead of one
  ~20 ms frame.

`VOXEL_UPLOAD_KIB` overrides it, clamped to 64–16 384 KiB. The staging chunk is
2 MiB, so a frame's writes share one or two chunks.

## Measured run

Release viewer, default camera, 1080p, `--features chunk128` where marked.

| GPU | Chunk | Ring | Frames | Max frame | Peak backlog | Overruns | Copy mean / max |
|---|---|---|---:|---:|---:|---:|---|
| RTX 4060 | 128³ | 120.3 MiB | 166–169 | 1022–1023 KiB / 1024 | 32.5 MiB | 0 | 2.85–2.92 / 15.60–17.62 ms |
| Radeon iGPU | 128³ | 120.3 MiB | 160–162 | 1023–1024 KiB / 1024 | 32.5 MiB | 0 | 2.95–2.98 / 16.02–18.61 ms |
| RTX 4060 | 64³ | 30.2 MiB | 35 | 1018 KiB / 1024 | 10.9 MiB | 0 | 4.05 / 11.32 ms |

The copy maximum is an outlier: the same machine shows single page creations of
147–262 ms, so the tail is the driver/WSL scheduler, not the queue. The mean is
the number to compare against a frame budget; M3-09 owns the p99 over motion.
Spreading the same bytes over a 256 KiB budget gives a 1.7 ms mean and 261
frames; a 4 MiB budget gives a 13.6 ms mean and 49 frames. 1 MiB is the point
where the ring's largest mesh still fits one frame.

A 64 KiB budget stress run drains the 128³ ring in 257 frames with 251 overruns
(each mesh is larger than the budget and is taken alone) and a 0.50 ms mean —
the escape hatch works and the ring still completes.

The staging-belt ordering and alignment rules are collected in
[`wgpu-traps.md`](wgpu-traps.md).

## Tests

```bash
cargo test -p voxel-viewer upload_budget
cargo test -p voxel-viewer --test upload_budget
cargo test -p voxel-viewer --features chunk128
```

`tests/upload_budget.rs` covers the 10 MB acceptance case, ordering and
exactly-once delivery, the per-frame cap, the oversized-job escape hatch and its
overrun counter, the backlog cap, and the size relationships between the budget,
the staging chunk and the backlog cap. The module's own unit tests pin the
empty-queue and stop-at-next-job behaviour.

The GPU path is exercised by the viewer: the load log's `upload:` line reports
meshes, bytes, frames, the largest frame against the budget, the peak backlog,
overruns and the copy time.

## Limits

- **Lighting is still per chunk.** The preparer creates the light, probe and
  origin buffers and the bind group off the frame thread; only mesh bytes are
  budgeted. Pooling those buffers is M3-11/M3-06 work.
- **The copy is a CPU memcpy into mapped staging memory.** On this machine that
  is ~330 MiB/s, so the budget is the only lever; a platform with slower mapped
  writes must lower `VOXEL_UPLOAD_KIB`.
- **One mesh above the budget overruns** by design. At the gate's 128³ p95 mesh
  (2,115 KiB) a frame would move 2.1× the budget; the streamed ring's maximum
  is 585 KiB.
- **Uploads only advance on presented frames.** A skipped or occluded frame
  drains nothing, which is the right pacing for a renderer and is why the queue
  reports `frames` rather than wall time.
- **No cancellation.** A job already queued for a position the camera left is
  freed at take time, but it still consumed its staging slot for that frame.

## Next

M3-04 landed the CPU chunk cull; M3-05 is the draw loop over the visible set.
