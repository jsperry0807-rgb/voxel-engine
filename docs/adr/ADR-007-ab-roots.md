# ADR-007: A/B Roots Over the Shared Pool

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the single-root pointer and per-chunk-interner position in
[`../reference/adr-three-tier-store-v2.md`](../reference/adr-three-tier-store-v2.md)
**Related:** spec §2, §3, §8, §10.1–10.3, §11, §12;
[`../pre-registration.md`](../pre-registration.md); ADR-011 node layout;
ADR-012 traversal; M1-04, M1-12, M4-01…M4-17

## Context

COW consolidation publishes a new root while the GPU may still be traversing the old
one. Overwriting a single root slot in place would either tear a frame or force every
edit to wait on a fence, which breaks the 8 ms edit budget. The spec therefore gives
each chunk **two roots (A/B)** and flips a `versionIndex` only after the batch fence
signals (spec §3).

The pair is per chunk, but the node pool is not: both roots index one shared,
hash-consed pool of runs, shared across chunks (spec §2, §10.3). That is what keeps
A/B affordable. A chunk DAG is **60–90 KiB at 64³ and 0.5–0.7 MiB at 128³**, so a
second full copy would be a per-chunk doubling. COW duplicates only the path nodes an
edit changed: one run per level, up to 64 B each — **384 B at 64³, 448 B at 128³**.
Steady-state pair overhead is **~2 KiB at 64³ and ~4 KiB at 128³**, at most 5% of the
chunk DAG. The pre-registered **16 B/node** figure (8 B × 2 roots) is a conservative
hard cap, not the expected cost.

## Decision

**Each chunk has two root slots (A/B) and a `uint32 versionIndex`; both roots index one
shared, hash-consed run pool. Writes go to the inactive root, the active root stays
readable, and a slot is recycled only after its own in-flight frame is two frames old.**

1. **Slot and index.** The chunk-table slot holds `rootA`, `rootB`, `versionIndex` and
   `block_table` (M4-01). `versionIndex` is 0 for A, 1 for B, and exactly one root is
   active. A root slot is a run index into the pool, never a second copy of the DAG.
2. **Shared pool, run interning.** Runs are interned by `(level, mask, ordered child
   ids)` with a collision-safe equality check, appended to level bands, with full
   `0xFF` runs 64 B aligned (M1-03/M1-04, ADR-011). Because the pool is shared, both
   roots share every untouched run with each other and with other chunks. Live runs are
   refcounted; the old root is retired through the epoch list, and unload releases the
   chunk's references. This sharing, not the number of slots, is the memory argument.
3. **Overhead is a path, not a chunk.** One edit appends one run per level on the path
   to the new root: `ROOT_LEVEL` runs, ≤ 64 B each. The inactive root therefore adds
   **≤ 384 B at 64³ and ≤ 448 B at 128³** per edit, and the live pair costs ~2 KiB /
   ~4 KiB against a 60–90 KiB / 0.5–0.7 MiB DAG. M4-17 records the ratio and requires
   **≤ 5% of the chunk DAG**.
4. **Write inactive, flip after the fence.** At a frame boundary, all dirty chunks are
   staged into their inactive roots in one pass (M4-04). One GPU fence covers the
   batch; no `versionIndex` flips until it signals (M4-05/M4-06). Traversal reads
   `versionIndex` **once per workgroup**, broadcasts it, and uses that root for the
   whole dispatch (M4-11, spec §11). It never re-reads mid-traversal.
5. **Recycle rule.** Track `inFlightFrame` **per root**, not with a global counter
   (M4-07). A vacated root may be overwritten only when
   `currentFrame − inFlightFrame >= 2` (M4-08). With two roots that bounds flips to
   one every two frames. If measured completion lag is three frames, **add a third
   root**; do not weaken the rule. Per-root tracking delays recycle of exactly the
   root a lagging frame read, not every chunk in the world. Compaction is forbidden
   while either root or any in-flight frame references the chunk (M1-12, M4-12).
6. **One abstraction, two users.** `VersionedResource { active, inactive, inFlight[2],
   fence }` is shared by roots and mesh buffers (M4-03/M4-10), so a root and its mesh
   flip in the same batch and no frame sees a mismatched pair.

## Consequences

- **Positive:** no torn root and no mid-frame wait; a chunk stays traversable on its
  old root while dirty, so edits do not stall rendering (spec §5). The pair is cheap
  because the pool, not the slot count, owns the nodes. Append-only bands keep both
  roots pointer-stable.
- **Negative:** the shared pool requires refcount and epoch discipline; unload cannot
  simply drop a private arena. Two live versions bound the edit history, and a lagging
  frame can starve a chunk until the third-root contingency is exercised. Compaction
  cannot touch a resident chunk, and the pair still costs ~2–4 KiB/chunk.
- **Explicit trade:** duplicate path runs instead of a second chunk copy; accept a
  two-frame flip cadence instead of per-frame flips; share the pool across chunks and
  pay reference accounting instead of keeping per-chunk interners and simple unload.
- **Invariants:** exactly one active root per chunk; both roots live in the shared
  pool; no flip before the batch fence signals; `versionIndex` read once per workgroup;
  `inFlightFrame` per root; refcounts balanced; GPU is never authoritative; no
  per-frame readback; no in-place compaction of a GPU-resident chunk.
- **Verification:** `root_pair` (M4-02), `inflight` (M4-07), `recycle` (M4-08),
  `version_once` (M4-11), the root-pair benchmark (M4-17), the soak and torn-frame
  detector (M4-13/M4-14), the leak detector (M4-15), and the compaction guard
  (M1-12/M4-12).

## Rejected alternatives

- **One root overwritten in place.** The GPU may still read it; the options are a torn
  frame or a fence on every edit. Both violate the sync and edit-latency rules.
- **Copy the whole chunk DAG into the second root.** Doubles 60–90 KiB at 64³ and
  0.5–0.7 MiB at 128³ per chunk. A/B exists precisely to avoid this copy.
- **Per-chunk private pools or interners.** No cross-chunk run sharing, so two roots
  approach a true 2× and identical content is stored repeatedly. The historical
  three-tier ADR chose this to make unload trivial; refcounts plus epoch retirement buy
  the same safety without giving up sharing.
- **Flip `versionIndex` on submit.** The fence has not signalled; a traversal can read
  the new root before its upload is visible. The flip must follow the fence.
- **A global frame counter for recycling.** It either reclaims a root a lagging frame
  still reads or retains every root because one chunk lagged. Per-root tracking is the
  only rule that is both safe and starvation-local.
- **A third root by default.** Extra slot and path bytes for no measured need. It is a
  pre-specified contingency, triggered only by a measured three-frame lag.
- **Re-read `versionIndex` mid-traversal.** One dispatch would mix two roots and emit
  culling results from neither consistent version.
- **Compact a resident chunk in place.** It invalidates pointers held by either root or
  by an in-flight frame; compaction runs only outside GPU residency.

## Status of open questions

None. The third-root contingency has a measured trigger (completion lag of three
frames), not an open design question.
