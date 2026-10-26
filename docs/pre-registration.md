# M1 Pre-Registration

**Status:** FROZEN 2026-10-26, before any M1 measurement
**Machine:** [`reference-machine.md`](reference-machine.md) — RTX 4060, 8,188 MiB, 1080p
**Targets:** [`performance-targets.md`](performance-targets.md)
**Rule:** this file is not edited after freezing. Corrections go in a **dated
appendix** at the bottom; the original values stay visible.

---

## 1. Profiles measured

| Profile | Resolution | Far clip | Near radius | Vertical shell | LOD rings | Detail |
|---|---|---:|---:|---:|---|---|
| **Minimum** | 1080p | 8 km | 64 m | ±16 m | 64 / 128 / 256 m | reduced |
| **Default** | 1080p | 16 km | 128 m | ±16 m | 128 / 256 / 512 m | full |

High (32 km / 256 m) is not gated on this machine. "60 fps" means 60 average and 45
1% low over the scripted flight and cave walk.

## 2. Residency model

```
N(edge, R, V) = ceil(pi * (R / edge)^2) * ceil(V / edge)
```

where `edge` is the chunk edge in metres, `R` the near voxel radius, `V` the full
vertical shell (`±V/2`), and `N` the number of near voxel chunks. The far field is a
mesh and is not counted here.

| Edge | Profile | R | V | **N** | Per-chunk range | Implied voxel memory |
|---|---|---:|---:|---:|---|---:|
| 128³ (8 m) | Minimum | 64 m | 32 m | **804** | 1.2–3.2 MiB | 0.94–2.5 GiB |
| 128³ (8 m) | Default | 128 m | 32 m | **3,216** | 1.2–3.2 MiB | 3.8–9.8 GiB |
| 64³ (4 m) | Minimum | 64 m | 32 m | **6,432** | 0.24–0.70 MiB | 1.5–4.3 GiB |
| 64³ (4 m) | Default | 128 m | 32 m | **25,736** | 0.24–0.70 MiB | 6.0–17.2 GiB |

The chunk count is measured on the fixture in M1; this table is the pre-registered
prediction, not a result.

## 3. The table

| Parameter | Pre-registered value | Basis | Implied / check |
|---|---|---|---|
| VRAM budget for voxel data | **40% of 8,188 MiB = 3,275 MiB**; hard fail at 60% = **4,913 MiB** | Fraction survives hardware changes | Gate is measured × 1.5 |
| Target loaded chunk count | Per profile and edge, table §2 | Using one count for both edges makes the report meaningless | Measured on the fixture |
| SVDAG node budget/chunk | 64³ ≤ **200 k**; 128³ ≤ **1.5 M** unique nodes | 8 B/node × 2 roots = 16 B/node | 3.2 MB / 24 MB hard caps, not p95 |
| Light volume/chunk | 64³: **64 cells = 1 KiB**; 128³: **512 cells = 8 KiB** | 16 B/cell, per-block | Fixed by ADR-004 |
| Probe grid/chunk | 64³: **64 probes = 768 B**; 128³: **512 probes = 6 KiB** | 12 B SH4 RGB, near LOD | Fixed by ADR-005 |
| Mesh buffer/chunk | 64³: **150–600 KB**; 128³: **600 KB–2.4 MB** | 76 B indexed quad, exposed faces | The dominant per-chunk cost |
| Edit latency | p95 ≤ **8 ms** | Consolidate + upload + fence + flip + dirty light | On the real save |
| Probe-update budget | p95 ≤ **4 ms** per edited chunk | Keeps probes off the critical path | On the real save |
| Culling granularity | ≤ **15%** traversed-but-invisible at chunk boundaries @ max view | 64³ should pass; 128³ is the question | Reference traversal at block resolution |
| Safety factor | **1.5×** on the memory budget | Chunky content, fragmentation, driver reserve | Gate uses 4,913 MiB |
| Measurement dataset | **Real populated save**; median, p95, max per chunk | Synthetic chunks lie | Provisional fixture until the first real save |
| Fallback trigger | See §4 | Pre-registered | No renegotiation inside M1 |

## 4. Fallback rules

The three failures are **not interchangeable**:

1. **Edit latency p95 > 8 ms, probe update p95 > 4 ms, or granularity > 15% at 64³**
   → **128³ is dead.** These forces get worse with chunk volume. Stay at 64³ and
   record the numbers.
2. **Memory fails at 64³** → **do not automatically stay at 64³.** The 64³ profile
   holds 8× the chunks at roughly 1/8 the per-chunk volume, but meshes and overlays
   scale with area and chunk count, so 64³ can lose the memory comparison. Run the
   §2 analytic projection to 128³; if 128³ clears, the failure is an argument for
   the upgrade, not against it.
3. **Memory fails at 128³** → reduce the profile in the sacrifice order: mesh
   compression, then near radius, then vertical shell. Never shrink the far clip
   below 8 km to fix memory.

The 64³ measurement is still mandatory: it is the only way to test rules 1 and to
calibrate the 128³ projection.

## 5. Known risk at freeze time

On the pre-registered ranges, the **Default profile at 128³ is 3.8–9.8 GiB** against
a 3,275 MiB target and a 4,913 MiB fail line. It may fail on memory. The first lever
is mesh size, not the world: 76 B/quads at 2.4 MB/chunk is the upper bound, not the
expected value. M1 measures the real distribution before anything is cut.

## 6. Amendment rule

- Append a dated section: what changed, why, the old value, the new value.
- Never edit or delete a frozen row.
- An amendment made after a gate run invalidates that run; re-run and re-record.
