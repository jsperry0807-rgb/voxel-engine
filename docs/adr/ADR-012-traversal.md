# ADR-012: Traversal — Workgroup-per-Chunk, Frontier-Synchronous, Warp-per-Node

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** none — closes the traversal row in spec §9 and §11
**Related:** spec §4.3, §5, §8, §9, §10, §11, §12; ADR-001 raster; ADR-006
occlusion; ADR-007 A/B roots; ADR-008 lifecycle; ADR-009 LOD; ADR-011 node
layout; ADR-013 state ownership; M5-01…M5-24

## Context

ADR-001 makes the mesh the draw primitive, but the store still decides which
blocks survive. The SVDAG is traversed on the GPU each frame without readback, and
survivors land in an append buffer that an indirect draw consumes (spec §4.3, §8,
§11). A missed block is a hole; an extra block is only overdraw.

The store fixes the kernel's shape: 8 B nodes, compacted popcount-addressed
children, and one aligned 64 B line per full sibling run (ADR-011). One chunk is an
independent tree with its own root and version (ADR-007), so chunks parallelise
without a shared frontier, and one chunk's frontier fits in groupshared memory.

Only execution mechanics remain: workgroup and warp mapping, level
synchronisation, frontier overflow and dispatch order. Budgets are **1.0 ms** for
broad phase plus traversal, **1.5 ms** for fill plus traversal at 4,096 chunks,
**≥ 3×** draw-work reduction, zero false negatives and **< 0.1%** overflow (spec
§11; [`../performance-targets.md`](../performance-targets.md); roadmap M5).

## Decision

**Traversal is a per-chunk, level-synchronous SVDAG cull: one 128-thread workgroup
per candidate chunk, a double-buffered groupshared frontier of 1,024 nodes, one
warp per frontier node, and four fixed dispatches ending in an indirect draw.**

1. **Four-dispatch order (fixed).** The frame executes, in order:
   - **Dispatch 0 — occlusion fill** (M5-01): camera-seeded, 6-connected block
     flood fill, one bit per block, 128 m horizon, beyond-horizon conservative
     visible (ADR-006).
   - **Dispatch 1 — broad phase and compact** (M5-04, M5-05): one workgroup per
     resident chunk, chunk-root AABB against six frustum planes, lifecycle
     rejection for `EDITING`/`UPLOADING`/`EVICTING`, survivors compacted into the
     candidate list, traversal workgroup count written to indirect args.
   - **Dispatch 2 — traversal** (M5-07…M5-16): indirect over the candidate list,
     four warps per chunk, frustum and block-occlusion tests plus the LOD cutoff,
     survivors appended to the visible-block buffer.
   - **Dispatch 3 — draw** (M5-17): indirect over the append buffer.

   The order is strict on the compute and draw queues. No CPU readback sits between
   dispatches; only GPU-written counters and indirect args move forward. Async
   compute may overlap frames, so slot recycling tracks `inFlightFrame` per root,
   not a global frame number (spec §8).

2. **One workgroup per chunk.** A workgroup is 128 threads = 4 warps and owns one
   candidate chunk. Lane 0 reads `versionIndex` once at workgroup start and
   broadcasts it; the whole traversal uses that root. An empty chunk exits before
   the level loop, and indirect dispatch keeps offscreen or empty candidates free.
   No cross-chunk recursion, shared frontier or work stealing.

3. **Frontier-synchronous.** One level of the chunk's tree at a time. The frontier
   of candidate nodes for the current level lives in groupshared memory; threads
   test, compact survivors into the next frontier, call `workgroupBarrier()`, swap
   and repeat. It is double-buffered and holds **1,024 nodes** (8 B per entry,
   8 KiB per buffer). Ordering is BFS with an approximate front-to-back orbit
   order, not a full sort.

4. **Warp-per-node.** Each warp claims one frontier node at a time and strides
   across the frontier. Lane 0 broadcasts `mask`, `base` and the node origin. Lanes
   0–7 each own one child: lane *c* computes
   `base + popcount(mask & ((1 << c) − 1))`, reads that 8 B node, and the
   eight-child run is one 64 B coalesced fetch. Lanes 8–31 idle or prefetch the
   next run. The warp ballots survivors and lane 0 appends them to the next
   frontier. The origin stays in registers; a child's origin adds
   `2^(CHUNK_BITS − level − 1) × VOXEL_SIZE` per set axis bit, and the AABB is
   derived, never stored.

5. **Cuts: occlusion, frustum, LOD.** At `BLOCK_LEVEL` the traversal reads the
   block's visibility bit once and skips the whole 16³ block when it is clear; it
   does not descend or append. The frustum test carries a parent-inside flag, so a
   node fully inside all six planes lets its children skip the plane tests.
   `lodMaxDepth` (ADR-009) is the deepest level this dispatch may enter: in M5 the
   cutoff is `BLOCK_LEVEL`, and M6-03 supplies the coarser LOD representation at
   shallower depths. The traversal never reads below the cutoff.

6. **Output.** Surviving blocks append to the visible-block buffer with a stable
   layout; the block/instance count is written to the indirect args that dispatch 3
   consumes. Append order is not deterministic, but the emitted block set is. The
   draw is opaque and depth-tested, so order must not affect the image.

7. **Overflow policy.** If the next frontier would exceed 1,024 nodes, the kernel
   does not drop the node and does not silently grow groupshared: it appends that
   node's coverage as **conservative visible blocks** and increments a per-chunk
   overflow counter. This is over-draw, never under-draw, so the
   zero-false-negative rule survives. Overflow is controlled by orbit order, early
   retirement and the per-launch frontier cap (roadmap R14), never by changing the
   capacity. M5 exit requires overflow **< 0.1%**, measured by that counter and the
   M5-20 visualiser.

8. **Lifecycle and versioning.** Dispatch 1 rejects `EDITING`, `UPLOADING` and
   `EVICTING` chunks from the CPU-uploaded lifecycle table; traversal never enters
   them. The version read is the only root selection and happens once. No per-frame
   GPU→CPU readback occurs in any dispatch (spec §12).

## Consequences

- **Positive:** one 64 B fetch per sibling run, with eight children consumed by
  eight lanes and no pointer chase; the frontier stays in groupshared memory, so a
  level costs no VRAM round-trip; per-chunk independence lets indirect dispatch
  make empty chunks free; the append buffer maps directly to the indirect draw;
  the workgroup shape matches the 8 B node and 64 B run width.
- **Negative:** workgroup-per-chunk load imbalance is real — a few chunks or poses
  can dominate (R14); 128 threads plus the frontier pair cap occupancy; the level
  barrier serialises within a chunk; BFS re-reads the frontier each level; append
  order is nondeterministic; overflow and a shallow LOD cutoff buy safety or speed
  with overdraw; per-chunk dispatch overhead grows with chunk count.
- **Explicit trade:** immediate-mode or per-thread DFS and global work-stealing are
  given up for coalesced run fetches, bounded groupshared use and a fixed frontier
  capacity. Conservative overflow is accepted so the culler can never create a
  hole. The LOD cutoff trades detail for a bounded descent; M6 decides the
  representation, not this ADR.
- **Invariants:** 128 threads = 4 warps per workgroup; one chunk per workgroup;
  1,024-node frontier; one barrier per level; lanes 0–7 own the children and lane 0
  broadcasts; popcount child addressing and a 64 B run fetch; `versionIndex` read
  once; block bit tested at `BLOCK_LEVEL`; `lodMaxDepth` cutoff; no traversal of
  `EDITING`/`UPLOADING`/`EVICTING`; no per-frame readback; overflow emits a
  superset; the four dispatches run in the stated order.
- **Verification:** M5-07 smoke; M5-08 frontier parity with a CPU BFS; M5-09 warp
  mapping; M5-10 popcount vectors; M5-11 AABB; M5-12 frustum parity and skip flag;
  M5-13 block skip; M5-14 LOD cutoff; M5-15 append layout; M5-16 forced overflow;
  M5-17 indirect draw; M5-18 500-pose block-set parity; M5-19 false-negative
  detector; M5-21 fill + traversal ≤ 1.5 ms at 4,096 chunks; M5-22 ≥ 3× draw-work
  reduction; M5-23 kill-criterion report.

## Rejected alternatives

- **CPU cull as the production path.** The M3 CPU frustum cull is the stopgap and
  correctness reference, but per-frame CPU work does not scale to 4,096 chunks and
  cannot deliver the ≥ 3× draw-work reduction alone. If the GPU path misses the
  1.5× kill criterion after the M5 timebox, the CPU cull remains and ADR-006
  records why.
- **One warp per chunk, or one workgroup per node.** A warp cannot hold a
  multi-level frontier and loses four-warp latency hiding; a workgroup per node
  explodes dispatch count and drops per-chunk version/root locality.
- **Thread-per-node or per-thread DFS.** Divergence replaces the 8-lane run fetch,
  the level barrier disappears, and survivors cannot be compacted into a shared
  frontier. Recursion also makes the frontier size unbounded.
- **A global frontier in VRAM with work-stealing.** It adds atomics and round-trips
  for data that already fits in groupshared memory, makes per-chunk overflow
  accounting ambiguous, and destroys the version/root isolation.
- **One dispatch per tree level.** Up to `ROOT_LEVEL` extra launches per chunk, with
  the frontier in global memory between them; the barrier and groupshared frontier
  exist to avoid exactly that cost.
- **A frontier larger than 1,024 nodes or dynamic groupshared.** Capacity is a
  portability and occupancy contract, not a tuning dial. Overflow makes a larger
  frontier unnecessary; growing it would hide the imbalance the counter exposes.
- **Dropping overflowing nodes, or falling back to the CPU on overflow.** Dropping
  produces holes; a CPU fallback reintroduces readback and stall. Both violate the
  culler's conservative contract.
- **The M2 ray-test inside this traversal, or per-pixel DAG raymarch.** The ray-test
  is an occlusion escalation behind `occlusion.raytest` (ADR-006), not a second
  traversal; raymarch was rejected with the raster path (ADR-001).

## Status of open questions

None. The kernel shape, frontier capacity, overflow policy and four-dispatch order
are closed. M5 measures parity, overflow and cost; a reduction below 1.5× keeps the
CPU cull and is recorded in ADR-006, not reopened here. `lodMaxDepth` is the only
policy input; its value belongs to ADR-009 and M6, while the cutoff mechanism is
fixed. The chunk edge does not change the kernel: `CHUNK_BITS`, `ROOT_LEVEL` and
`BLOCK_LEVEL` change the level count and frontier contents, never the dispatch
order or workgroup shape.
