# Interim scope — v1 pool residency

**Status:** scoped, not scheduled · **Related:** M6-09a (banded disk/GPU migration)
**Context:** `CODE-REVIEW-2026-10-29.md`, M1 decision amendment (2026-10-29)

## Why

`Svdag` keeps two encodings of the same chunk: the authoritative banded
`ChunkDag` and a canonical v1 `pool` used by the disk record (`to_bytes`), GPU
import (`GpuStore::set_chunk`) and the legacy layout bench. The pool is
**resident for every loaded chunk**:

| Measurement | Value |
|---|---:|
| 64³ fixture, pool p50 / p95 | 31,472 B / 167,968 B per chunk |
| Share of corrected per-chunk p95 | 14.0 % |
| 64³ fixture, total resident | 5.67 MB over 72 chunks |
| 128³ Minimum projection (volume-scaled) | ~1.01 GiB over 805 chunks |

Counting it moves the 128³ Minimum optimistic projection from 3.88 to
**4.89 GiB** against the 4.80 GiB hard-fail line. Removing the *resident* copy
recovers that margin without changing the disk format, the GPU layout, or any
gate threshold.

## Goal

Keep the v1 view as a **transient** build product: exist only while a save or a
GPU import is running, never between frames.

**Non-goals:** no disk-format change, no shader/GPU-layout change, no gate
threshold change, no removal of the `LegacyPool` encoder (it becomes the
on-demand builder). The end-state format migration remains M6-09a.

## Design

### Representation

```rust
pub struct Svdag {
    chunk: ChunkDag,      // authoritative
    root: u32,            // v1 root offset, cheap to keep
    v1_words: usize,      // logical pool length, for words()/metrics
}
```

- `pool: Vec<u32>` is deleted from the struct.
- `root` stays (4 bytes/chunk) because `to_bytes` and import both need it.
- `v1_words` is recorded at construction (the canonical pool is built once
  there anyway) so `words()` stays O(1) and keeps its historical meaning for
  buffer sizing and benchmarks.
- Construction (`from_chunk`, `from_bytes`) builds the canonical pool once,
  records `root`/`v1_words`, and drops the `Vec`. Peak memory during
  construction is unchanged (one pool at a time).

### On-demand access

```rust
/// Canonical v1 view, rebuilt from the banded chunk.
fn legacy_pool(&self) -> (Vec<u32>, u32);   // crate-visible
```

Consumers:

| Consumer | Change |
|---|---|
| `Svdag::to_bytes` | build, serialise, drop |
| `GpuStore::set_chunk` | build, `import(&pool, root)`, drop |
| `bench/layout.rs` legacy positions | build, walk, drop |
| `Svdag::words()` | return stored `v1_words` |
| `Svdag::PartialEq` | exact compare by rebuilding both sides (equality is test-only today); no hash-collision semantics |
| tests using `.pool` | `dag.legacy_pool()` or `dag.words()` |

`to_bytes` gains a scratch-buffer variant (`to_bytes_into(&self, &mut Vec<u32>)`)
so the consolidation path can reuse one allocation across chunks.

### Where the transient pool is built

- **Load / `set_chunk`**: once per chunk, at import. Same order of work as
  today, plus one banded→v1 encode.
- **Save / `put_dag`**: once per consolidated chunk. This is a *new* encode on
  the save path (today `apply_edits` left the pool in place); see Costs.
- Edits (`apply_edits`) no longer retain the pool they build for `v1_words`.

## Costs and risks

1. **Extra encode on save.** `apply_edits` already encodes the pool to compute
   `v1_words`; `put_dag` then rebuilds it. Mitigation options, in order:
   - reuse one scratch `Vec` per flush (`to_bytes_into`);
   - measure `dag/build/*` and the edit p95 before/after; the gate's 8 ms budget
     has ~5 ms of headroom at p95 2.6 µs, so one extra encode is likely noise,
     but it must be measured.
2. **Equality is O(nodes) instead of O(1).** Production code does not compare
   `Svdag`s; tests do (edit-vs-rebuild, store round-trips). Acceptable. If it
   ever becomes hot, switch to a 256-bit content hash stored at construction.
3. **API break.** `Svdag::pool` is public and used by `bench/layout.rs` and
   tests. This is a deliberate, internal-only break (crate + workspace tests).
4. **Peak, not average.** Importing/saving a 128³ chunk allocates ~1.3 MB
   transiently. RSS per frame is unchanged; the M6-09a migration removes even
   that.

## Gate and reporting changes

- `gate memory`: `legacy_v1` is reported as **transient, not resident**:
  `p50/p95/max = 0` for resident bytes, with the transient peak (one chunk)
  noted. `total_bytes` stops including it.
- `gate scaleup`: the pool contributes nothing to `total_bytes_per_chunk`;
  the 128³ projection returns to the 3.88 GiB basis **until** a chunk is being
  saved/imported.
- Amendment note: this changes the model again; the decision doc should record
  "counted while resident, removed by the interim lifetime change, removed
  entirely by M6-09a".
- No threshold, row or pre-registration change.

## Test plan

1. **Byte identity:** for every fixture chunk, `to_bytes()` after the change is
   byte-identical to `to_bytes()` at `76436cc` (compare hashes offline).
2. **Equality:** edit-vs-dense-rebuild still equal; a one-voxel difference not
   equal.
3. **Round-trip:** `from_bytes(to_bytes(dag))` equals `dag` and expands to the
   same dense array.
4. **GPU import:** `MemSink` import produces the same node pool as today
   (compare `GpuStore::pool()` before/after on the fixture).
5. **Memory:** `gate memory` reports v1 resident = 0; the component total no
   longer includes it.
6. **Cost:** `cargo bench -p voxel-bench --features iai --bench iai` and the
   edit p95 in `gate edits` within noise of baseline.
7. **Leak:** a loop of 1,000 save/import cycles keeps RSS flat (reuse scratch).

## Sequencing and effort

1. Representation + `legacy_pool()` + `words()`; update `PartialEq` (0.5 d).
2. Consumers: disk, GPU import, layout bench, tests (0.5 d).
3. Gate memory/scaleup reporting + doc amendment (0.5 d).
4. Bench/verify + 128³ measurement when the M2 fixture exists (0.5 d).

**~2 days**, three commits. Land after the in-flight M2 mesh work to avoid
conflicts in `bench` and `storage` tests.

## Acceptance

- No `pool` field; `Svdag` holds only `ChunkDag`, `root`, `v1_words`.
- Save/import bytes and GPU node pools are unchanged.
- `gate memory` shows zero resident v1 bytes; 128³ Minimum projection no longer
  includes the pool.
- Workspace tests, clippy and `texgen check` green.
