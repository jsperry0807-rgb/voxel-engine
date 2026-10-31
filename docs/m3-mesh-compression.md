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
| 128³ `m2-128` (9 chunks) | 2,519 KiB | **3,650 KiB** | 1.449× | 2,400 KiB | **1,250 KiB (52.1%)** |
| 64³ `m1-representative` (72 chunks) | 753 KiB | **925 KiB** | 1.228× | 600 KiB | **325 KiB (54.2%)** |

The 128³ production payload is 346,081 quads / 26,302,156 B over 9 chunks;
the gate adds 16 B per non-empty block and the u16 table (3,650 KiB p95). The
p95 chunk is 49,112 quads; the mean is 38,454, **28% below the p95**. The gate
uses p95 by pre-registration, so its projection is deliberately conservative.
With n = 9, the nearest-rank p95 is also the maximum, a second reason to treat
3,650 KiB as an upper-bound-flavoured number from a single archetype.

Two consequences:

1. **Row 1 is also wrong today.** `MemoryComponents::total` includes the mesh
   estimate, so 3.29 GiB is built on `1,772 + 2,519` KiB. With the production
   mesh it is `(1,772 + 3,650) × 805 = 4.16 GiB`: under the 4.80 GiB hard fail,
   but over the 3.20 GiB target by ~0.96 GiB.
2. **Row 6 is a per-chunk cap.** Rule 3's "near radius, then vertical shell"
   moves rows 1/2, not row 6. AO-tolerant merging is possible but rejected on
   visual-parity grounds, so the only lever that preserves the current look is
   the mesh encoding itself.

**Immediate record:** add a dated erratum to [`m2-exit.md`](m2-exit.md) §3 and
update the repeated numbers in `task-list.md`, `m1-gate-decision.md` and the
roadmap. Keep the archived M2 run append-only.

---

## 2. Primary: instanced quads with storage pull (8 B/quad)

### 2.1 Record layout

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `origin.x/y/z` | `u8` each, chunk-local, `0..=CHUNK_DIM` |
| 3 | `material_lo` | low 8 material bits (atlas slot today) |
| 4–7 | `packed` | `u32`, 32 of 32 bits used |

`packed` bit allocation:

| Bits | Field |
|---:|---|
| 0–2 | face (3) |
| 3–9 | `extent_u` (7, size−1) |
| 10–16 | `extent_v` (7, size−1) |
| 17–24 | AO (4 corners × 2 bits) |
| 25 | AO diagonal-flip flag (reserved 0 today) |
| 26–31 | `material_hi` (6) |

All 32 packed bits are allocated: 25 bits of geometry/face/AO, 1 flip bit and 6
material-high bits. Material is **14 bits** with the flip reserved (15 if the
flag is never used). Extents are `size − 1`; 7 bits cover up to 128, so the
format does not depend on the current 16³ block cap. The current atlas is
16 × 16 tiles (256 slots), so 8 material bits are enough today; the extra bits
are reserved.

### 2.2 Draw and fetch contract

**Design: read-only storage-buffer pull.** There is no per-instance vertex
buffer and no `VertexStepMode::Instance` record stream. A hardware-indexed
vertex buffer cannot be offset by `base_quad` in the shader; storage pull can.

- The record buffer is a `var<storage, read>` array. **One rule, no double
  counting:** the draw sets `first_instance = base_quad`, and the vertex shader
  fetches `records[instance_index]`. WebGPU's `instance_index` already includes
  `first_instance`; adding the base again would count it twice.
- The spike's direct draw sets `first_instance = base_quad`.
  **`INDIRECT_FIRST_INSTANCE` is a hard requirement for M3-05's indirect
  path**, not a soft check: without it, indirect draws must use
  `first_instance == 0`, and the per-meshlet base cannot be expressed. It is
  **expected** on Vulkan (RTX 4060/Linux, Intel/AMD iGPU), DX12 (Windows) and
  Metal (macOS); WebGL2 does not expose it. This is an expectation, not a
  guarantee: the spike records the actual feature bit on each target adapter. If a target lacks it, the options are (a) one direct
  draw per meshlet with `first_instance = base_quad` (gives up the indirect
  path for that target) or (b) take the 8 B fallback, which does not need
  `first_instance` but abandons 64³. This is the same risk-owner decision as a
  performance regression; it is not a silent fallback and it is not an
  automatic milestone blocker.
- **Target limits:** the vertex stage must be allowed to read a storage buffer.
  Native WebGPU (Vulkan/Metal/DX12) supports it; WebGL2 does not. Assert
  `max_storage_buffers_per_shader_stage >= 1` with `ShaderStages::VERTEX`.
  Also assert `max_storage_buffer_binding_size` against the record arena: the
  128³ profile holds 805 chunks × ~393 KiB ≈ **309 MiB**, above the 128 MiB
  WebGPU default. Either request a higher adapter limit or page the arena into
  bindings ≤128 MiB; M3-02 owns that paging.
- Draw: 6 vertices × `quad_count` instances per meshlet, no index buffer. The
  shader maps `vertex_index` 0..5 to `[0,1,2,0,2,3]`, reconstructs the four
  corners from `origin + face + extent`, and selects the AO corner. The diagonal
  stays fixed `(0,1,2),(0,2,3)`; the flip bit is the hook for a future AO-aware
  flip.
- This removes the 12 B/quad index buffer. It is not an index-range fix:
  ADR-010 already splits a chunk into meshlets with `base_vertex` for the
  pathological >16,384-quad block.

### 2.3 Projection arithmetic

Per-chunk components from the M1/M2 gate reports plus the measured production
mesh; `n` is the loaded-chunk count.

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
the M6-09a v1-pool removal (~0.7% margin against the 3.20 GiB target),
~2.17 GiB after. The 64³ 8 B fallback is **5.86 GiB, above the 4.80 GiB hard
fail**.

### 2.4 Parity and performance gates

Four gates (A, B, C, D); B has a compute decode check (B1) and a real-draw check (B2). None substitutes for another.

**Gate A — CPU encoder/decoder exactness.** Decode every record into its four
corners (position, material, face, AO) and assert equality with the `Mesh`
vertices from `mesh_block`, for every fixture block at both edges. Positions
are integers, exactly representable in `f32`.

**Gate B1 — shader fetch/reconstruction exactness.** Add a compute entry point to
the same WGSL module that uses the **same storage-pull `fetch_record` helper**
as the vertex shader, writes the reconstructed `(position, face, material, AO)`
for every vertex to a storage buffer, and read it back. Compare **positions,
face, material and AO levels exactly**. AO levels are integers 0–3, so there is
no tolerance here. Coverage must include all six faces, every extent (including
repeated extents), all four AO levels, both material extremes, **a nonzero
`first_instance` base, and several meshlets in one batch**. The compute entry
point feeds thread ids and cannot observe the `instance_index`/`first_instance`
behaviour, so it must receive the same global quad index
(`base_quad + local`) that the draw will produce. This checks the decode, not
the real `instance_index`; Gate B2 checks that. Compare in the **Mesh's
expanded index order, six corners per quad** (`[0,1,2,0,2,3]`), not just the
four stored vertices.

**Gate B2 — real-draw index test (catches the base double-count).** Gate B1's
compute entry is handed the global index, so it cannot observe what
`instance_index` does in a real draw. Use an orthographic top-down camera and
non-overlapping quads on a grid so each pixel's quad is known; the vertex shader
writes the fetched quad index as a flat varying and the fragment writes it to an
`R32Uint` target. Assert each pixel equals the expected global index; if a
non-overlapping layout is impractical, assert the observed multiset equals the
expected set with **no missing or duplicate values**. Run both variants:

- **direct** with `first_instance = base_quad`, and
- **indirect** with the same nonzero base (the path M3-05 uses).

Run both on the **discrete and integrated adapters**, not llvmpipe alone:
Vulkan and DX12/Metal lower the instance-index base differently, so one
backend is not evidence for another. This is the test that fails if the base is
added twice.

Because Gate B1 and production share `fetch_record`, Gate B1 covers the production
record decode. It does **not** cover a `Uint8x4` vertex-attribute decode,
because the instanced path has none. If the 8 B vertex fallback is chosen, it
needs its own exact attribute-decode test.

**Gate C — full-frame performance on real GPUs.** llvmpipe is a CPU rasterizer
and is **not admissible** for the performance decision (functional coverage
only). Run on at least:

- one discrete GPU (RTX 4060 reference), and
- one integrated GPU (e.g. Intel UHD 770 or AMD Radeon 780M).

The primary metric is **full-frame time on a pre-registered scene**, measured
with **GPU timestamp queries** (not CPU wall time) and with **vsync disabled**
(uncapped/`AutoNoVsync` or Immediate present mode). The scene is fixed before
the run:

- **Scene:** the `m2-128` fixture tiled to 64 visible chunks; once M3-08 exists,
  its seeded stress scene is the replacement and gets the same treatment.
- **Camera:** one deterministic fly-through/cave-walk path, recorded as a
  config file in the spike directory; the same path for both renderers.
- **Frames:** 60-frame warm-up, then 300 measured frames per run; **5 runs**.
- **Noise rule:** use the **median** of the 5 runs. Define spread as
  `(max − min) / median` across the five run medians; if the spread is >5%, add
  runs or reject the set. An **11%** result within the noise rule is a
  regression; an 11% result with >5% spread is not yet a measurement.
- **Threshold:** no more than 10% full-frame regression vs the indexed path on
  either GPU, applied to the median. The reference discrete must meet 60 Hz
  (median full-frame ≤16.6 ms) at 1080p with vsync off.

The exact scene/camera config, frame counts and run count are checked into the
spike directory so the gate is reproducible.

**Vertex-bound time is informational only.** Six invocations per quad against
four cached, shared indexed vertices is a ~1.5× vertex-work problem by
construction; a 1×1-viewport or vertex-only pass will show that even when real
frames do not change. Report it (timestamped, 1×1 viewport, no fragment output)
but do not gate on it.

If the integrated-GPU runner is unavailable, Gate C stays open. The local dev
host has llvmpipe only, so this plan cannot close Gate C by itself. The
deadline/default is in §7.

**Gate D — image smoke test.** Render the same scene through the current 16 B
indexed path and the instanced path; allow a documented **≤1 LSB** tolerance.
This is where the f32 AO/colour conversion tolerance lives (not Gate B1).
Run it on the **same discrete and integrated GPUs as Gate C**, not llvmpipe
alone: the tolerance exists for GPU/compiler differences (FMA, unorm8), and the
CPU rasterizer does not exercise them. llvmpipe is functional coverage only.

### 2.5 Cost and risk

- GPU-side corner/face reconstruction and winding are a correctness risk; Gates
  B/C/D are the mitigation.
- Row 6's 600 KB lower bound is a prediction from the 76 B/quad model, not a hard
  floor; the gate note already says a lower estimate is not a failure. The
  pre-registration appendix must re-base it and keep the cap unchanged.
- The 14-bit material field is reserved, not used.

---

## 3. Fallback: 8 B packed vertex (44 B/quad)

Use this only if the instanced spike fails Gates A/B1/B2/D or Gate C after the one
retry in §7. It is **not** a schedule shortcut: it costs more time than the
instanced path (4.25–5 d vs 3–4 d) and it **abandons 64³ Minimum**.

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `local.x/y/z` | `u8` each, `0..=CHUNK_DIM` |
| 3 | spare | zero |
| 4–5 | `material` | `u16`; no 255 ceiling, no truncation |
| 6–7 | `packed` | `u16`: face 3 bits, AO 4 × 2 bits, 5 spare/version bits |

- Quad: `4 × 8 + 6 × 2 = 44 B` (42.1% smaller).
- 128³: ~2,119 KiB p95, **88.3% of cap** (2,160 KiB line, 41 KiB buffer).
- 64³: ~536 KiB, **89.3% of cap** (540 KiB line, **4 KiB buffer**).
- Row 1: 128³ 2.99 GiB (passes 3.20); **64³ 5.86 GiB (above the 4.80 hard
  fail)**. Choosing this fallback is a product decision to abandon 64³
  Minimum, not just a row-6 stopgap.
- Of the two candidates, this is the one that keeps the 12 B/quad index buffer.
- The fallback has **no real-save headroom** at either edge. Treat it as a
  synthetic-fixture stopgap that is expected to trigger the third exit on a real
  save.
- Keep the logical `Vertex` at 16 B for goldens/determinism; add a `GpuVertex`
  (8 B) for upload. The M1.5 naive path keeps its own 16 B layout.
- If chosen, Gate B1 must add an exact `Uint8x4`/`Uint16x2` attribute-decode test;
  the storage-pull Gate B1 does not cover it.

---

## 4. Options considered

| Option | Bytes/quad | 128³ p95 | Share of 2,400 KiB cap | Verdict |
|---|---:|---:|---:|---|
| **Instanced quads + storage pull** | **8 B** | **~393 KiB** | **16.4%** | **Primary** |
| 8 B packed vertex | 44 B | ~2,119 KiB | 88.3% | Fallback; no real-save headroom; abandons 64³ |
| Per-quad constants + 3 B positions | ~28 B | ~1,343 KiB | 56.0% | Dominated by instanced |
| Vertex dedup (unmeasured) | 32–40 B | ~1,500–1,900 KiB | 62–79% | Only after a 0.5 d measurement |
| 12 B vertex | 60 B | ~2,877 KiB | 120% | Reject: still over cap |
| Index compression only | 64 B | ~3,069 KiB | 128% | Reject: not enough |
| More aggressive AO merge | fewer quads | fewer | — | Reject: visual artifacts |

Percentages use the 2,400 KiB cap and the production AO p95 of 3,650 KiB. The
table is ADR-014's rejected-alternatives record.

---

## 5. Measurement fix (do this first)

Phase 0 must not depend on a codec that does not exist yet.

- In `crates/bench/src/gate/memory.rs`, after expanding the chunk, run
  `for_each_block` + `greedy_block` + `mesh_block`. Measure the **current 76 B
  logical** `Mesh` bytes in Phase 0; this reproduces the 3,650 KiB production
  number and fixes rows 1/6 now.
- Make the gate's mesh component take a **format descriptor** (bytes per quad,
  or a `MeshFormat` callback), so Phase 2 switches the row to the chosen
  encoder's byte length when it lands. Do not add an encoder dependency to
  Phase 0.
- `mesh` component = GPU bytes + meshlet records + block table. Add
  `mesh_reference = reference_quads × 76` as an informational component not in
  `total`.
- Row 6 string: `"p95 X KiB (max Y); production AO / <format> (reference Z KiB × 76 B)"`.
- Add `MESH_BYTES_PER_QUAD` sourced from the active format descriptor.
- Bump the memory and gate-report schemas. Memory v3 is a semantic break
  (reference -> production), so pre-v3 memory JSON is rejected rather than
  silently reinterpreted; archived gate reports remain readable.
- Re-run 128³ and 64³; archive; keep M1/M2 runs append-only; add the M2 erratum
  and a new history entry.

**Phase 0 result (2026-10-31):** `gate memory` now runs `mesh_block` and
charges production AO bytes + records/table. Measured: 128³ **3,650 KiB** p95
(1,250 KiB / 52.1% over the 2,400 KiB cap) and **4.16 GiB** total; 64³ **925
KiB** (325.5 KiB over 600) and **34.51 GiB**. Runs:
`benchmarks/runs/20261031T163853Z-m2-production-mesh-128` and
`...-64`. The M2 erratum is recorded.

**Phase 1 spike result (2026-10-31, partial):** the 8 B `QuadRecord` codec and
Gate A pass (exhaustive faces/extents/AO; >10k mixed-chunk quads). The
storage-pull example `quad_spike` passes Gate B1 on a **144-record coverage list
(all six faces, non-square extents, all AO patterns, material high bits)**,
B2 direct and indirect on 64 quads, and Gate D at **0 LSB** on both the RTX
4060 and the Radeon iGPU (driver `Dozen`).

**Gate C result (2026-10-31): FAIL for instanced quads.** Scene: the largest
u16-meshlet chunk of `m2-128` (capped at 16,384 quads), tiled to 64 chunks,
fixed 300-frame camera path, 60 warm-up frames, 5 runs, GPU timestamps,
vsync-free 1080p. The reported ratio is instanced/indexed median full-frame:

| GPU | Indexed | Instanced (6-vertex) | Indexed-4 | 4-vertex strip |
|---|---:|---:|---:|---:|
| RTX 4060 | 0.422 ms | 0.591 ms (**1.400×**) | 1.351× | 0.553 ms (**1.311×**) |
| Radeon iGPU | 4.323 ms | 6.237 ms (**1.443×**) | — | 5.970 ms (**1.381×**) |

All variants exceed the 10% full-frame threshold. The instanced record path is
rejected on performance; the compute/attribute equivalence is not the issue,
the extra per-quad vertex work is. Run with
`cargo run -p voxel-viewer --features chunk128 --example gate_c` under the WSL
GPU env.

**Next decision:** the planned fallback is the **8 B packed vertex** attribute
path (4 vertices per quad; 44 B/quad indexed, or 32 B/quad with a 4-vertex
strip). It clears row 6 at 2,119 KiB (11.7% margin) and row 1 at 2.99 GiB, but
the 64³ total is 5.86 GiB (44 B) or 5.0 GiB (32 B), above the 4.80 GiB hard
fail until M6-09a. Choosing the fallback **abandons 64³ Minimum** until then;
that is the §7/§9 product decision.

---

## 6. Work plan and schedule

### Phase 0 — true baseline (1 d)

1. Add the production `mesh_block` measurement with the current 76 B format.
2. Re-run both edges; archive.
3. Append the M2 erratum: production 3,650 KiB, miss 1,250 KiB, row 1 4.16 GiB.
4. Update the repeated numbers in the task list, M1 decision, roadmap and
   performance targets.

### Phase 1 — instanced-quad spike (1.5–2 d; the 2 d upper bound includes B2)

- Record encoder + CPU decoder (Gate A).
- Storage-pull `fetch_record` helper, used by both the vertex and compute entry
  points; Gate B1 readback.
- Gate D (≤1 LSB) on the **discrete and integrated GPUs** (same session as
  Gate C); llvmpipe is only a functional pre-check, not the tolerance gate.
- Gate B2 (grid/ortho scene, direct **and** indirect) on the discrete and
  integrated GPUs; it shares the Gate C session and fits in the 2 d upper
  bound.
- Gate C on discrete + integrated GPUs with timestamp queries and vsync off;
  full-frame primary, vertex-bound informational.
- **Deadline:** if the iGPU is not available within **5 working days**, proceed
  on the discrete-only result with the affected real-GPU gates (C, D and B2)
  recorded **open**, unless the discrete result itself fails; then take the
  fallback. The open flag is carried to M3-13 for a follow-up.
- If Gates A/B1/B2/D and the full-frame result pass, the spike becomes the
  implementation base. A fixable failure gets one retry; otherwise fallback.

### Phase 2 — instanced quads (3–4 d)

- Record encoder, meshlet records (`base_quad`, `quad_count`, block index),
  per-chunk record buffer.
- Shader storage pull, fixed diagonal, AO corner selection, flip bit reserved.
- Viewer wiring: `ChunkMeshData` holds records; `chunk_mesh`/`chunk_render` use
  the storage-pull draw; no index buffer.
- Naive/screenshot path stays on its own 16 B layout.
- Gate A/B1 and the functional B2 (llvmpipe) are CI-gated; the real-GPU direct
  and indirect B2 variants are recorded with C and D in the same session.
- Gate re-run at both edges; row 6 and row 1 pass; archive.
- **M3-02 arena requirement:** the arena must be **format-agnostic** (allocate
  byte ranges; take the stride/record size from a descriptor), and it must page
  the record arena into ≤128 MiB bindings with **no draw or indirect batch
  crossing a page**; `first_instance` is page-relative, and a 309 MiB single
  buffer also exceeds the default `max_buffer_size` (256 MiB). This adds
  **1.0–1.5 d** to M3-02 (1.5 d → 2.5–3.0 d); see the schedule.

### Fallback path

8 B `GpuVertex`, 44 B/quad: codec 0.75 d, viewer 1 d, naive split 0.5 d, CPU +
shader + image parity 1 d, gate/re-run 1 d = **4.25–5 d after the spike**. It
costs more than instanced (3–4 d) and abandons 64³ Minimum.

### Effort and calendar

Request the iGPU on **day 1**; Phase 0 and the functional spike (2.5–3 d) finish
inside the 5-day wait, so only the post-gate work is additive to the calendar.

| Path | Effort | Calendar if the iGPU arrives early | Calendar if it takes the full 5 d |
|---|---:|---:|---:|
| Best (instanced passes) | 5.5–7 d | 5.5–7 d | **8–9 d** (5 d wait + 3–4 d instanced after the gate) |
| Typical (spike fails, fallback passes) | 6.75–8 d | 6.75–8 d | **9.25–10 d** (5 d wait + 4.25–5 d fallback) |
| Worst (fallback misses 90%, third exit) | 7.75–11 d | 7.75–11 d | **10.25–13 d** (5 + 4.25–5 + 1–3) |

M3 needs **+1.0–1.5 d for M3-02** (format descriptor, ≤128 MiB binding pages,
`max_buffer_size`, and no draw crossing a page), so the published 9 d becomes
**15.5–17.5 d best and 17.75–21.5 d worst**, before any late-iGPU calendar
slip. Quote effort and calendar separately; the wait is calendar, not effort,
but it is still a schedule commitment. The full-wait column is
`5 + post-gate effort`, not `effort + remaining wait`.

---

## 7. Rule, retries, hardware deadline and exits

**Rule (both formats):** a format is accepted only if its measured/projected p95
is **≤ 90% of the row-6 cap** (≥10% margin).

| Format | 128³ | 64³ |
|---|---:|---:|
| Instanced | 16.4% of cap (83.6% margin) | 16.4% (83.6%) |
| 8 B vertex | 88.3% (11.7% margin, 41 KiB) | **89.3% (10.7%, 4 KiB)** |

The 8 B fallback is inside the rule only on the synthetic fixtures; it has no
real-save headroom, and at 64³ its total (5.86 GiB) fails the 4.80 GiB hard
fail. **Choosing the fallback abandons 64³ Minimum.**

**Decision tree (one retry maximum):**

1. Spike attempt 1 passes Gates A/B1/B2/D and the full-frame Gate C → **instanced**.
2. Attempt 1 fails a fixable class → fix once, **retry attempt 2**. Pass →
   instanced; fail → 3.
3. **8 B fallback.** If it measures ≤90% of cap → accept as a stopgap and record
   the real-save trigger. This **abandons 64³ Minimum**. If it measures >90% →
   4.
4. **Third exit:**
   - **4a. Vertex dedup**, only after a **0.5 d measurement** on the fixtures
     proves the byte win. The 32–40 B/quad figure is unmeasured and needs an
     index cache that breaks the stateless 8 B stride; if the measurement does
     not clearly clear 90%, do not rely on it.
   - **4b. Re-plan row 6/cap** or accept another profile — an explicit ADR-002
     decision.

**Real-GPU gates open:** the 5-day deadline and open-flag cover **Gate C, Gate
D and Gate B2's real-GPU variants together**. Request the iGPU on **day 1**. If
it is unavailable after **5 working days**, proceed on the discrete-only
results with the affected gates recorded open (or take the fallback if the
discrete result fails). The open flag goes to M3-13; it does not block the
decision indefinitely and it is not silently closed.

**Consequence and risk owner:** if the discrete-only result is accepted and a
later iGPU check shows a regression, the response is the fallback (4.25–5 d,
which also abandons 64³ Minimum) or a re-plan. A missing
`INDIRECT_FIRST_INSTANCE` on a target is the **same risk-owner decision**:
direct draws for that target (no indirect) or the 8 B fallback (abandons 64³).
The milestone owner accepts that risk in writing when the discrete-only
decision is recorded; it is not an implicit acceptance.

**64³ contingency:** if the 64³ row 1 measures between 3.20 and 4.80 GiB, the
mesh format cannot close the gap. Record a dated pre-registration appendix that
the 64³ target is breached **pending M6-09a** (v1 pool removal), with the
milestone owner's sign-off and a resolution deadline at M6-09a. If the
measurement is above 4.80 GiB, 64³ Minimum is rejected. Do not silently relax
the target and do not pretend a format change can fix it.

**Real-save trigger:** if the first real save's row 11 p95 is above 90% of the
selected format's cap, re-open the format decision. Do not close row 11 on a
synthetic fixture.

---

## 8. Acceptance

- `cargo test -p voxel-mesh` (logical codec, mesh shape, AO, determinism,
  goldens at both edges) and `cargo test -p voxel-viewer` (Gates A/B1/B2/D).
- Gate A: decoded corners equal `Mesh` exactly.
- Gate B1: shader fetch/reconstruction exact for position, face, material and AO
  levels; coverage includes all faces, extents and AO levels.
- Gate B2: direct and indirect draws on the discrete and integrated GPUs;
  per-pixel expected index, or multiset equality with no missing/duplicate
  quads.
- Gate C: discrete + integrated GPUs, GPU timestamp queries, vsync off;
  full-frame ≤10% regression; vertex-bound reported informationally; reference
  discrete still 60 Hz at 1080p. If open after the deadline, recorded as open
  with a follow-up, not passed.
- Gate D: RGBA difference ≤1 LSB on the same discrete and integrated
  GPUs.
- Gate 128³ Minimum: row 6 PASS with p95 ≤ 2,400 KiB and ≤90% of cap; row 1 ≤
  3.20 GiB.
- Gate 64³: row 6 ≤ **540 KiB** (90% rule), row 1 ≤ **3.20 GiB** and ≤ 4.80
  GiB hard fail. The 3.20 GiB target remains **required**: instanced is 3.18
  GiB p95-projected, 0.02 GiB (~0.7%) under the target before the M6-09a pool
  removal, and 2.17 GiB after. The ~0.7% margin is thin and a real save may
  consume it; this is not a pre-registration relaxation. If the measurement
  lands in 3.20–4.80 GiB, the §7 contingency applies (dated appendix, owner
  sign-off, M6-09a resolution). The 8 B fallback fails both the hard fail
  (5.86 GiB) and the target.
- ADR-014 merged, pre-registration appendix added, M2 erratum recorded, CI
  goldens/parity job green.

---

## 9. Decisions requested

1. **Primary path:** instanced quads + storage pull with the spike. The 8 B
   vertex is a fallback only if the spike fails, not a schedule choice; it costs
   more time and abandons 64³ Minimum.
2. **Hardware access:** can the spike run on the RTX 4060 reference **and** an
   integrated GPU within 5 working days? If not, the default is discrete-only
   with the affected real-GPU gates (C, D and B2) recorded open; the milestone
   owner accepts in writing that a later iGPU regression costs the fallback
   (4.25–5 d) and abandons 64³.
3. **64³ product decision:** if the fallback is ever chosen, accept that 64³
   Minimum is abandoned (5.86 GiB > 4.80 hard fail). If 64³ must stay viable,
   the instanced path is required. If the instanced 64³ row 1 measures
   3.20–4.80 GiB, the dated appendix / owner sign-off / M6-09a contingency in
   §7 applies; it is not a silent relaxation.
4. **Schedule:** re-baseline M3 to best **15.5–17.5 d** / worst **17.75–21.5 d** (includes the +1.0–1.5 d M3-02 work), or move M3-08/M3-12.
5. **Placement:** new `M3-00a`/`M3-00b` before `M3-01`.

---

## 10. Revision history

- **rev 1:** 8 B vertex primary; Phase 2 bit budget wrong; u16 material treated
  as a 1 B trade; 15% rule contradicted by an 11.7% recommendation.
- **rev 2:** instanced quads primary; fixed the 4 B packed word and u16
  fallback; introduced a parity spike; left a circular fallback and a 95%
  trigger.
- **rev 3:** one 10% rule; third exit; CPU exactness; ≤1 LSB GPU smoke.
- **rev 4:** real-GPU Gate C with vertex-bound/pixel split; best/typical/worst
  schedule; 14-bit material allocation; shader output readback; 64³ fallback
  called out as 4 KiB inside the rule; arithmetic table.
- **rev 5:** storage-pull design (no per-instance vertex buffer);
  full-frame Gate C with timestamp queries and vsync off, vertex-bound
  informational; 5-day Gate C open deadline; Phase 0 uses the current 76 B
  format; fallback explicitly abandons 64³ and is not a schedule option; M3-02
  arena made format-agnostic.
- **rev 6:** one `first_instance` rule; effort/calendar split, risk owner, and
  the first storage-binding note.
- **rev 7:** real-draw index Gate B2; `INDIRECT_FIRST_INSTANCE` as a
  hard requirement with the adapter list; page-boundary draws, 128 MiB binding
  and 256 MiB buffer limits; Gate D on real GPUs; 64³ target kept at 3.20 GiB
  (~0.7% p95 margin); 1.5–2 d spike; 11% noise example and spread definition.
- **rev 8:** corrected the 5-day calendar arithmetic (`5 + post-gate effort`);
  B2 pixel mapping and direct + indirect variants on real GPUs; the
  `INDIRECT_FIRST_INSTANCE` list marked expected; 64³ 3.20–4.80 GiB contingency
  with owner sign-off and M6-09a resolution; real-GPU deadline covers C, D and
  B2; acceptance adds B2 and the Gate D GPU requirement.
- **rev 9 (current):** B2 is in the Phase 1 spike and covered by the 2 d upper
  bound; Phase 1 deadline names C/D/B2; a missing `INDIRECT_FIRST_INSTANCE` is
  a risk-owner choice (direct draws or fallback), not an automatic blocker; CI
  runs functional B2 and the real-GPU variants are recorded with C/D; 64³
  margin corrected to ~0.7%.
