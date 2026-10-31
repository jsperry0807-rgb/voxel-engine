# Mesh compression plan (M3-00)

**Status:** Proposed — 2026-10-31
**Trigger:** M2 exit, gate row 6. The production AO mesh exceeds the 128³ cap.
**Goal:** bring the measured 128³ mesh p95 under the 2,400 KiB cap, restore the
128³ Minimum memory projection, and preserve the current visual contract.

---

## 1. The real gap is 52%, not 5%

The M2 exit recorded row 6 as **2,519 KiB p95** and a **119.4 KiB** miss. That is
the **M1-31 no-AO reference mesh**, not the production mesh. The production path
(`for_each_block` + `greedy_block` + `mesh_block`) over the same frozen saves
gives:

| Fixture | Reference p95 | Production AO p95 | Ratio | Cap | Miss |
|---|---:|---:|---:|---:|---:|
| 128³ `m2-128` (9 chunks) | 2,519 KiB | **3,645 KiB** | 1.447× | 2,400 KiB | **1,245 KiB (51.9%)** |
| 64³ `m1-representative` (72 chunks) | 753 KiB | **924 KiB** | 1.227× | 600 KiB | **324 KiB (54.0%)** |

The 128³ production total is 346,081 quads / 26,302,156 B over 9 chunks. The
p95 chunk is 49,112 quads; the mean is 38,454, **28% below the p95**. The gate
uses p95 by pre-registration, so its projection is deliberately conservative.
With n = 9, the nearest-rank p95 is also the maximum, a second reason to treat
3,645 KiB as an upper-bound-flavoured number from a single archetype.

Two consequences:

1. **Row 1 is also wrong today.** `MemoryComponents::total` includes the mesh
   estimate, so 3.29 GiB is built on `1,772 + 2,519` KiB. With the production
   mesh it is `(1,772 + 3,645) × 805 = 4.16 GiB`: under the 4.80 GiB hard fail,
   but over the 3.20 GiB target by ~0.96 GiB.
2. **Row 6 is a per-chunk cap.** Rule 3's "near radius, then vertical shell"
   moves rows 1/2, not row 6. AO-tolerant merging is possible but rejected on
   visual-parity grounds, so the only lever that preserves the current look is
   the mesh encoding itself.

**Immediate record:** add a dated erratum to [`m2-exit.md`](m2-exit.md) §3 and
update the repeated numbers in `task-list.md`, `m1-gate-decision.md` and the
roadmap. Keep the archived M2 run append-only.

---

## 2. Primary: instanced quads (8 B/quad)

### 2.1 Record layout and spare-bit allocation

One 8 B record per quad:

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `origin.x/y/z` | `u8` each, chunk-local, `0..=CHUNK_DIM` |
| 3 | `material_lo` | low 8 material bits (atlas slot today) |
| 4–7 | `packed` | `u32`, see below |

`packed` bit allocation:

| Bits | Field |
|---:|---|
| 0–2 | face (3 bits) |
| 3–9 | `extent_u` (7 bits, size−1) |
| 10–16 | `extent_v` (7 bits, size−1) |
| 17–24 | AO (4 corners × 2 bits) |
| 25 | AO diagonal-flip flag (reserved 0 today) |
| 26–31 | `material_hi` (6 bits) |

Material is therefore **14 bits** with the flip flag reserved (15 bits if the
flag is never needed). The spare-bit budget is not counted twice. The current
atlas is 16 × 16 tiles (256 slots), so 8 bits are enough today; the 14-bit field
leaves room without changing the 8 B record.

The record is **25 bits in a 4 B word**, with the remaining 7 bits allocated
above. Extents are stored as `size − 1`; 7 bits cover up to 128, so the format
does not depend on the current 16³ block cap.

### 2.2 Draw contract

- No index buffer. One draw per meshlet: **6 vertices × `quad_count`
  instances**, `instance_index` selects the record within the meshlet,
  `vertex_index` selects the corner.
- **Do not rely on `first_instance`.** The spike passes the meshlet's base quad
  as a uniform or stores `base_quad` in the meshlet record and computes
  `quad = base_quad + instance_index` in the shader. M3-05's indirect path must
  either keep the base in the record or verify `INDIRECT_FIRST_INSTANCE` on the
  target adapters; the record-based base is the default because the feature is
  not universal.
- The vertex shader maps `vertex_index` 0..5 to the current index pattern
  `[0,1,2,0,2,3]`, reconstructs the four corners from
  `origin + face + extent`, and selects the AO corner. The quad diagonal stays
  the current fixed `(0,1,2),(0,2,3)`; the mesher does not flip it today. The
  reserved flip bit is the hook if an AO-aware flip is added later.
- This removes the 12 B/quad index buffer. It is **not** an index-range fix:
  ADR-010 already splits a chunk into meshlets with `base_vertex` for the
  pathological >16,384-quad block, so the 49k-quad 128³ chunk is a meshlet
  count, not an index problem. The saving is bytes, not correctness.

### 2.3 Projection arithmetic

The per-chunk components are from the M1/M2 gate reports (DAG, v1 pool, light,
probes) plus the measured production mesh. `n` is the loaded-chunk count for the
profile.

| Edge | DAG | v1 pool | Light | Probes | Mesh | Total/chunk | × n | Projection |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 128³ instanced | 1,099.9 | 657.7 | 8.0 | 6.0 | 393.0 | 2,164.6 KiB | 805 | **1.66 GiB** |
| 128³ 8 B vertex | 1,099.9 | 657.7 | 8.0 | 6.0 | 2,119.0 | 3,890.6 KiB | 805 | **2.99 GiB** |
| 64³ instanced | 253.2 | 164.0 | 1.0 | 0.75 | 98.4 | 517.4 KiB | 6,440 | **3.18 GiB** |
| 64³ instanced, no v1 pool | 253.2 | 0 | 1.0 | 0.75 | 98.4 | 353.4 KiB | 6,440 | **2.17 GiB** |
| 64³ 8 B vertex | 253.2 | 164.0 | 1.0 | 0.75 | 536.0 | 955.0 KiB | 6,440 | **5.86 GiB** |

Sources: DAG/light/probes and the v1 pool from
[`m1-gate-decision.md`](m1-gate-decision.md) §3 and the 2026-10-29 amendment;
the mesh numbers are measured. The 64³ instanced total is ~3.18 GiB **before**
the M6-09a v1-pool removal, i.e. essentially no margin against the 3.20 GiB
target; it is ~2.17 GiB after. The 8 B vertex fallback does not re-open 64³.

The 128³ instanced projection clears both the mesh cap and the 3.20 GiB target
with a large margin. The 128³ 8 B fallback clears the hard fail and the target,
but with a thin row-6 margin (§3).

### 2.4 Parity and performance gates

The spike has three separate gates; none of them substitutes for another.

**Gate A — CPU exactness (encoder/decoder).** Decode every record into its four
corners (position, material, face, AO) and assert they equal the `Mesh` vertices
from `mesh_block` **exactly**, for every fixture block at both edges. Positions
are integers, exactly representable in `f32`.

**Gate B — shader-output exactness (the actual corner/winding risk).** Add a
compute entry point to the same WGSL module that calls the same reconstruction
helper and writes the reconstructed `(position, face, material, AO)` for every
vertex to a storage buffer. Read it back and compare **positions exactly** to
the `Mesh` vertices; compare AO and colour with the documented tolerance. The
scene must cover all six faces, every extent (including repeated extents), all
four AO levels, and both material extremes. A single-chunk image diff is not
enough coverage.

**Gate C — frame time on real GPUs.** llvmpipe is a CPU rasterizer and is
**not admissible** for the performance decision (functional parity only). Run
the spike on at least:

- one discrete GPU (the RTX 4060 reference machine), and
- one integrated GPU (e.g. Intel UHD 770 or AMD Radeon 780M).

Report **vertex-bound time separately from pixel time**: render a vertex-heavy
pass into a 1 × 1 viewport with no fragment output for the vertex-bound number,
and the full 1920 × 1080 frame for pixel time; report the difference. Compare
the instanced path with the current indexed path on each GPU. Threshold: no
more than 10% regression in vertex-bound or full-frame time on either GPU, and
the reference discrete still meets 60 Hz at 1080p. The integrated GPU is not
expected to hit 60 Hz; it is there to expose vertex-bound regressions.

If the integrated-GPU runner is unavailable, **Gate C is open and the spike is
not passed**; CPU/shader exactness and functional parity can still be recorded,
but the format decision waits. The local dev host has llvmpipe only, so this
plan cannot close Gate C by itself.

**Gate D — image smoke test.** Render the same scene through the current 16 B
indexed path and the instanced path; allow a documented **≤1 LSB** tolerance.
Division and unorm8 fetch are not bit-identical across GPUs/compilers, and FMA
contraction can differ between programs. `f32(level * 85u) / 255.0` is the
right input, but it is not a bit-exactness guarantee. Gate B is the exact one;
Gate D catches gross regressions.

### 2.5 Cost and risk

- GPU-side corner/face reconstruction and winding are a correctness risk the
  8 B vertex does not have; Gates B/C/D are the mitigation.
- The pre-registered row 6 range is 600 KB–2.4 MB at 128³. 393 KiB is below the
  600 KB lower bound. That lower bound is a prediction from the 76 B/quad
  model, not a hard floor; the gate note already says a lower estimate is not a
  failure. The dated pre-registration appendix must re-base the lower bound and
  keep the cap unchanged.
- The 14-bit material field is reserved, not used; the current atlas needs 8
  bits.

---

## 3. Fallback: 8 B packed vertex (44 B/quad)

Use this only if the instanced spike fails Gates A/B/D or Gate C, after the
single retry in §7. It is **not** a schedule shortcut: the saved time is about
1 d against the instanced path, and the spike is already spent.

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `local.x/y/z` | `u8` each, `0..=CHUNK_DIM` |
| 3 | spare | zero |
| 4–5 | `material` | `u16`; no 255 ceiling, no truncation |
| 6–7 | `packed` | `u16`: face 3 bits, AO 4 × 2 bits, 5 spare/version bits |

A `u16` material fits in the same 8 B stride because the reserved bytes are
used. It removes the silent truncation hazard at the encoder;
`resolve_materials` is still required for the atlas tile, but a missed resolve
is a wrong tile rather than a lost id.

- Quad: `4 × 8 + 6 × 2 = 44 B` (42.1% smaller).
- 128³: ~2,119 KiB p95, **88.3% of cap** (2,160 KiB line, 41 KiB buffer).
- 64³: ~536 KiB, **89.3% of cap** (540 KiB line, **4 KiB buffer**).
- Row 1: `3,890.6 × 805 = 2.99 GiB`, under 3.20.
- Of the two candidates, this is the one that keeps the 12 B/quad index buffer.
  That is a simplicity trade, not a requirement.
- The fallback has **no real-save headroom**: the 64³ case is 4 KiB inside the
  90% line, and the 128³ case is 41 KiB inside. Treat it as a synthetic-fixture
  stopgap that is expected to trigger the third exit on a real save, not as a
  comfortable pass.
- Keep the logical `Vertex` at 16 B for goldens/determinism; add a `GpuVertex`
  (8 B) for upload. The M1.5 naive path keeps its own 16 B layout so the
  screenshot example and its goldens do not churn.

---

## 4. Options considered

| Option | Bytes/quad | 128³ p95 | Share of 2,400 KiB cap | Verdict |
|---|---:|---:|---:|---|
| **Instanced quads** | **8 B** | **~393 KiB** | **16.4%** | **Primary** |
| 8 B packed vertex | 44 B | ~2,119 KiB | 88.3% | Fallback; no real-save headroom |
| Per-quad constants + 3 B positions | ~28 B | ~1,343 KiB | 56.0% | Dominated by instanced |
| Vertex dedup (unmeasured) | 32–40 B | ~1,500–1,900 KiB | 62–79% | Only after a 0.5 d measurement |
| 12 B vertex | 60 B | ~2,877 KiB | 120% | Reject: still over cap |
| Index compression only | 64 B | ~3,069 KiB | 128% | Reject: not enough |
| More aggressive AO merge | fewer quads | fewer | — | Reject: visual artifacts |

Percentages use the 2,400 KiB cap and the production AO p95 of 3,645 KiB. The
table is the rejected-alternatives record ADR-014 needs.

---

## 5. Measurement fix (do this first)

The gate must measure the real upload path, not a quad-count proxy:

- In `crates/bench/src/gate/memory.rs`, after expanding the chunk, run
  `for_each_block` + `greedy_block` + `mesh_block`, and call the encoder's
  byte-length method (`Mesh::gpu_byte_len(format)` or `GpuMesh::from(&mesh)`).
  Cost is ~0.3 s for 9 chunks at 128³.
- `mesh` component = GPU bytes + meshlet records + block table. Add
  `mesh_reference = reference_quads × 76` as an informational component that is
  **not** in `total`; the gate keeps the M1 historical number without mixing it
  into row 1.
- Row 6 string: `"p95 X KiB (max Y); production AO / <format> (reference Z KiB × 76 B)"`.
- Add `MESH_BYTES_PER_QUAD` sourced from the encoder so the row cannot drift.
- Bump the memory and gate-report schemas; use `#[serde(default)]` on new
  fields so archived JSON still loads.
- Re-run 128³ and 64³; archive both; keep M1/M2 runs append-only; add the M2
  erratum and a new history entry.

---

## 6. Work plan and schedule

### Phase 0 — true baseline (1 d)

1. Add the production `mesh_block` measurement to `gate memory`.
2. Re-run both edges; archive.
3. Append the M2 erratum: production 3,645 KiB, miss 1,245 KiB, row 1 4.16 GiB.
4. Update the repeated numbers in the task list, M1 decision, roadmap and
   performance targets.

### Phase 1 — instanced-quad spike (1 d functional + hardware time)

- Implement the 8 B record encoder and CPU decoder; run Gate A.
- Implement the shared reconstruction helper and the compute readback; run Gate
  B over the full face/extent/AO coverage set.
- Run Gate D (≤1 LSB image smoke) on whatever adapter is local (llvmpipe is
  fine for functional coverage).
- Run Gate C on the discrete and integrated GPUs; record vertex-bound and pixel
  time separately. If the iGPU is unavailable, Gate C stays open.
- If Gates A/B/D and C pass, the spike becomes the implementation base. If a
  fixable class fails, fix once and re-run (one retry, §7). If it still fails,
  take the fallback.

### Phase 2 — instanced quads (3–4 d)

- Record encoder, meshlet records (`base_quad`, `quad_count`, block index),
  per-chunk record buffer.
- Shader reconstruction, fixed diagonal, AO corner selection; flip bit reserved.
- Viewer wiring: `ChunkMeshData` holds records; `chunk_mesh`/`chunk_render` use
  the instance layout; no index buffer.
- Naive/screenshot path stays on its own 16 B layout.
- Gate A/B/D tests committed and CI-gated; Gate C recorded from real hardware.
- Gate re-run at both edges; row 6 and row 1 pass; archive.

### Fallback path

8 B `GpuVertex`, 44 B/quad: codec 0.75 d, viewer 1 d, naive split 0.5 d, CPU +
shader + image parity 1 d, gate/re-run 1 d = **4.25–5 d after the spike**.

### Schedule: best, typical, worst

| Path | Composition | Cumulative |
|---|---|---:|
| Best case (instanced passes) | Phase 0 1 + spike 1 + instanced 3–4 | **5–6 d** |
| Typical (spike fails, fallback passes) | Phase 0 1 + spike 1 + fallback 4.25–5 | **6.25–7 d** |
| Worst (fallback misses 90%, third exit) | typical + dedup measurement/implementation or re-plan 1–3 | **7.25–10 d** |

M3's published budget is 9 d. The best case is a re-baseline to ~14–15 d; the
worst case is ~16–19 d. Quote both when scheduling, not just the best case.

---

## 7. Rule, retries and exits

**Rule (applied to both formats):** a format is accepted only if its
measured/projected p95 is **≤ 90% of the row-6 cap** (≥10% margin).

| Format | 128³ | 64³ |
|---|---:|---:|
| Instanced | 16.4% of cap (83.6% margin) | 16.3% (83.7%) |
| 8 B vertex | 88.3% (11.7% margin, 41 KiB buffer) | **89.3% (10.7% margin, 4 KiB buffer)** |

The 8 B fallback is technically inside the rule on the synthetic fixtures, but
it has **no real-save headroom** at either edge. It is a stopgap, not a
comfortable pass.

**Decision tree (one retry maximum):**

1. Spike attempt 1 passes Gates A/B/D and C → **instanced**.
2. Spike attempt 1 fails a fixable class → fix once, **retry attempt 2**. Pass
   → instanced. Fail → 3.
3. **8 B fallback.** If it measures ≤90% of cap on a re-run → accept as a
   stopgap, record that the real-save trigger is pending. If it measures >90%
   → 4.
4. **Third exit** (no “switch back to instanced”; the spike is already spent):
   - **4a. Vertex dedup**, but only after a **0.5 d measurement** on the
     fixtures proves the byte win; the 32–40 B/quad figure is unmeasured and
     needs an index cache that breaks the stateless 8 B stride. If the
     measurement does not clearly clear 90%, do not rely on it.
   - **4b. Re-plan row 6/cap** or accept another profile — an explicit ADR-002
     decision, not a silent widening.

The retry is capped at one so the spike cannot loop indefinitely. There is no
third “fix the spike” step separate from step 2.

**Real-save trigger:** if the first real save's row 11 p95 is above 90% of the
selected format's cap, re-open the format decision. Do not close row 11 on a
synthetic fixture.

---

## 8. Acceptance

- `cargo test -p voxel-mesh` (logical codec, mesh shape, AO, determinism,
  goldens at both edges) and `cargo test -p voxel-viewer` (Gate A/B/D).
- Gate A: decoded corners equal the `Mesh` vertices exactly.
- Gate B: shader-reconstructed positions equal the `Mesh` positions exactly;
  AO/colour within the documented tolerance; coverage includes all faces,
  extents and AO levels.
- Gate C: discrete + integrated GPUs; vertex-bound and pixel time reported;
  no more than 10% regression on either; reference discrete still 60 Hz at
  1080p.
- Gate D: RGBA difference ≤1 LSB.
- Gate 128³ Minimum: row 6 PASS with p95 ≤ 2,400 KiB and ≤90% of cap; row 1 ≤
  3.20 GiB.
- Gate 64³: row 6 PASS ≤ 600 KiB; record the total and the pool caveat.
- ADR-014 merged, pre-registration appendix added, M2 erratum recorded, CI
  goldens/parity job green.

---

## 9. Decisions requested

1. **Primary path:** instanced quads with the spike (recommended) or the 8 B
   vertex first. Both are under the same 10% rule; the 8 B fallback is a
   stopgap with no real-save headroom.
2. **Hardware access:** can the spike run on the RTX 4060 reference **and** an
   integrated GPU? Without the iGPU, Gate C is open and the spike cannot pass.
3. **Schedule:** re-baseline M3 to best 14–15 d / worst 16–19 d, or move
   M3-08/M3-12.
4. **Placement:** new `M3-00a`/`M3-00b` before `M3-01`.

---

## 10. Revision history

- **rev 1:** 8 B vertex primary; Phase 2 bit budget wrong; u16 material treated
  as a 1 B trade; 15% rule contradicted by an 11.7% recommendation.
- **rev 2:** instanced quads primary; fixed the 4 B packed word and u16
  fallback; introduced a parity spike; left a circular fallback and a 95%
  trigger.
- **rev 3:** one 10% rule; third exit; CPU exactness; ≤1 LSB GPU smoke.
- **rev 4 (current):** real-GPU Gate C with vertex-bound/pixel split; best/
  typical/worst schedule; 14-bit material allocation with the flip bit; shader
  output readback; 64³ fallback called out as 4 KiB inside the rule; arithmetic
  table; dedup marked unmeasured.
