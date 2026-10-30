# M1 Gate Decision

**Status:** Decided — 2026-10-27; revised 2026-10-27 after the per-profile projection
**Task:** M1-42, following the recorded gate run M1-41
**Gate:** [pre-registration.md](pre-registration.md) (frozen 2026-10-26)
**Machine:** [reference-machine.md](reference-machine.md) — AMD Ryzen 5 7600, 6 cores / 12 threads; NVIDIA GeForce RTX 4060, 8,188 MiB; 32 GiB RAM; Linux
**Run:** [`benchmarks/runs/20261027T204626Z-m1-gate`](../benchmarks/runs/20261027T204626Z-m1-gate) at commit `f904640a`
**Raw report:** [`gate.md`](../benchmarks/runs/20261027T204626Z-m1-gate/gate.md) · [`gate.json`](../benchmarks/runs/20261027T204626Z-m1-gate/gate.json)
**128³ projection:** [Default](../benchmarks/runs/20261027T204626Z-m1-gate/scaleup.md) · [Minimum](../benchmarks/runs/20261027T204626Z-m1-gate/scaleup-minimum.md)

---

## Amendment — 2026-10-29: the v1 pool is resident and now counted

The M1 memory model excluded the transitional v1 `Svdag` pool because it is a
representation the banded layout replaces. It is still allocated for every
resident chunk (disk and GPU compatibility), so the gate now counts it as a
`legacy_v1` component.

- 64³ p95 total: **1,199,068 B/chunk** (was 1,033,124 B); the pool is 167,968 B,
  **14.0%** of the corrected total.
- **128³ Minimum optimistic: 4.89 GiB**, above the **4.80 GiB** hard-fail line
  (pessimistic 7.20 GiB).
- The 3.88 GiB figure in §1/§2 and ADR-002 is the model **without** the pool.

No threshold, row or budget changed. The M2 greenlight now carries an extra
condition: the pool must be removed by **M6-09a** (banded disk records and GPU
import) before the 128³ edge is confirmed, in addition to mesh compression.

---

## 1. Decision

**The 64³ Default gate fails. M2 is greenlit to implement and measure the 128³
Minimum profile; M1 remains built and measured at 64³.**

- Gate verdict at the measured edge: **FAIL — 8 pass, 2 fail, 2 not measured**
  (Default profile, 64³ build).
- The memory failures trigger pre-registration §4 rule 2. Rule 2 does not say
  "stay at 64³"; it says to evaluate the 128³ edge at the same budget.
- The 128³ **Default** projection fails (15.50 GiB optimistic, 24.74 GiB
  pessimistic). The 128³ **Minimum** projection is **3.88 GiB optimistic**
  against the 4.80 GiB hard-fail line, and 6.19 GiB pessimistic.
- The optimistic memory model is the physically appropriate one: mesh bytes grow
  with surface area, while DAG, light and probes grow with volume. The
  pessimistic bound assumes the mesh grows with volume; it is reported, not
  silently discarded, and the M2 128³ measurement must retire it.
- The 128³ Minimum projection is therefore the rule-2 upgrade candidate. It is
  not a final pass: row 6 still needs mesh compression, and the M2 run must
  measure the real 128³ build before the edge is confirmed.
- The 24.76 GiB figure is the **64³ Default p95 rehearsal**, not a shipping
  estimate. It is the known memory-risk profile that selects the edge; the
  engine is not being designed to allocate 24.76 GiB.
- No frozen budget, row or threshold was changed.

## 2. Why 24.76 GiB is not the number to optimize

| Configuration | Loaded chunks | Per-chunk p95 | Projected | 4.80 GiB hard fail |
|---|---:|---:|---:|---|
| 64³ Default | 25,736 | 1008.9 KiB | 24.76 GiB | **FAIL** |
| 64³ Minimum | 6,440 | 1008.9 KiB | 6.20 GiB | **FAIL** |
| 128³ Default | 3,217 | 4.93 / 7.88 MiB | 15.50 / 24.74 GiB | **FAIL** both |
| 128³ Minimum | 805 | 4.93 / 7.88 MiB | 3.88 / 6.19 GiB | PASS optimistic / MISS pessimistic |

The pre-registration already predicts 6.0–17.2 GiB for 64³ Default and calls it
the memory-risk profile ([`pre-registration.md`](pre-registration.md) §2, §5).
The 24.76 GiB is that prediction plus the measured fixture's high-end p95. It is
a verdict on 64³ Default, not a requirement the engine must meet.

The per-chunk projection is profile-independent; only the loaded count changes.
That is why the profile argument belongs in the scale-up report, not in a
hand-written multiplier. `voxel-bench gate scaleup --profile minimum` now emits
the fallback projection directly.

## 3. Every row, with its margin (Default, 64³)

| # | Pre-registered row | Requirement | Measured | Margin | Result |
|---|---|---|---|---|---|
| 1 | VRAM budget for voxel data | ≤ 4.80 GiB hard fail (3.20 GiB target) | 24.76 GiB (p95 1008.9 KiB × 25,736 chunks) | −19.96 GiB to hard fail | **FAIL** |
| 2 | Target loaded chunk count | N64 = 25,736 = 8 × N128; table predicted N64 = 25,736, N128 = 3,216 | N64 = 25,736 at 64³; N128 = 3,217 at 128³ equal volume | 0 chunks (equal-volume identity) | **PASS** |
| 3 | SVDAG node budget/chunk | ≤ 200,000 nodes/chunk at 64³ | max 36,946 nodes/chunk (p95 32,405) | +163,054 nodes | **PASS** |
| 4 | Light volume/chunk | = 1,024 B/chunk at 64³ (64 cells) | 1,024 B/chunk; p50 1,024 B | 0 B | **PASS** |
| 5 | Probe grid/chunk | = 64 probes = 768 B/chunk at 64³ | 64 probes/chunk, 768 B; 4,096 rays/full rebake | 0 B | **PASS** |
| 6 | Mesh buffer/chunk | ≤ 600 KiB/chunk at 64³ | p95 753 KiB (max 775 KiB) | −153.1 KiB | **FAIL** |
| 7 | Edit latency | p95 ≤ 8.00 ms | p95 2.615 µs (p50 1.343 µs, max 14.598 µs); 1,000 edits | +8.00 ms | **PASS** |
| 8 | Probe-update budget | p95 ≤ 4.00 ms per edited chunk | p95 146.098 µs (p50 54.734 µs, max 239.956 µs); 1,000 edited chunks | +3.85 ms | **PASS** |
| 9 | Culling granularity | ≤ 15% traversed-but-invisible | not measured by this run | n/a | **NOT MEASURED** |
| 10 | Safety factor | hard fail = 4,913 MiB | 4,913 MiB = ceil(1.5 × 3,275 MiB) | 0 MiB | **PASS** |
| 11 | Measurement dataset | real populated save | m1-representative (provisional; synthetic) | n/a | **NOT MEASURED** |
| 12 | Fallback trigger | pre-registration §4 fallback order | rule 2 triggered by the measured rows | n/a | **PASS** |

The component split behind row 1 is DAG p95 253.2 KiB, light 1.0 KiB, probes 0.8 KiB,
overlay 0 KiB and mesh 753.1 KiB. The mesh estimate is the dominant term; the DAG alone
would still project to 6.21 GiB at 25,736 chunks.

## 4. The 128³ projections

### Default (3,217 chunks)

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit total | 2.974 µs (PASS) | 43.900 µs (PASS) | 8.00 ms |
| Probe update | 608.996 µs (PASS) | 77.95 ms (MISS) | 4.00 ms |
| Memory projected | 15.50 GiB (MISS) | 24.74 GiB (MISS) | 4.80 GiB |
| DAG nodes/chunk | 248,760 (PASS) | 248,760 (PASS) | 1,500,000 |

### Minimum (805 chunks)

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit total | 3.226 µs (PASS) | 46.487 µs (PASS) | 8.00 ms |
| Probe update | 653.640 µs (PASS) | 83.67 ms (MISS) | 4.00 ms |
| Memory projected | 3.88 GiB (PASS) | 6.19 GiB (MISS) | 4.80 GiB |
| DAG nodes/chunk | 248,760 (PASS) | 248,760 (PASS) | 1,500,000 |

- **Edit latency** clears 128³ at both profiles under both assumptions.
- **Probe update** clears under the local-edit assumption but misses under the
  whole-chunk-rebake assumption. The design only rebakes affected probes, so the
  optimistic term is the expected one; the pessimistic term is the bound the M2
  measurement must close.
- **Memory** at Minimum clears the hard-fail line optimistically, 0.68 GiB above
  the 3.20 GiB target. At Default it misses in both modes.
- A 128³ chunk's mesh is 4× the 64³ mesh at the same surface, not 8×. The
  pessimistic memory model is a volume-scaled bound; the optimistic model is the
  one that matches the content path.
- The projection remains analytic and the fixture remains provisional. The M2
  128³ run is the measurement that confirms or rejects the upgrade.

## 5. Fallback and the work it creates

The pre-registered order is unchanged:

1. Rule 1 — latency, probe update or granularity fails at 64³ → 128³ is dead.
   **Not triggered:** edit and probe pass; granularity is unmeasured, not failed.
2. Rule 2 — a memory row fails at 64³ → run the 128³ projection. **Triggered by
   rows 1 and 6.** 128³ Default does not clear; **128³ Minimum clears the
   hard-fail line optimistically**, so the failure argues for the upgrade at the
   fallback profile.
3. Rule 3 — memory still fails at 128³ → sacrifice in order: **mesh compression,
   then near radius, then vertical shell.** Never shrink the far clip below 8 km.

The upgrade is conditional on closing the two measured gaps in M2:

- **Mesh compression (row 6):** the 128³ mesh cap is 2.4 MiB/chunk. The measured
  64³ p95 of 753 KiB scales to 3.0 MiB, 0.6 MiB over. Bringing the 64³ estimate to
  the 600 KiB cap also brings the 128³ estimate to 2.4 MiB. This is the first
  rule-3 lever and the first M2 task.
- **Real save and granularity (rows 9 and 11):** the M1-34 granularity reference
  must be wired into the gate report, and a real populated save must replace the
  synthetic fixture. Until both are closed, the gate report stays provisional.

If the M2 128³ measurement exceeds 4.80 GiB, the upgrade is rejected and the
project falls back to 64³ Minimum with the rule-3 reductions. No budget is
widened in either direction.

## 6. M1 exit record

- **Gate run:** `LABEL=m1-gate scripts/bench.sh`, archived in
  [`benchmarks/HISTORY.md`](../benchmarks/HISTORY.md); the report is attached to
  [PR #1](https://github.com/jsperry0807-rgb/voxel-engine/pull/1).
- **Measured edge:** 64³. The 128³ variant remains compile-only until M2.
- **M2 target:** 128³ Minimum, conditional on the M2 128³ gate measurement and
  mesh compression. Default is not greenlit.
- **Layout:** 8.0–8.4 B/internal node including padding (limit ≤ 9); COW 258–366 B
  per edited path for one edit and 93–175 B for a 100-edit batch (limit ≤ 512).
- **Next milestone:** M1.5, then M2. M1.5 is code-complete; only `M1.5-17`
  remains open. M2 is in progress: `M2-01` (apron) landed on
  2026-10-28. Gate cases remain runnable (`voxel-bench gate report`,
  `gate scaleup --profile minimum`) and are advisory in CI.

**Signed off:** Home <jsperry0807@gmail.com> (M1 owner), 2026-10-27.

## 7. Implementation progress — 2026-10-29

This section records progress after the decision. It does not change the frozen
gate, the chosen profile or any threshold.

- **M1.5 vertical slice:** `voxel-viewer` opens a `winit`/`wgpu` window, loads and
  verifies the committed atlas, draws one generated chunk with the naive
  per-face mesher, and has the camera, mouse look and input layer. The
  `docs/images/m1.5-13-chunk-1080p.png` artifact and viewer README are committed.
- **M1.5 exit artifact still open:** `M1.5-17` (before/after texel-density
  screenshots). M1.5-15 is closed by the full D9 set under `docs/images/m1.5-15-*`
  and the viewer README.
- **M1.5-16 closed:** `crates/engine/viewer/tests/validation.rs` validates the
  chunk shader with naga and records an offscreen chunk draw inside a wgpu
  validation error scope. CI runs it with `RUST_LOG=wgpu_core=warn` against
  Lavapipe.
- **M2 started:** `M2-01` six-face apron landed. `M2-02` measured the apron at
  8.5% of the dense path's bytes and 6.6% of its time at 64³ (4.5%/4.3% at
  128³); see [`m2-apron-cost.md`](m2-apron-cost.md). `M2-03` added the
  block-granular driver with an 11,664 B reusable scratch; `M2-04` added the
  greedy rectangle core and matched the M1-31 reference on five fixtures;
  `M2-05` added the `(material, orientation, 4×AO)` merge key; `M2-06` culls
  interior faces before the mask; `M2-07` samples four-level vertex AO and
  matches the brute-force reference on 10k corners; `M2-08` measured a 1.65×
  quad cost for AO-safe merging; `M2-09` passes the chunk-border AO seam;
  `M2-10` packs the 16 B vertex and round-trips all fields; `M2-11` freezes five
  golden mesh fixtures at both edges; `M2-12` passes 1/2/4/8-worker and reversed
  order determinism; `M2-13` ports the full-pass block-light fill and matches
  the M1-29 reference on five fixtures; `M2-14` seeds the apron across chunk
  borders and matches the union fill; `M2-15` updates 1,000 random edit sets to
  the same values as a full recompute; `M2-16` escalates boundary removals to a
  full relight. The next task is `M2-17`, the light cost benchmark.
- **128³ Minimum is still unmeasured.** The M2 gate run after `M2-29` is the
  measurement that confirms or rejects the upgrade; mesh compression remains
  the first lever.
