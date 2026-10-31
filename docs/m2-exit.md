# M2 exit: the measured 128³ Minimum run

**Date:** 2026-10-31
**Milestone:** M2 (production content pipeline), task M2-30
**Run:** [`benchmarks/runs/20261031T152718Z-m2-gate-128`](../benchmarks/runs/20261031T152718Z-m2-gate-128)
**Fixture:** [`benchmarks/fixtures/m2-128`](../benchmarks/fixtures/m2-128) — 9 chunks, 128³ edge, synthetic and provisional
**Profile:** `minimum` (gated)
**Gate report:** [`gate.md`](../benchmarks/runs/20261031T152718Z-m2-gate-128/gate.md) · [`gate.json`](../benchmarks/runs/20261031T152718Z-m2-gate-128/gate.json)
**Granularity:** [`granularity.md`](../benchmarks/runs/20261031T152718Z-m2-gate-128/granularity.md)
**Scale-up:** [`scaleup.md`](../benchmarks/runs/20261031T152718Z-m2-gate-128/scaleup.md)

---

> **Erratum 2026-10-31 (Phase 0, M3-00).** The row-6 figure in the archived M2
> run is the **M1-31 no-AO reference**. The **production AO mesh** measures
> **3,650 KiB p95** (3,645 KiB payload + 16 B/block records + the u16 table), a
> **1,250 KiB (52.1%)** miss, not 119.4 KiB. Row 1 becomes **4.16 GiB**
> (p95 5.42 MiB × 805; still under the 4.80 GiB hard fail, over the 3.20 GiB
> target by 987 MiB). The verdict (FAIL) is unchanged; the severity is not.
> Phase 0 runs: `benchmarks/runs/20261031T163853Z-m2-production-mesh-128` and
> `...-64`. The archived M2 run remains append-only.

## 1. Verdict

**128³ Minimum is measured, but not confirmed.** The measured VRAM projection
clears the 4.80 GiB hard-fail line at **3.29 GiB** (archived reference; production
**4.16 GiB**, see erratum), and edit, probe, node, light and probe-grid rows pass.
The archived **mesh-buffer row misses by 119.4 KiB** (2,519 KiB
measured against the 2,400 KiB cap), so the pre-registered rule-3 lever — mesh
compression — is still open. Gate row 9 (culling granularity) is measured and
formally passes, but it is non-discriminating on this synthetic fixture. Gate row
11 (real populated save) is **carried**: no real save exists before
M2.5/worldgen or gameplay, so the dataset row remains not measured.

This is an engineering exit with a measured open condition, not a green light
for 128³ Default or a confirmed 128³ Minimum.

## 2. The twelve rows

| # | Row | Measured | Limit | Result |
|---|---|---|---:|---|
| 1 | VRAM budget | 3.29 GiB archived (production 4.16) | 4.80 GiB hard fail | **PASS** |
| 2 | Loaded chunks | N64 6,440; N128 805 | equal-volume identity | **PASS** |
| 3 | SVDAG nodes/chunk | max 138,636 | 1,500,000 | **PASS** |
| 4 | Light volume/chunk | 8,192 B | 8,192 B | **PASS** |
| 5 | Probe grid/chunk | 6,144 B (512 probes) | 6,144 B | **PASS** |
| 6 | Mesh buffer/chunk | p95 2,519 KiB archived reference (production 3,650) | 2,400 KiB | **FAIL** (archived −119.4; production −1,250) |
| 7 | Edit latency | p95 8.136 µs | 8.00 ms | **PASS** |
| 8 | Probe update | p95 765.844 µs | 4.00 ms | **PASS** |
| 9 | Culling granularity | p95 0.00% over 100 poses | 15% | **PASS** |
| 10 | Safety factor | 4,913 MiB | 4,913 MiB | **PASS** |
| 11 | Measurement dataset | `m2-128` (synthetic) | real populated save | **NOT MEASURED** |
| 12 | Fallback trigger | rule 3 (128³ memory row) | pre-registration §4 | **NOT MEASURED** |

The measured p95 total is **4.19 MiB/chunk**: DAG 1,100 KiB, v1 pool 658 KiB,
light 8 KiB, probes 6 KiB, mesh 2,519 KiB (archived reference; production
3,650). The node row uses the per-chunk
maximum, 138,636 nodes, because the pre-registered cap is a hard per-chunk
limit rather than a distribution. The optimistic projection over 805
chunks is **3.29 GiB** archived (production 4.16) — 0.09 GiB over the 3.20 GiB
target but 1.50 GiB under the archived hard fail; production is 987 MiB over
the target and 650 MiB under the hard fail.

## 3. The mesh row is the open condition

The gate estimates mesh bytes as greedy quads × 76 B (the frozen ADR-010
indexed-quad size). The **production AO** p95 is **3,650 KiB**, 52.1% over the
2.4 MiB cap; the archived run's 2,519 KiB is the no-AO reference. Two facts make
this a real miss rather than a rounding artifact:

- the estimate is the **M1-31 reference greedy** mesh, which merges without AO
  and also counts chunk-boundary faces without an apron; the AO split is expected
  to dominate the boundary overcount on this fixture, so the production mesh is
  expected to be larger, but the estimate is neither a proven lower nor a proven
  upper bound;
- M2-27 confirms the production vertex stream is exactly 76 B per quad, so there
  is no hidden per-quad saving between the estimate and the real buffer.

Mesh compression is therefore the first rule-3 lever and the first M3 item. Until
it lands, the 128³ Minimum edge is a **measured candidate**, not a confirmed
profile. The pre-registered fallback remains 64³ Minimum with the rule-3 order:
mesh compression, then near radius, then vertical shell.

## 4. Culling granularity (row 9) is measured — but non-discriminating

`gate granularity` runs the M1-34 reference over 100 poses on the frozen save:
frustum traversal at block resolution against the M1-33 occlusion oracle. The
measured p95 is **0.00%** traversed-but-invisible, with zero missed blocks. The
row is therefore formally **PASS**, but the result is non-discriminating on this
fixture and should not be read as "the culler has 0% waste".

The result needs its caveat stated plainly: the reference fill is conservative,
so a block that is only partly filled is passable and the fill reaches almost
everything. On this terrain save the oracle marks every traversed block visible,
which is the best case for the row. The synthetic two-chunk M1-34 test remains
the sensitivity check that exercises non-zero ratios; the save-level number is a
conservative lower bound on the granularity cost, not a proof that a production
occluder will stay under 15%. Row 9 is recorded as **measured**, not as proof of
a zero-cost culler, and the 128³ conclusion does not rest on it.

## 5. Gate row 11 is carried

The fixture is `m2-128`, the same deterministic stand-in generator as
`m1-representative`, frozen at the 128³ edge over a 3 × 1 × 3 chunk box (x, y, z). It is
not a gameplay save and not `docs/worldgen-design.md`; its manifest says so. The
dataset row therefore stays **NOT MEASURED**.

The M2 exit records this as a carried item, not a pass. When the first real save
exists, re-freeze the fixture against it, rerun `gate report --profile minimum`
at the same edge, and only then close row 11.

## 6. Throughput and content numbers

- **Chunk build throughput** (M2-28): 210.4 chunks/s at 64³ and **34.3 chunks/s
  at 128³**, single-threaded, both above the 20/s budget.
- **Viewer** (M2-27): 2,208 production quads covering 8,226 naive faces (73.2%
  fewer), 167,808 B mesh + 1,024 B light + 776 B probes; 166–172 fps on llvmpipe
  at 1920×1080.
- **Scrub** (M2-24): the gate edits report includes the production path; see
  [`gate.md`](../benchmarks/runs/20261031T152718Z-m2-gate-128/gate.md) for the
  measured terms.
- **Probes** (M2-26): the ray-test remains behind the off-by-default flag; the
  flood fill stays the default occluder.

## 7. Reproduce

```sh
# 128³ fixture: rebuild and verify the frozen bytes.
cargo run --release -p voxel-bench --features chunk128 -- fixture verify \
  --manifest benchmarks/fixtures/m2-128/manifest.json

# The measured Minimum gate and the projection.
cargo run --release -p voxel-bench --features chunk128 -- \
  gate report --manifest benchmarks/fixtures/m2-128/manifest.json --profile minimum
cargo run --release -p voxel-bench --features chunk128 -- \
  gate granularity --manifest benchmarks/fixtures/m2-128/manifest.json
cargo run --release -p voxel-bench --features chunk128 -- \
  gate scaleup --manifest benchmarks/fixtures/m2-128/manifest.json --profile minimum

# The frozen mesh goldens run at both edges in CI and can be uploaded as artifacts.
cargo test -p voxel-mesh --test golden
cargo test -p voxel-mesh --features chunk128 --test golden
```
