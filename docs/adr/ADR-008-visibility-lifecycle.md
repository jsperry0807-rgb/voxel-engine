# ADR-008: Visibility × Lifecycle — Two Axes, No Readback

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the implicit single-axis chunk state (`resident` / `clean` / `dirty` /
`visible`) in the current store and shader
**Related:** spec §2, §3, §5, §8, §12; [`../pre-registration.md`](../pre-registration.md);
ADR-007 A/B roots; ADR-013 state ownership; roadmap risk R5

## Context

A chunk answers two different questions. "May I render it?" is per frame and changes
with camera, frustum and occlusion. "May I mutate, upload or evict it?" is per event
and changes with edits, fences and streaming. The first is a GPU fact; the second is
CPU authority. The spec already separates them (spec §5), but the store still carries
one implicit state: a `Chunk` is resident or not, `edited` means dirty, and GPU ranges
are dirty until flushed. Those terms overload "dirty" across DAG, persistence and
upload, and none of them says whether traversal may read the chunk.

The temptation is to collapse the axes: let the CPU learn what the GPU culled and drive
streaming from it, or mark a chunk "not visible" while it is dirty. Both are wrong.
Reading visibility back stalls the frame and couples streaming to render latency;
treating a dirty chunk as undrawable makes every edit pop the chunk out of the frame and
defeats the A/B roots. The GPU tier is never authoritative (spec §2); visibility is the
one designed exception, and it stays a per-frame render fact that never becomes CPU
correctness state (spec §12).

## Decision

**Visibility is GPU-only and per frame. Lifecycle is CPU-only and event-driven. A chunk
has one of each; neither implies the other.**

### 1. Lifecycle (CPU ECS, authoritative)

| State | Traversal eligible? | Meaning |
| --- | --- | --- |
| `READY` | Yes | Root, mesh, light and probes current; CPU and GPU agree |
| `EDITING` | No | CPU mutation in progress |
| `PENDING_CONSOLIDATION` | Yes, on the last `READY` root/mesh | Dirty root and/or stale content; old version still drawable |
| `UPLOADING` | No | New root/mesh staged, fence not signalled |
| `EVICTING` | No | Leaving the resident set |

Transitions are caused only by events: load, edit applied, content complete, batch
staged, fence signalled, flip, unload. No polling and no inference from a `dirty` bit.
`READY` is entered only when the fence has signalled and the root, mesh, light and
probes are all current; the `versionIndex` flip and `READY` are one event, so no frame
sees a new root with a stale mesh (M4-06, M4-10). `PENDING_CONSOLIDATION` keeps drawing
the last `READY` version, which is what keeps edits from stalling rendering. `EDITING`,
`UPLOADING` and `EVICTING` are excluded by the lifecycle table alone. Every transition
is asserted, not inferred (M4-03).

### 2. Visibility (GPU, per frame, never read back)

| State | Meaning |
| --- | --- |
| `FRUSTUM_VISIBLE` | Passed the frustum; survives until occlusion resolves |
| `OCCLUDED` | Passed the frustum, rejected by block-level flood fill |
| `CULLED` | Rejected by the frustum |

Visibility is rebuilt every frame, lives only in GPU buffers (occlusion bits, candidate
list, frontiers, append buffer, indirect args, per-chunk visibility bit), and is never
persisted, uploaded to the CPU, or written to a save. It is not an ECS field. A chunk
can be `PENDING_CONSOLIDATION` and `FRUSTUM_VISIBLE` at the same time; a `READY` chunk
can be `CULLED`. The two axes are orthogonal.

### 3. What the CPU may learn

The CPU may receive only these events:

- an edit was applied (chunk, block/voxel range, material, dirty runs);
- content completed (light, mesh/AO, probes) for a chunk;
- a consolidation batch was staged and committed (chunk set, root version, epoch);
- the fence signalled (epoch);
- a chunk was loaded or unloaded (position);
- a camera ring was crossed (new ring, prefetch/evict sets);
- a probe LOD changed (chunk, new LOD);
- a lifecycle transition it initiated or completed.

### 4. What the CPU may not learn

The CPU may not learn per-frame visibility, whether a chunk was drawn, visible/culled/
occluded counts, candidate or append counts, frontier occupancy or overflow, indirect
arguments, occlusion bits, or the result of a `versionIndex` read. It may not use any
GPU-produced counter to drive streaming, eviction, LOD, prioritisation or correctness.
Streaming is camera-predictive, not visibility-reactive (spec §12).

Two readbacks are allowed, neither on the critical path: a diagnostics stats readback
every **≥ 30 frames**, off the frame path and never consumed by correctness; and a
test-only readback of a sample region in the torn-frame detector (M4-14) or a golden
comparison. A future feature that wants visibility on the CPU needs a new ADR with a
measured budget; it does not enter through a "small" readback.

### 5. State mapping (normative)

| Old / overloaded term | New owner | New state |
| --- | --- | --- |
| absent / unloaded | ECS | no entity |
| `loading`, `generating`, `meshing`, `lighting`, `probing` | content timeline | not lifecycle states; the chunk becomes `READY` only when content is current |
| resident / clean | ECS | `READY` |
| `dirty` = DAG needs folding | ECS | `PENDING_CONSOLIDATION` |
| `dirty` = not persisted | journal/disk | not a lifecycle state; orthogonal `unsaved` flag |
| `dirty` = GPU range pending | upload tracker | not a lifecycle state; coalesced ranges |
| editing / mutating | ECS | `EDITING` |
| consolidating / uploading / fence pending | ECS | `UPLOADING` |
| unloading / dropping | ECS | `EVICTING` |
| `visible` boolean | GPU | `FRUSTUM_VISIBLE` / `OCCLUDED` / `CULLED` |
| `mesh_dirty`, `light_dirty`, `probe_dirty` | content timeline | not lifecycle states; content versions |

The mapping is exhaustive for the current terms. Any new state must name its owner, its
traversal eligibility and the event that enters and leaves it.

## Consequences

- **Positive:** rendering never waits for CPU state and streaming never waits for the
  GPU. Edits keep drawing the old root. One fact has one owner: lifecycle on the CPU,
  visibility on the GPU. The lifecycle table is a small per-frame CPU→GPU upload; no
  GPU→CPU path exists.
- **Negative:** the CPU cannot optimise streaming from actual visibility, so it will
  prefetch unseen chunks and retain some off-screen ones. Visibility bugs are GPU-side
  and need a capture or the diagnostics readback to observe. Every lifecycle transition
  needs an assertion and an event, not a flag check.
- **Explicit trade:** predictive streaming over visibility-reactive streaming, and
  frame-time predictability over streaming optimality. Correctness never depends on GPU
  visibility; if that ever changes, the design has changed.
- **Invariants:** GPU is never authoritative except for per-frame visibility; visibility
  is never in the ECS, save, disk format or streaming decision; no per-frame GPU→CPU
  readback; `READY` implies root, mesh, light and probes current; only `READY` and
  `PENDING_CONSOLIDATION` are traversable; flip only after the fence; `versionIndex`
  read once per workgroup; transitions asserted, not inferred.
- **Verification:** lifecycle assertions (M4-03), flip-after-fence (M4-06),
  version-read-once (M4-11), torn-frame detector (M4-14), broad-phase lifecycle parity
  (M5-04), camera-predictive streaming (M6-05), and a profile check that no readback
  appears on the frame path (M7-03).

## Rejected alternatives

- **One combined state (`VISIBLE_DIRTY`, `READY_VISIBLE`).** Visibility changes per frame
  and lifecycle per event; combining them produces a state explosion, and the only way
  to keep it current is a readback or a render stall.
- **CPU-known visibility / visibility-driven streaming.** Requires per-frame readback,
  couples streaming to frame latency, and makes correctness depend on GPU culling.
  Forbidden by spec §12 and roadmap R5.
- **A boolean `visible`.** Loses the distinction between frustum rejection and
  occlusion, so the two culls cannot be tuned or measured separately and the ray-test
  escalation has no state to write.
- **Treat `PENDING_CONSOLIDATION` as not drawable.** Every edit would pop its chunk out
  of the frame and defeat the A/B root pair; this is the failure the two-axis split
  exists to prevent.
- **GPU-authoritative lifecycle.** The GPU cannot own edit, disk or streaming state, and
  any GPU state must be reconstructible from CPU + disk (spec §2).
- **Polling GPU counters for correctness.** That is a readback with extra steps;
  counters are diagnostics only.
- **Persisting visibility.** It is meaningless on the next frame and would put a render
  fact into the save format.

## Status of open questions

None. The axes, owners, states and readback rule are fixed. ADR-013 turns the allowed
event list into the concrete ECS event ABI; it may add events, but it cannot merge the
axes or introduce a per-frame readback.
