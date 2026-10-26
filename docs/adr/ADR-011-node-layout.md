# ADR-011: Node Layout — Level Bands, 8 B Nodes, Run Dedup, Compaction

**Status:** Accepted (closed) — 2026-10-26
**Decision owner:** engine
**Supersedes:** the per-node `u32 [header, children…]` hash-consed encoding in
[`../reference/adr-three-tier-store-v2.md`](../reference/adr-three-tier-store-v2.md)
**Related:** spec §1.2, §2, §3, §10.1–10.4, §11;
[`../pre-registration.md`](../pre-registration.md); ADR-002 chunk edge; ADR-007 A/B
roots; ADR-012 traversal; `voxel-core::node_word`; M0-05, M1-01…M1-18, M4-09, M4-12

## Context

One node layout must serve four readers at once: GPU traversal, which wants a sibling
group in one coalesced fetch; the CPU editor, which must publish a new version inside
the 8 ms budget; the A/B root scheme, which lets an old and a new version coexist; and
the disk format, which must reopen without a full rebuild. Those preferences conflict.
A per-node hash-consed pool maximises sharing but puts an independent pointer on every
node, so traversal becomes a chain of dependent loads and a one-voxel edit can copy
nodes that a sibling-group layout would copy as one line. A whole-chunk copy is
contiguous but duplicates **60–90 KiB at 64³ and 0.5–0.7 MiB at 128³** on every edit
(ADR-007). Dedup of whole chunks is weaker still: terrain does not repeat entire chunks.

The memory cap is explicit. The pre-registration allows **200 k unique nodes/chunk at
64³ and 1.5 M at 128³**; at **8 B/node × two roots = 16 B/node** that is a **3.2 MB /
24 MB hard cap**, not a p95 target. The layout therefore has to make the common case
dense and the edit path bounded, not merely maximise sharing.

The resolution is to make the **sibling run** — the present children of one parent —
the unit of locality, sharing, COW and reclamation, rather than the individual node.

## Decision

**The store is a set of per-level append-only bands of 8 B internal nodes. A parent's
present children form one contiguous run. Runs are the unit of hash-consing, 64 B
alignment and COW; bands are compacted offline only when dead or padding bytes exceed
25%.**

1. **Node word.** One 64-bit word: `mask: u8` (bits 0–7), `level: u3` (bits 8–10),
   `flags: u5` (bits 11–15, reserved and zero in M0), `base: u48` (bits 16–63). Child
   *c* lives at `base + popcount(mask & ((1 << c) − 1))`; absent children cost no slot.
   `voxel-core::node_word` is the only encoder and decoder. Internal levels are
   **8 B/node**; leaves are `u16` material IDs, eight per 16 B leaf run, with `0 = air =
   absent`. The `u48` base bounds the pool at `2^45` runs, beyond any streamed world.

2. **Bands.** Every level has its own contiguous append-only arena and the chunk carries
   `levelStart[ROOT_LEVEL + 1]`, plus a leaf arena. A fresh build emits root → leaves
   (BFS), so the chunk is strictly level-ordered and `levelStart` is uploaded once per
   version (M4-09). Appends go to the tail of the owning band, so existing indices never
   move and no relocation is needed. After the first edit a band is **level-ordered,
   append-ordered**, not strictly BFS; children are reached by `base`, not by band
   position, so that is legal.

3. **Runs and alignment.** A run is the `popcount(mask)` present children in ascending
   child order, contiguous in the next level band. A full internal run (`mask = 0xFF`) is
   **64 B aligned**, so its eight nodes occupy exactly one cache line and one fetch.
   Partial runs are 8 B aligned and packed; padding is bounded by **56 B per full run**,
   is queryable (M1-03), and counts toward the compaction trigger. A run never crosses
   a band and never spans two parents.

4. **Run dedup and COW.** Hash-consing keys internal runs on
   `(level, mask, ordered child ids)` and leaf runs on their eight material IDs, with a
   collision-safe equality check. Two parents share a run only when the entire ordered
   child set matches; a single child is never shared independently of its siblings. A
   COW edit therefore duplicates exactly one run per level of the edited path —
   `ROOT_LEVEL` runs of **≤ 64 B each: ≤ 384 B at 64³, ≤ 448 B at 128³**. That cost is
   bounded by path depth, not chunk volume, and untouched runs stay shared across
   versions and chunks (ADR-007). Retired runs are reclaimed by the epoch list, never
   overwritten while a root or frame can still read them.

5. **Compaction.** Compaction is storage reclamation, not edit consolidation. When a
   band's dead or padding bytes exceed **25%**, renumber that band, remap every parent
   base that points into it, rebuild leaf references and bump the chunk version. It may
   run only when neither GPU root and no in-flight frame references the chunk; the
   implementation asserts this and therefore panics rather than compacting a resident
   chunk in place (M1-12, M4-12). The new band is published like any other version, so
   a traversal sees either the old or the new numbering, never a half-renumbered band.
   Below the threshold, do nothing: append-only growth is accepted until it is worth
   paying the renumber.

## Consequences

- **Positive:** one 64 B fetch covers a full sibling group with no pointer chase.
  Contiguous bands make level upload a range copy and keep pointers stable under
  append. 8 B/node is one word for mask, level and base, and the fixed stride makes the
  invariant checker, serialization, compaction and upload simpler. COW is path-bounded
  (**≤ 384 B / ≤ 448 B**), which is the property that keeps A/B roots affordable.
- **Negative:** run-granular dedup loses sharing when only one of eight siblings
  differs, and a one-voxel edit rewrites one run per level even though one child
  changed. Full-run alignment can waste up to 56 B per full run. Compaction invalidates
  parent bases, requires GPU quiescence and costs a version bump. A band may carry up to
  25% dead or padding bytes before anything is done.
- **Explicit trade:** run-level sharing plus cache-line alignment is chosen over
  per-node sharing and variable-length packing. The price is at most one 64 B run per
  level plus bounded padding; the return is single-fetch traversal, stable append-only
  bands and a simple COW path. This is not per-node hash-consing and not a whole-chunk
  copy.
- **Invariants:** every internal node is exactly 8 B; `base` is a node-slot index into
  the next band; the flag bits stay zero in M0; child *c* is at
  `base + popcount(mask & ((1 << c) − 1))`; a `0xFF` run is exactly 64 B aligned and
  occupies one cache line; partial runs are 8 B aligned; no run crosses a band; leaf
  material `0` means air and is absent; COW never mutates a live run; compaction is
  offline only and bumps the version.
- **Verification:** `node_word` (M1-01), `bands` (M1-02), `run_align` (M1-03),
  `run_intern` (M1-04), `leaf_runs` (M1-06), `bfs_build` (M1-07), `cow_paths` (M1-10),
  `uniform_collapse` (M1-11), `compaction` plus the live-root `#[should_panic]` test
  (M1-12), the layout cost report — bytes/node including padding, run count, dedup
  ratio, COW bytes/path (M1-18) — `band_upload` (M4-09) and the compaction guard
  (M4-12).

## Rejected alternatives

- **Per-node hash-consing, the old `u32 [header, children…]` encoding.** It maximises
  per-node sharing but stores a header on every node, pays a dependent pointer load per
  child and gives no run alignment; a one-node change is not a one-line change. This is
  the encoding the ADR supersedes.
- **Copy the whole chunk DAG on edit.** Simple pointer arithmetic, but it duplicates
  60–90 KiB at 64³ and 0.5–0.7 MiB at 128³ per edit and approaches 2× residency. A/B
  roots and run COW exist to avoid exactly that cost.
- **Dedup whole chunks by Merkle root.** Only byte-identical chunks share; real terrain
  never repeats an entire chunk, so the dedup ratio collapses and nothing below the root
  is reused.
- **64-bit child pointers in the node.** A full pointer consumes the word, leaving no
  room for mask, level and base, and forces a second word or a second indirection. The
  compact `u48` base is the reason 8 B is enough.
- **Pack runs across parents to remove padding.** It would break the one-run-per-parent
  contract that `base + popcount` addressing and compaction rely on, and it would
  destroy the 64 B one-fetch guarantee. Up to 56 B padding is the cheaper price.
- **Larger runs (several parents in a 128 B block).** A parent has at most eight
  children; a multi-parent block cannot be addressed by one `base` with `popcount`, and
  it makes dedup coarser for no fetch benefit.
- **Compact on a fixed cadence, or always.** Unnecessary renumbering, pointer
  invalidation and upload bytes for bands that are mostly live. The 25% threshold ties
  work to actual waste.
- **Compact a GPU-resident chunk in place.** It invalidates bases held by either root
  or an in-flight frame; the result is a torn traversal or a fence on every edit.

## Status of open questions

None. The node width, band structure, run granularity, dedup key, alignment and
compaction trigger are closed. M1-18 measures the padding and dedup ratio, but changing
the unit of sharing or the trigger requires a new ADR, not an open question in this one.
