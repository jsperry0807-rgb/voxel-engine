# ADR-002: Chunk Edge — 64³ Baseline, 128³ Pre-Registered

**Status:** Accepted (closed) — 2026-10-26; M1 outcome recorded 2026-10-27 — 128³ Minimum greenlit for M2
**Decision owner:** engine
**Supersedes:** the "64³ as the final chunk edge" position in
[`../reference/material-spec-v1.md`](../reference/material-spec-v1.md)
**Related:** spec §1.2, §9.1, §10; [`../pre-registration.md`](../pre-registration.md);
[`../performance-targets.md`](../performance-targets.md); ADR-001, ADR-010

## Context

Chunk edge is `2^CHUNK_BITS` blocks per edge, and every level, AABB, band and buffer
size is written in `CHUNK_BITS` (spec §1.2). The choice is not free. At fixed loaded
volume, memory is roughly neutral, but meshes and overlays scale with surface area and
chunk count, so the smaller edge carries 8× the chunks per edge doubling. Edit latency,
consolidation tail, scratch memory and culling granularity scale with chunk volume, so
the larger edge gets worse exactly where the 8 ms and 15% gates bite.

The frozen prediction makes neither edge obviously safe. At Default, 64³ holds 25,736
chunks at 0.24–0.70 MiB = **6.0–17.2 GiB**; 128³ holds 3,216 at 1.2–3.2 MiB =
**3.8–9.8 GiB**. Both are measured against a 3,275 MiB target and a 4,913 MiB fail
line ([`../pre-registration.md`](../pre-registration.md) §2–3). The gate, not intuition,
must choose.

## Decision

**M1 builds 64³. 128³ is pre-registered, parameterized and not built in M1.**

1. **64³ is the only edge built and benchmarked in M1** (`CHUNK_BITS = 6`, 4 m,
   64 blocks, 64 m³). Content, meshing, light, probes and culling all run at 64³. There
   is no 128³ art path, no 128³ tuning and no 128³ benchmark.
2. **128³ remains a compile-time variant, not a product path** (`CHUNK_BITS = 7`,
   8 m, 512 blocks, 512 m³). It must compile under `--features chunk128` in CI
   (M1-24, M1-26); the temporary `compile_error!` guard stops a 7-bit build silently
   running 6-bit code. The upgrade is a `CHUNK_BITS` flip plus a gate re-run in M2 —
   not a rewrite.
3. **The gate is the frozen table, not a judgement call.** On the reference machine,
   with a real populated save: measured p95 per-chunk memory × target loaded chunk
   count ≤ 40% VRAM (3,275 MiB) × 1.5 = **4,913 MiB**; edit p95 ≤ **8 ms**; probe-update
   p95 ≤ **4 ms**; culling granularity ≤ **15%** traversed-but-invisible at chunk
   boundaries at max view. Median, p95 and max per chunk are reported; synthetic chunks
   are inadmissible, and there is no renegotiation inside M1 (spec §9.1).
4. **The fallback trigger separates the three failures.** Per pre-registration §4:
   - Latency, probe or granularity fails at 64³ → **128³ is dead**; stay at 64³.
   - Memory fails at 64³ → **do not automatically stay**; run the analytic 128³
     projection, because fewer chunks can win the memory comparison. If 128³ clears
     there, the failure argues *for* the upgrade.
   - Memory fails at 128³ → sacrifice in order: mesh compression, near radius,
     vertical shell. Never shrink the far clip below 8 km to fix memory.
5. **M1-42 records the outcome.** Pass → amend this ADR to greenlight 128³ for M2.
   Fail → record stay-64³ with its measured numbers. M2 onward runs at the gated edge.

## Outcome — 2026-10-27

The M1 gate ran on the reference machine and **failed at the measured 64³ Default
profile: 8 pass, 2 fail, 2 not measured**
([`../m1-gate-decision.md`](../m1-gate-decision.md);
[`gate.md`](../../benchmarks/runs/20261027T204626Z-m1-gate/gate.md)). The failures are
the VRAM budget (24.76 GiB projected against the 4.80 GiB hard-fail line) and the mesh
buffer (p95 753 KiB against 600 KiB). The 24.76 GiB is the known 64³ Default risk
profile, not a shipping estimate.

Decision 5 applies the rule-2 fallback. The 128³ Default projection fails
(15.50 GiB optimistic / 24.74 GiB pessimistic), but the **128³ Minimum** projection
is **3.88 GiB optimistic** against the 4.80 GiB hard-fail line (6.19 GiB pessimistic).
The optimistic memory model is the physically appropriate one: mesh grows with
surface area, DAG/light/probes with volume. The **128³ Minimum edge is therefore
greenlit for M2 implementation and measurement**, conditional on mesh compression
(row 6) and the M2 128³ gate run. Default is not greenlit. If the 128³ measurement
exceeds 4.80 GiB, the upgrade is rejected and rule 3 applies.

No frozen row and no budget was changed, and this ADR's decision is not amended.

**Amendment (2026-10-29).** The memory model above excluded the resident
transitional v1 `Svdag` pool. Counted as a component, the **128³ Minimum
optimistic projection is 4.89 GiB**, above the 4.80 GiB hard-fail line
(pessimistic 7.20 GiB). The M2 greenlight therefore carries a second condition
alongside mesh compression: remove the pool (M6-09a, banded disk records and GPU
import) before the 128³ edge is confirmed. Thresholds are unchanged.

## Consequences

- **Positive:** one measured edge, one content path and one set of tuning targets in
  M1. The store already matches 64³, so M1 is parameterization and measurement rather
  than a coordinate migration. The upgrade stays cheap because no literal 64, 128, 6
  or 7 escapes the constants module.
- **Negative:** 64³ is the memory-risk profile, not the safe one: 25,736 chunks at
  Default, 8× the chunk count of 128³. Mesh memory is 150–600 KB/chunk at 64³ against
  600 KB–2.4 MB at 128³, and meshes and overlays scale with area and chunk count.
  Accepted because the gate can still choose 128³ on memory.
- **Explicit trade:** M1 does not answer "ship at 64³". It answers whether 64³ clears
  latency and granularity, and what the 128³ projection costs. The shipping edge is
  decided at M1-42, not assumed.
- **Invariants:** no 6-bit or 7-bit literals outside `voxel-core::constants` and its
  assertions; both roots remain 16 B/chunk (8 B/node); node budgets 200 k at 64³ and
  1.5 M unique nodes at 128³; light 1 KiB/8 KiB; probes 768 B/6 KiB; no CPU readback;
  no far-clip cut for memory.
- **Verification:** `chunk128` guard and compile-only CI (M0-08, M1-24, M1-26); edit,
  probe, memory and granularity runs (M1-35…M1-40); gate decision with every threshold
  and margin (M1-42).

## Rejected alternatives

- **Build 128³ directly in M1.** It is the riskier latency profile, needs a larger
  content path before the first real save exists, and buys nothing the gate cannot
  decide later with a constant flip and a re-run.
- **Commit to 64³ permanently.** The pre-registration shows 64³ can lose the memory
  comparison at equal loaded volume; freezing it would discard the only configuration
  that may fit the Default profile.
- **Build both edges in parallel.** Two content paths, two tuning targets and a
  doubled gate for a decision the same measurements already make.
- **Choose by intuition or synthetic chunks.** Forbidden by the pre-registration:
  synthetic chunks hide the populated-save distribution the gate exists to measure.
- **Shrink the far clip to fix memory.** The 8 km horizon is game identity
  ([`../performance-targets.md`](../performance-targets.md) §4); memory failures follow
  the sacrifice ladder instead.

## Status of open questions

None. The edge is parameterized and the gate owns the outcome; "stay at 64³" and
"greenlight 128³" are both already-specified outcomes, not new architecture.
