# ADR-013: State Ownership — GPU Per-Frame, Event-Driven CPU

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** none; ADR-008 owns the axis split, this record owns the ECS event ABI
**Related:** spec §2, §5, §8, §12; ADR-007 A/B roots; ADR-008 lifecycle;
ADR-012 traversal; [`../performance-targets.md`](../performance-targets.md);
M0-21, M3-04, M4-13, M4-14

## Context

Chunk state has two axes that change at different rates and for different reasons.
Visibility is a render fact recomputed every frame; lifecycle is a content fact moved
by edits, commits and streaming. The store also keeps frames in flight and flips COW
roots asynchronously, so neither axis can be a simple mirror of the other.

Two ownership mistakes are available. The CPU can mirror per-frame GPU visibility,
by reading back counters or keeping a shadow copy: that stalls the frame on GPU
completion and buys a fact that is already one or more frames stale. Or the GPU can
own lifecycle and residency: it would then allocate, evict and compact behind the
CPU's edit journal, and a COW root could be read after the CPU had reclaimed it.
Both failures disappear once one writer per axis is named.

The spec already fixes the split (§5, §12): visibility lives only on the GPU,
lifecycle only in the CPU ECS, and streaming is camera-predictive. This ADR records
the boundary, the closed event set that moves the CPU side, and the readback rules
that keep the two from becoming one.

## Decision

**Two owners, no shared mutable state. The GPU owns per-frame render state and never
reports it. The CPU owns authoritative chunk lifecycle and moves only on an event.**

1. **GPU per-frame state.** Block occlusion bits, the compacted chunk candidate
   list, per-chunk frontiers, the visible-block append buffer and counter, indirect
   argument buffers, the per-chunk visibility bit and the `versionIndex` table. This
   state is written and consumed inside the frame, is not authoritative, and must be
   reconstructible from CPU + disk — except visibility, which is a per-frame render
   fact and never a correctness input. The `versionIndex` table is GPU-resident but
   CPU-written on the flip event; the GPU reads each chunk's index once at workgroup
   start and never writes it.

2. **CPU authoritative state.** One sparse-set ECS of chunk entities with `pos`,
   `lifecycle`, `root_version`, `mesh_version`, `last_touched_frame` and streaming
   priority. The lifecycle set stays dense, so commit and streaming loops touch live
   chunks only. The CPU alone decides residency, ring/LOD and probe placement, edit
   and commit order, and lifecycle. It uploads a lifecycle table to the GPU once per
   frame; the broad phase reads that one-way table and traversal honours it. No GPU
   pass allocates, evicts, compacts, writes lifecycle or reorders edits.

3. **The event set is closed.** Only ADR-008's events wake the ECS:
   - **edit applied** — publishes a dirty root and/or stale content, bumps
     `root_version` and `last_touched_frame`; the chunk is `EDITING` while mutating
     and `PENDING_CONSOLIDATION` once the root is dirty, still traversable on its
     old drawable version.
   - **content completed** — light, mesh/AO or probes for a version are current, and
     `mesh_version` advances. With the root current too, the chunk becomes `READY`.
   - **consolidation committed** — stages the dirty root into the inactive A/B slot
     and moves the chunk to `UPLOADING`, where it is no longer traversable.
   - **fence signalled** — flips `versionIndex` and updates `root_version`; `READY`
     follows only once content is current too. This is a synchronisation event, not
     a readback.
   - **chunk loaded / unloaded** — inserts the entity on load (content work follows)
     or enters `EVICTING` and then removes it. Residency changes only here.
   - **camera ring crossed** — updates streaming priority and may enqueue prefetch
     or eviction candidates; it never changes visibility.
   - **probe LOD change** — schedules content work at the new probe resolution.
   - **lifecycle transition initiated / completed** — a notification and assertion
     point only; it cannot invent a state that no earlier event authorised.

   Frame ticks, render passes, indirect draw counts and visibility results are not
   events. Between events the ECS is unchanged. Systems may drain event queues at a
   frame boundary, but a frame alone never causes a lifecycle transition.

4. **No per-frame readback.** The CPU never learns per-frame visibility, candidate
   lists, frontier occupancy or draw counts, and no render or streaming decision
   waits on GPU work. The frame path never blocks on a fence; an edit
   acknowledgement may wait for its batch fence, and that wait is charged to the
   8 ms edit budget. Streaming priority derives from camera position, velocity and
   ring distance only. `EDITING`, `UPLOADING` and `EVICTING` chunks are excluded from
   traversal through the uploaded lifecycle table; `PENDING_CONSOLIDATION` stays
   traversable on its previous root, which is what keeps an edit from stalling
   rendering.

5. **Diagnostics are not state.** GPU-derived stats are read back no more often than
   every 30 frames (500 ms at 60 Hz), off the frame path, and never feed correctness.
   CPU-side counters may be shown every frame in development. A fence signal is
   control flow, not diagnostics. The M4-14 torn-frame harness is the only per-frame
   GPU readback: it is test-only, cfg-gated and never shipped. The M3-04 CPU frustum
   stopgap uses camera data, not GPU visibility, and does not reopen this decision.

## Consequences

- **Positive:** no pipeline stall and no frame-rate-dependent state. Frames stay in
  flight behind the fence, lifecycle correctness is independent of GPU timing, and
  the dense ECS keeps O(live chunks) loops. Streaming is deterministic from camera
  motion and reproducible on replay, and all GPU state except visibility can be
  rebuilt after device loss.
- **Negative:** the CPU cannot stream by visibility, cull gameplay with render
  results, or size frame N+1 from frame N's draw count. Residency is conservative: a
  chunk behind the camera can remain resident until ring distance and hysteresis
  evict it, and the CPU learns of a completed flip at least one batch later. The
  one-way lifecycle upload costs a small buffer per frame, and the event queue must
  be lossless and ordered or the ECS diverges.
- **Explicit trade:** camera-predictive streaming over visibility-reactive
  streaming, and causal correctness over opportunistic per-frame CPU feedback.
  Telemetry is up to 30 frames stale by design; render truth is not gameplay truth.
- **Invariants:** no readback on the frame path; no visibility in the ECS; no
  frame-path wait on GPU completion (only the budgeted edit acknowledgement may wait
  on its batch fence); lifecycle changes only through the events above;
  `PENDING_CONSOLIDATION` remains traversable; GPU-derived diagnostics at most once
  per 30 frames and never authoritative; no GPU writes to lifecycle; `versionIndex`
  is read once per workgroup and flipped only after the fence signals.
- **Verification:** M1 edit p95 ≤ 8 ms with no fence wait on the frame path; M3-04
  CPU cull matches the M1-32 reference set; M4-13 soak runs 1 000 edits/s for 10 000
  frames with a CPU-matching content hash; M4-14 catches an injected early flip;
  M6-05/M6-06 exercise velocity prefetch and hysteresis eviction; CI greps for
  readback calls outside diagnostics and test code.

## Rejected alternatives

- **CPU shadow of per-frame visibility.** It needs a readback or GPU writes into CPU
  memory, stalls at least a frame, duplicates state, and any divergence produces
  wrong eviction. The CPU cannot act faster than the data arrives.
- **Visibility-reactive streaming.** By the time visibility or a 30-frame stat
  returns, the camera has moved. It is strictly worse than camera prediction and
  costs a sync point. M6 may tune prediction weights; it does not add feedback.
- **GPU-authoritative lifecycle or residency.** The GPU would have to allocate,
  evict, compact and order edits, race COW roots against in-flight frames, and
  violate the rule that GPU state is reconstructible from CPU + disk.
- **A per-frame ECS tick or polled fence.** Sampling causal events on a clock burns
  CPU and makes state depend on frame rate. Fence callbacks are used instead.
- **Reading indirect draw counts to size the next frame.** It couples frame N+1 to
  GPU completion of frame N, destroys overlap, and is unnecessary when work is
  bounded by a budget rather than by observed output.
- **Per-frame stats readback.** One sync point per frame plus an invitation to use
  stale data for correctness. The 30-frame cadence is enough for telemetry.
- **One shared CPU/GPU state buffer with both writers.** No ownership, torn reads,
  and no single-writer guarantee for the flip protocol.

## Status of open questions

None. The event set is closed; adding an event or a readback site requires amending
this ADR. M6 may tune streaming priority and eviction hysteresis — policy values
inside the CPU-owned model, not ownership changes.
