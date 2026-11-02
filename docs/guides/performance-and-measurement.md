# Performance and measurement

The engine is judged on **measured** numbers, not on how modern the design
sounds. This guide explains the rules, the machine, the gates, and how to read a
report without getting fooled by a good-looking average.

The normative documents are
[`performance-targets.md`](../specs/performance-targets.md),
[`pre-registration.md`](../specs/pre-registration.md) and
[`reference-machine.md`](../specs/reference-machine.md).

## 1. Two numbers that decide everything

### 16 ms

**No operation may exceed 16 ms.** Edit latency, probe update, a hitch, a
generation step, a frame — all of it is capped at 16 ms. A 60 fps frame is
16.6 ms, but the project's engineering limit is deliberately tighter.

The original pre-registration used 8 ms for edits and 4 ms for probes. A dated
appendix on 2026-11-02 raised both to 16 ms. The frozen rows stay visible; older
runs keep their old verdicts.

### 40% of VRAM

Voxel data may use **40% of the device's VRAM**:

| | Value |
|---|---:|
| Reference GPU VRAM | 8,188 MiB |
| Target (40%) | **3,275 MiB (3.20 GiB)** |
| Hard fail (60%) | **4,913 MiB (4.80 GiB)** |

Voxel data means the tree, meshes, overlays, light, probes, far field and
culling buffers. It excludes the swapchain, TAA history and the OS. The gate
also applies a **1.5× safety factor**, so the real comparison is
`p95 per chunk × N ≤ 4,913 MiB`, not `≤ 3,275 MiB`.

## 2. The reference machine

Every gate number belongs to one machine:

| Part | Value |
|---|---|
| CPU | AMD Ryzen 5 7600, 6 cores / 12 threads |
| GPU | NVIDIA RTX 4060, **8,188 MiB** |
| RAM | 32 GB |
| OS | Linux (primary) |
| Display | 1920 × 1080 |

A run on other hardware is not a gate run. The Radeon iGPU appears in several
reports, but as a **second data point**, not as the gate.

## 3. Pre-registration: write the test before you take it

The M1 pre-registration was frozen on 2026-10-26, **before any M1
measurement**. It names the profiles, the rows, the limits, the fallback rules
and the amendment rule.

The point is to stop the project from moving the finish line after seeing the
result. If a row fails, the response is a pre-agreed lever, not a new threshold.

The twelve gate rows cover:

1. VRAM budget
2. Loaded chunk count at the profile
3. SVDAG nodes per chunk
4. Light volume per chunk
5. Probe grid per chunk
6. Mesh buffer per chunk
7. Edit latency
8. Probe update
9. Culling granularity
10. Safety factor
11. Measurement dataset
12. Fallback trigger

Row 6 is the one that failed M2: the production mesh was 3,650 KiB p95 against a
2,400 KiB cap. The pre-agreed lever was **mesh compression**, which landed in
M3 as the 44-byte-per-quad packed vertex and brought the row to **2,115 KiB**.

Row 11 is still **not measured**. The fixture is synthetic — it exercises the
shape of a save, not a real gameplay save — so the row cannot close until a real
save exists.

## 4. Profiles and residency

Two profiles are gated:

| | Minimum | Default |
|---|---|---|
| Resolution | 1080p | 1080p |
| Far clip | 8 km | 16 km |
| Near voxel radius | 64 m | 128 m |
| Vertical shell | ±16 m | ±16 m |
| LOD rings | 64 / 128 / 256 m | 128 / 256 / 512 m |
| Detail | reduced | full |

“High” (32 km, 256 m near) exists but is not gated on this machine.

The chunk count is not guessed. The residency model is:

```
N(edge, R, V) = ceil(pi * (R / edge)^2) * ceil(V / edge)
```

At 128³ the Minimum profile is 804 chunks; the gate report uses **805**, which
includes the centre. The Default profile is **3,216** chunks. This is why the
same per-chunk cost can be fine at Minimum and catastrophic at Default.

## 5. How to read a report

### p50, p95, p99

A percentile says “this fraction of samples are at or below this value”. If the
p95 chunk memory is 5.29 MiB, 95% of chunks are at or below 5.29 MiB; the
remaining 5% can be larger. The gate uses p95 because a single lucky chunk must
not hide a heavy tail.

### The 1% low

The 1% low is the **average frame rate of the slowest 1% of frames**, inverted.
It deliberately includes the worst frame, unlike a statistical p99 over 100
samples, which is the 99th value and can exclude it.

A 60 Hz claim needs the 99th-percentile frame to fit 16.6 ms, which is the same
as a 1% low of at least 60 fps. An average of 60 with a 1% low of 45 is not a
pass.

### Per chunk vs total

The gate multiplies:

```
p95 per-chunk memory × target chunk count ≤ 4,913 MiB
```

The per-chunk number is useful for comparing chunks; the total is what decides
whether a profile fits. Always check which one a sentence is quoting.

### Archived reference vs production

The M2 exit is the cautionary tale. Its archived row 6 quoted **2,519 KiB**,
which was the *reference* mesh without AO. The production AO mesh measured
**3,650 KiB** — a 52% miss rather than a 5% miss. The verdict (FAIL) was
unchanged, but the severity was not. The correction is recorded as a dated
erratum in [`m2-exit.md`](../milestones/m2/m2-exit.md); the archived run stays
append-only.

When you read a number, ask: **which build, which profile, which dataset, and is
it archived or production?**

## 6. What the current numbers say

| Claim | Number | Source |
|---|---|---|
| M2 production mesh | 3,650 KiB p95 vs 2,400 cap — **FAIL** | [M2 exit](../milestones/m2/m2-exit.md) |
| M3 packed mesh | 2,115 KiB p95 — **PASS** | [M3-00](../milestones/m3/m3-packed-8b.md) |
| M2 VRAM projection | 4.16 GiB vs 4.80 hard fail — **PASS** | [M2 exit](../milestones/m2/m2-exit.md) |
| M3 render scene VRAM | 107.5 MiB = 3.4% of target | [M3-11](../milestones/m3/m3-11-memory.md) |
| M3 frame | 5.5 ms (4060) / 13.7 ms (iGPU) | [M3-07](../milestones/m3/m3-07-frame-breakdown.md) |
| M3 60 Hz | 192–202 fps / 104–113 1% low (4060); 70–75 / 56.6–61.2 (iGPU) | [M3-10](../milestones/m3/m3-10-60hz.md) |
| M3 upload | 512 KiB/frame, p99 1.6–3.1 ms | [M3-09](../milestones/m3/m3-09-hitch-test.md) |

Two things these numbers do **not** say:

- **“128³ is green everywhere.”** The storage projection is 2.98 GiB at the
  minimum view, but 11.93 GiB at default and 95.4 GiB at high. 128³ is green at
  the gated minimum view only.
- **“The renderer is cheap, so storage is cheap.”** The M3 scene holds 107.5 MiB
  because it uploads no sparse tree and no overlay. The storage gate’s row 1 is
  a different model and remains the binding one.

## 7. Reproducing a run

Milestone numbers go through the archive script, never an ad-hoc command:

```sh
LABEL=m3-gate-128 GATE=1 FEATURES=chunk128 \
  GATE_MANIFEST=benchmarks/fixtures/m2-128/manifest.json \
  GATE_PROFILE=minimum scripts/bench.sh
```

The viewer writes its own frame trace for 60 Hz claims:

```sh
cargo build --release -p voxel-viewer --features chunk128
VOXEL_FRAMETIME=target/frametime.json \
VOXEL_FRAMETIME_EYE=14,6,20 VOXEL_FRAMETIME_TARGET=2,1,2 \
  ./target/release/voxel-viewer
```

Every run lands in [`benchmarks/HISTORY.md`](../../benchmarks/HISTORY.md). A
number quoted without a run ID is a comment, not evidence.

## 8. CI policy

- CI checks correctness, formatting, clippy, shaders and both chunk edges.
- Wall-clock microbenchmarks stay **advisory permanently**: they are
  machine-specific and this is a solo project.
- The hard CI gates are memory over the budget, a p99 hitch over 16 ms, and
  generation p95 over 16 ms — but only after the machine has ten stable runs.
- A regression over 10% on a stable metric fails review.

See [§6 of the performance targets](../specs/performance-targets.md) for the
exact policy.

## Where to go deeper

- [Performance targets](../specs/performance-targets.md) — budgets and the
  sacrifice ladder.
- [Pre-registration](../specs/pre-registration.md) — the frozen table and its
  amendments.
- [Reference machine](../specs/reference-machine.md) — the hardware and profiles.
- [`benchmarks/HISTORY.md`](../../benchmarks/HISTORY.md) — every recorded run.
- [Rendering and streaming](rendering-and-streaming.md) — where the frame time
  actually goes.
