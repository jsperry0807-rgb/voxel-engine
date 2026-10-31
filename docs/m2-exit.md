# M2 exit: the measured 128³ Minimum run

**Date:** 2026-10-31
**Milestone:** M2 (production content pipeline), task M2-30
**Run:** [`benchmarks/runs/20261031T150052Z-m2-gate-128`](../benchmarks/runs/20261031T150052Z-m2-gate-128)
**Fixture:** [`benchmarks/fixtures/m2-128`](../benchmarks/fixtures/m2-128) — 9 chunks, 128³ edge, synthetic and provisional
**Profile:** `minimum` (gated)
**Gate report:** [`gate.md`](../benchmarks/runs/20261031T150052Z-m2-gate-128/gate.md) · [`gate.json`](../benchmarks/runs/20261031T150052Z-m2-gate-128/gate.json)
**Granularity:** [`granularity.md`](../benchmarks/runs/20261031T150052Z-m2-gate-128/granularity.md)
**Scale-up:** [`scaleup.md`](../benchmarks/runs/20261031T150052Z-m2-gate-128/scaleup.md)

---

## 1. Verdict

**128³ Minimum is measured, but not confirmed.** The measured VRAM projection
clears the 4.80 GiB hard-fail line at **3.29 GiB**, and edit, probe, node, light
and probe-grid rows pass. The **mesh-buffer row misses by 119.4 KiB** (2,519 KiB
measured against the 2,400 KiB cap), so the pre-registered rule-3 lever — mesh
compression — is still open. Gate row 9 (culling granularity) is now measured
and passes. Gate row 11 (real populated save) is **carried**: no real save exists
before M2.5/worldgen or gameplay, so the dataset row remains not measured.

This is an engineering exit with a measured open condition, not a green light
for 128³ Default or a confirmed 128³ Minimum.

## 2. The twelve rows

| # | Row | Measured | Limit | Result |
|---|---|---|---:|---|
| 1 | VRAM budget | 3.29 GiB (p95 4.19 MiB × 805 chunks) | 4.80 GiB hard fail | **PASS** |
| 2 | Loaded chunks | N64 6,440; N128 805 | equal-volume identity | **PASS** |
| 3 | SVDAG nodes/chunk | p95 140,786 | 1,500,000 | **PASS** |
| 4 | Light volume/chunk | 8,192 B | 8,192 B | **PASS** |
| 5 | Probe grid/chunk | 6,144 B (512 probes) | 6,144 B | **PASS** |
| 6 | Mesh buffer/chunk | p95 2,519 KiB | 2,400 KiB | **FAIL** (−119.4 KiB) |
| 7 | Edit latency | p95 5.290 µs | 8.00 ms | **PASS** |
| 8 | Probe update | p95 739.384 µs | 4.00 ms | **PASS** |
| 9 | Culling granularity | p95 0.00% over 100 poses | 15% | **PASS** |
| 10 | Safety factor | 4,913 MiB | 4,913 MiB | **PASS** |
| 11 | Measurement dataset | `m2-128` (synthetic) | real populated save | **NOT MEASURED** |
| 12 | Fallback trigger | rule 2 preview | pre-registration §4 | **NOT MEASURED** |

The measured p95 total is **4.19 MiB/chunk**: DAG 1,076 KiB, v1 pool 658 KiB,
light 8 KiB, probes 6 KiB, mesh 2,519 KiB. The optimistic projection over 805
chunks is **3.29 GiB** — 0.09 GiB over the 3.20 GiB target but 1.50 GiB under
the hard-fail line.

## 3. The mesh row is the open condition

The gate estimates mesh bytes as greedy quads × 76 B (the frozen ADR-010
indexed-quad size). The measured p95 is 2,519 KiB, 5.0% over the 2.4 MiB cap. Two
facts make this a real miss rather than a rounding artifact:

- the estimate is the **M1-31 reference greedy** mesh, which merges without AO;
  the production M2 AO merge key splits more quads, so the production mesh is at
  least this size;
- M2-27 confirms the production vertex stream is exactly 76 B per quad, so there
  is no hidden per-quad saving between the estimate and the real buffer.

Mesh compression is therefore the first rule-3 lever and the first M3 item. Until
it lands, the 128³ Minimum edge is a **measured candidate**, not a confirmed
profile. The pre-registered fallback remains 64³ Minimum with the rule-3 order:
mesh compression, then near radius, then vertical shell.

## 4. Culling granularity (row 9) is closed

`gate granularity` runs the M1-34 reference over 100 poses on the frozen save:
frustum traversal at block resolution against the M1-33 occlusion oracle. The
measured p95 is **0.00%** traversed-but-invisible, with zero missed blocks.

The result needs its caveat stated plainly: the reference fill is conservative,
so a block that is only partly filled is passable and the fill reaches almost
everything. On this terrain save the oracle marks every traversed block visible,
which is the best case for the row. The synthetic two-chunk M1-34 test remains
the sensitivity check that exercises non-zero ratios; the save-level number is a
conservative lower bound on the granularity cost, not a proof that a production
occluder will stay under 15%.

## 5. Gate row 11 is carried

The fixture is `m2-128`, the same deterministic stand-in generator as
`m1-representative`, frozen at the 128³ edge over a 3 × 3 × 1 chunk box. It is
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
  [`gate.md`](../benchmarks/runs/20261031T150052Z-m2-gate-128/gate.md) for the
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
