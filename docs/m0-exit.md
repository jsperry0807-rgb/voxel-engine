# M0 Exit Record — `m0`

**Status:** Complete
**Date:** 2026-10-26
**Tag:** `m0` (annotated)
**Scope:** [`task-list.md`](task-list.md) §3, tasks M0-01…M0-24
**Binding artifact:** [`pre-registration.md`](pre-registration.md), frozen 2026-10-26

This record signs off the four M0 exit checks and fixes the tag the roadmap
refers to. It does not amend the pre-registration; only the checklist evidence
lives here.

## 1. Checklist

| Check | Evidence | Result |
|---|---|---|
| Pre-registration table frozen | [`pre-registration.md`](pre-registration.md) is dated 2026-10-26, marked FROZEN, names the reference machine, separates memory from latency fallbacks and states the amendment rule. No dated appendix exists yet. | **PASS** |
| ADRs merged | ADR-001…ADR-013 are present under [`adr/`](adr/), each `Accepted (closed)`, and all thirteen are indexed in [`README.md`](README.md). | **PASS** |
| Constants used | [`crates/core/src/constants.rs`](../crates/core/src/constants.rs) and [`content_constants.rs`](../crates/core/src/content_constants.rs) exist with compile-time assertions; `voxel-storage` re-exports the geometry constants and uses `CHUNK_BITS`/`CHUNK_DIM` in index packing; content sizes derive from the block grid. | **PASS** |
| CI green | The required jobs in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) pass: fmt, clippy `-D warnings`, `cargo test --workspace` (168 passed), naga shader tests (4 passed). The `chunk128` job fails on the M0-08 guard exactly as designed. | **PASS** (local equivalent; see §3) |
| Tag exists | `git tag -n m0` resolves to this record's commit with the M0 exit annotation. | **PASS** |

## 2. Commands run

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p voxel-render --test shaders
cargo test --workspace --features chunk128   # expected failure: M0-08 guard
scripts/check-constants.sh                   # warn-only in M0, exits 0
git tag -n m0
```

Observed results on 2026-10-26:

- fmt: clean.
- clippy: clean; the only warning is Cargo's future-incompatibility note for
  `proc-macro-error2`, not a clippy lint.
- workspace tests: **168 passed, 0 failed, 0 ignored**.
- shader tests: **4 passed**.
- `chunk128`: fails at `crates/core/src/constants.rs` with the M0-08
  `compile_error!` guard. This is the intended state until M1-24.
- `check-constants.sh`: reports **28** known 6-bit literals outside the constants
  module and exits 0 in warn-only M0 mode.

## 3. Known deviations and follow-ups

1. **Hosted CI.** No git remote is configured, so the workflow has not run on
   GitHub yet. The local run above is the equivalent of every required job; the
   first push will produce the hosted record. The `chunk128` job remains
   `continue-on-error` with its `TODO(M1-24)` until the guard is removed.
2. **6-bit literals.** The 28 warnings are confined to the existing storage DAG,
   its tests and the benchmark suites. This is the expected M0-23 warn-only
   state; M1-19…M1-25 parameterize them and flip the guard to strict.
3. **128³ build.** The 7-bit variant is selected by the `chunk128` feature but
   deliberately fails the M0-08 guard until M1-24. M0 exit requires the switch
   to exist, not the production 128³ build.

## 4. Sign-off

**Signed off:** Home <jsperry0807@gmail.com> (M0 owner), 2026-10-26

M0 is complete. The next milestone is M1 — layout at 64³ plus the measurement
gate — and the pre-registration is binding for it.
