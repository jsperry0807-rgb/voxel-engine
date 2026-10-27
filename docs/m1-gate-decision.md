# M1 Gate Decision

**Status:** Decided — 2026-10-27
**Task:** M1-42, following the recorded gate run M1-41
**Gate:** [pre-registration.md](pre-registration.md) (frozen 2026-10-26)
**Machine:** [reference-machine.md](reference-machine.md) — AMD Ryzen 5 7600, 6 cores / 12 threads; NVIDIA GeForce RTX 4060, 8,188 MiB; 32 GiB RAM; Linux
**Run:** [`benchmarks/runs/20261027T204626Z-m1-gate`](../benchmarks/runs/20261027T204626Z-m1-gate) at commit `f904640a`
**Raw report:** [`gate.md`](../benchmarks/runs/20261027T204626Z-m1-gate/gate.md) · [`gate.json`](../benchmarks/runs/20261027T204626Z-m1-gate/gate.json)
**128³ projection:** [`scaleup.md`](../benchmarks/runs/20261027T204626Z-m1-gate/scaleup.md) · [`scaleup.json`](../benchmarks/runs/20261027T204626Z-m1-gate/scaleup.json)

---

## 1. Decision

**The M1 gate fails. The project stays at 64³.**

- Gate verdict: **FAIL — 8 pass, 2 fail, 2 not measured** (Default profile, 64³ build).
- The memory failures trigger pre-registration §4 rule 2, not rule 1: edit latency and
  probe update pass, so 128³ is not ruled out by the latency forces.
- Rule 2 requires the 128³ projection before deciding. The projection also misses the
  memory budget in both modes (15.50 GiB optimistic, 24.74 GiB pessimistic, against the
  4.80 GiB hard-fail line). It therefore does **not** clear, so the failure is not an
  argument for the upgrade.
- Per the M1 exit rule in the roadmap and ADR-002, a failing 128³ projection records
  **stay at 64³**. No threshold is changed, and no frozen row is renegotiated.
- The 64³ memory failure remains an open engineering problem. The pre-registered
  sacrifice order for it is recorded in §4; those levers are M2 work, not an M1 gate
  waiver.

The dataset row is also not measured. The fixture is the synthetic M1-28 placeholder;
this report is a rehearsal, not an admissible real-save result. A provisional pass
would not have been sufficient to upgrade either.

## 2. Every row, with its margin

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

## 3. The 128³ projection

The M1-40 model projects the measured 64³ p95 terms to 128³ at equal loaded volume
(N64 = 25,736, N128 = 3,217):

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit total | 2.522 µs (PASS) | 36.414 µs (PASS) | 8.00 ms |
| Probe update | 590.964 µs (PASS) | 75.64 ms (MISS) | 4.00 ms |
| Memory projected | 15.50 GiB (MISS) | 24.74 GiB (MISS) | 4.80 GiB |
| DAG nodes/chunk | 248,760 (PASS) | 248,760 (PASS) | 1,500,000 |

- **Edit latency** clears 128³ under both assumptions.
- **Probe update** clears under the local-edit assumption but misses under the
  whole-chunk-rebake assumption. That is a second reason not to greenlight 128³ from
  this run; it requires an M2 measurement, not a projection claim.
- **Memory** misses in both modes. The optimistic mode already scales the mesh by
  surface area (×4) and the other components by volume (×8); even that is 3.2× the hard
  fail line. The 128³ upgrade is not supported by the measured memory.

## 4. Fallback and the work it creates

The pre-registered order is unchanged:

1. Rule 1 — latency, probe update or granularity fails at 64³ → 128³ is dead.
   **Not triggered:** edit and probe pass; granularity is unmeasured, not failed.
2. Rule 2 — a memory row fails at 64³ → run the 128³ projection. **Triggered by rows
   1 and 6.** The projection does not clear, so the failure does not argue for 128³.
3. Rule 3 — memory fails → sacrifice in order: **mesh compression, then near radius,
   then vertical shell.** Never shrink the far clip below 8 km.

Because the 128³ projection also fails, this decision records **stay at 64³** with the
measured numbers above. The rule-3 levers are the next engineering work if the memory
budget is to be recovered; they are not applied here and no budget is widened. The
two unmeasured rows must be closed before any later run can call the gate green:

- **Row 9:** attach a real culling-granularity run. The M1-34 reference and its
  100-pose test exist; the gate generator does not yet consume them.
- **Row 11:** replace the synthetic fixture with a real populated save and re-freeze
  the fixture. Until then every distribution is illustrative.

## 5. M1 exit record

- **Gate run:** `LABEL=m1-gate scripts/bench.sh`, archived in
  [`benchmarks/HISTORY.md`](../benchmarks/HISTORY.md); the report is attached to
  [PR #1](https://github.com/jsperry0807-rgb/voxel-engine/pull/1).
- **Layout:** 8.0–8.4 B/internal node including padding (limit ≤ 9); COW 258–366 B per
  edited path for one edit and 93–175 B for a 100-edit batch (limit ≤ 512); run
  alignment and dedup recorded in the same run.
- **Edge:** 64³ is the gated edge. 128³ remains compile-only (`--features chunk128`)
  and is not greenlit for M2 by this decision.
- **Next milestone:** M1.5. The gate cases remain runnable (`voxel-bench gate report`,
  `gate scaleup`) and are advisory in CI.

**Signed off:** Home <jsperry0807@gmail.com> (M1 owner), 2026-10-27.
