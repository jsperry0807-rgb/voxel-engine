# Mesh compression plan (M3-00)

**Status:** Proposed (revision 3) — 2026-10-31
**Trigger:** M2 exit, gate row 6. The production AO mesh exceeds the 128³ cap.
**Goal:** bring the measured 128³ mesh p95 under the 2,400 KiB cap, restore the
128³ Minimum memory projection, and preserve the current visual contract.

> **Revision note.** Revision 2 fixed the 2-byte bit budget and the fake
> u16-material tradeoff, but left a circular fallback, a 10% rule paired with a
> 95% trigger, and an overclaimed byte-equal GPU parity. This revision fixes
> those: one 10% acceptance rule, a defined third exit, CPU-exact record
> decoding as the parity proof, and a GPU smoke test with a ≤1 LSB tolerance.

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

### 2.1 Record layout

One 8 B record per quad:

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `origin.x/y/z` | `u8` each, chunk-local, `0..=CHUNK_DIM` |
| 3 | `material` | `u8` atlas slot today; the 7 spare bits below extend it to 15 bits |
| 4–7 | `packed` | `u32`: face 3 bits, `extent_u` 7 bits (size−1), `extent_v` 7 bits (size−1), AO 8 bits (2 per corner), 7 spare bits |

The 21-bit version in the first draft did not fit 2 bytes:
`face 3 + u 5 + v 5 + ao 8 = 21`. The corrected record is **25 bits in a 4 B
word**, so the 8 B total survives but there are no spare bytes. Extents are
stored as `size − 1`; 7 bits cover up to 128, so the format does not depend on
the current 16³ block cap.

**Material headroom:** the material byte gives 8 bits and the packed word has 7
spare bits, so the record supports a **15-bit material** with no extra bytes.
The current atlas is 16 × 16 tiles (256 slots), so `u8` is enough today; the
format is not stuck at 256.

### 2.2 Draw

- No index buffer. One draw per meshlet: **6 vertices × `quad_count`
  instances**, `instance_index` selects the record, `vertex_index` selects the
  corner.
- The vertex shader maps `vertex_index` 0..5 to the current index pattern
  `[0,1,2,0,2,3]`, reconstructs the four corners from
  `origin + face + extent`, and selects the AO corner. The quad diagonal stays
  the current fixed `(0,1,2),(0,2,3)`; the mesher does **not** flip it for AO
  anisotropy today. If a future AO-aware diagonal flip is added, spend one of
  the 7 spare bits on a flip flag and mirror the permutation in the shader.
- This removes the 12 B/quad index buffer. It is **not** an index-range fix:
  ADR-010 already splits a chunk into meshlets with `base_vertex` for the
  pathological >16,384-quad block, so the 49k-quad 128³ chunk is a meshlet
  count, not an index problem. The saving is bytes, not correctness.

### 2.3 Projected result

| | 128³ | 64³ |
|---|---:|---:|
| Production p95 quads | 49,112 | 12,449 |
| Mesh at 8 B/quad + records | **~393 KiB** | **~98 KiB** |
| Row 6 cap | 2,400 KiB | 600 KiB |
| Share of cap | **16.4%** | **16.3%** |
| **Margin** | **~2,007 KiB (83.6%)** | **~502 KiB (83.7%)** |
| Row 1 total | **~1.66 GiB** (target 3.20) | **~3.2 GiB before M6-09a** (zero margin); **~2.2 GiB after** |

The 128³ Minimum clears both the mesh cap and the 3.20 GiB target with a large
margin. The 64³ fallback reaches ~3.2 GiB only before the v1-pool removal (zero
margin against the 3.20 target); do **not** claim it is re-opened until M6-09a.

### 2.4 Cost and risk

- GPU-side corner/face reconstruction and winding are a correctness risk the
  8 B vertex does not have. The parity proof is therefore split:
  - **CPU exactness (the proof):** decode every record into its four corners
    (position, material, face, AO) and assert they equal the `Mesh` vertices
    from `mesh_block` **exactly**, for every fixture block at both edges. This
    is a codec test, not a GPU test.
  - **GPU smoke test (not a byte-equality claim):** render the same scene
    through the current 16 B indexed path and the instanced path; allow a
    documented **≤1 LSB** tolerance. Division and unorm8 fetch are not
    bit-identical across GPUs/compilers, and FMA contraction can differ between
    programs. `f32(level * 85u) / 255.0` is the right input, but it is not a
    bit-exactness guarantee.
  - **Frame time (same spike):** 6 vertex invocations per quad versus 4
    post-transform-cached indexed vertices can cost real shader time. Measure
    both paths at 1920 × 1080 on the reference/llvmpipe and record the delta;
    the spike must report frame time as well as pixels.
- The pre-registered row 6 range is 600 KB–2.4 MB at 128³. 393 KiB is below the
  600 KB lower bound. That lower bound is a prediction from the 76 B/quad
  model, not a hard floor; the gate note already says a lower estimate is not a
  failure. The dated pre-registration appendix must re-base the lower bound and
  keep the cap unchanged.
- Material is `u8` today because it is an atlas slot; the 15-bit extension is
  reserved, not used.

---

## 3. Fallback: 8 B packed vertex (44 B/quad)

Use this if the instanced spike fails parity or frame time, or if instanced is
rejected for schedule reasons.

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `local.x/y/z` | `u8` each, `0..=CHUNK_DIM` |
| 3 | spare | zero |
| 4–5 | `material` | **`u16`**; no 255 ceiling, no truncation |
| 6–7 | `packed` | `u16`: face 3 bits, AO 4 × 2 bits, 5 spare/version bits |

This corrects the first draft's false tradeoff: a `u16` material fits in the
same 8 B stride because the reserved bytes are used. It removes the silent
truncation hazard at the encoder; `resolve_materials` is still required for the
atlas tile, but a missed resolve is a wrong tile rather than a lost id.

- Quad: `4 × 8 + 6 × 2 = 44 B` (42.1% smaller).
- 128³ p95: ~2,119 KiB with records (cap 2,400; **88.3% of cap**, 11.7% margin).
- 64³: ~536 KiB (cap 600).
- Row 1: `(1,772 + 2,119) × 805 = 2.99 GiB`, under 3.20.
- Keep the logical `Vertex` at 16 B for goldens/determinism; add a `GpuVertex`
  (8 B) for upload. The M1.5 naive path keeps its own 16 B layout so the
  screenshot example and its goldens do not churn.
- This is the **only** format that keeps the 12 B/quad index buffer; that is a
  deliberate simplicity trade, not a requirement.

---

## 4. Options considered

| Option | Bytes/quad | 128³ p95 | Share of 2,400 KiB cap | Verdict |
|---|---:|---:|---:|---|
| **Instanced quads** | **8 B** | **~393 KiB** | **16.4%** | **Primary** |
| 8 B packed vertex | 44 B | ~2,119 KiB | 88.3% | Fallback under the 10% rule |
| Per-quad constants + 3 B positions | ~28 B | ~1,343 KiB | 56.0% | Dominated by instanced |
| Vertex dedup within 8 B vertex | 32–40 B | ~1,500–1,900 KiB | 62–79% | Third exit if fallback misses |
| 12 B vertex | 60 B | ~2,877 KiB | 120% | Reject: still over cap |
| Index compression only | 64 B | ~3,069 KiB | 128% | Reject: not enough |
| More aggressive AO merge | fewer quads | fewer | — | Reject: visual artifacts |

Percentages use the 2,400 KiB cap and the production AO p95 of 3,645 KiB. The
table is the rejected-alternatives record ADR-014 needs.

---

## 5. Measurement fix (do this first)

The first draft proposed counting `greedy_block` quads × 44 B. That is the same
proxy mistake one layer down. Run the real path:

- In `crates/bench/src/gate/memory.rs`, after expanding the chunk, run
  `for_each_block` + `greedy_block` + `mesh_block`, and call the encoder's
  byte-length method (`Mesh::gpu_byte_len(format)` or `GpuMesh::from(&mesh)`).
  The gate then reads the exact upload bytes, not a projection. Cost is ~0.3 s
  for 9 chunks at 128³.
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

## 6. Work plan

### Phase 0 — true baseline (1 d)

1. Add the production `mesh_block` measurement to `gate memory`.
2. Re-run both edges; archive.
3. Append the M2 erratum: production 3,645 KiB, miss 1,245 KiB, row 1 4.16 GiB.
4. Update the repeated numbers in the task list, M1 decision, roadmap and
   performance targets.

### Phase 1 — instanced-quad spike (1 d, go/no-go)

- Implement the 8 B record encoder and a CPU decoder; assert decoded corners
  equal the `Mesh` vertices exactly for every fixture quad at both edges.
- Render one chunk through the current 16 B indexed path and the instanced path
  into two offscreen targets; assert the RGBA difference is within **≤1 LSB**
  (documented tolerance, not byte equality).
- Measure frame time for both paths at 1920 × 1080 on the reference/llvmpipe
  and record the delta. A large regression (e.g. >10% on the reference GPU or
  below 60 Hz) is a no-go even if pixels match.
- If the spike passes both, it becomes the implementation base. If it fails,
  record the failure class (CPU corner mismatch, GPU tolerance, frame time) and
  take the fallback or the third exit in §7.

### Phase 2 — instanced quads (3–4 d)

- Record encoder, meshlet records (`base_instance`, `quad_count`, block index),
  per-chunk record buffer.
- Shader reconstruction, fixed diagonal, AO corner selection; flip bit reserved.
- Viewer wiring: `ChunkMeshData` holds records; `chunk_mesh`/`chunk_render` use
  the instance layout; no index buffer.
- Naive/screenshot path stays on its own 16 B layout.
- CPU exactness test and GPU smoke test committed and CI-gated.
- Gate re-run at both edges; row 6 and row 1 pass; archive.

The 3–4 d includes the viewer wiring, naive split, CI parity test and the gate
re-run; the earlier “2–3 d after the spike” omitted those and was an undercount.

### Fallback path (if the spike fails)

8 B `GpuVertex`, 44 B/quad: codec 0.75 d, viewer 1 d, naive split 0.5 d, CPU +
GPU parity 1 d, gate/re-run 1 d ≈ **4.25–5 d**. Re-run and archive under the
same 10% rule.

### Schedule reality

Phase 0 + spike + instanced ≈ **5–6 d**; the fallback ≈ **4.5–5 d**. M3's
published budget is 9 d, so this is a **re-baseline to ~14–15 d**, or M3-08 /
M3-12 move. Do not hide it inside an existing task.

---

## 7. One margin rule, triggers and exits

**Rule (applied to both formats):** a format is accepted only if its
measured/projected p95 is **≤ 90% of the row-6 cap** (≥10% margin). This is the
single rule; it is not a 15% preference with a 10% exception.

| Format | p95 | Share of cap | Rule |
|---|---:|---:|---|
| Instanced quads | ~393 KiB | 16.4% | **Pass**, 73.6 points of headroom |
| 8 B vertex | ~2,119 KiB | 88.3% | **Pass**, but only 1.7 points inside the rule |

The 8 B fallback is admissible under the same rule, but its 41 KiB buffer to
the 90% line is thin. The previous draft's “10% accepted, 95% trigger” was
inconsistent; the trigger is now the rule itself.

**Decision tree:**

1. Instanced spike passes CPU exactness, the ≤1 LSB GPU smoke test and the
   frame-time check → **instanced**.
2. Spike fails a fixable class (corner mapping, winding, tolerance) → fix once
   and re-run. Still fails → 8 B fallback.
3. 8 B fallback measured at ≤90% of cap → **accept the fallback**; re-run and
   archive. Measured >90% → **third exit**:
   - **3a. Fix the spike** if the failure is understood and bounded (e.g. a
     corner permutation or a shader FMA issue); re-run the spike.
   - **3b. Vertex dedup** inside the 8 B vertex path (share corners with
     identical material/face/AO; 32–40 B/quad, ~62–79% of cap). Requires an
     index cache and a new measurement; it is the next byte lever before a cap
     decision.
   - **3c. Re-plan row 6/cap** or accept a different profile (ADR-002
     decision, not a task). Do not silently widen the frozen cap.

The third exit is the answer to the circularity: the fallback's trigger never
says “switch to instanced” when instanced already failed; it routes to a fix,
dedup, or an explicit profile decision.

**Real-save trigger:** if the first real save's row 11 p95 is above 90% of the
selected format's cap, re-open the format decision. Do not close row 11 on a
synthetic fixture.

**64³:** instanced reaches ~3.2 GiB before the M6-09a pool removal (zero
margin) and ~2.2 GiB after. The 8 B vertex fallback does not re-open 64³
(5.89 GiB).

---

## 8. Acceptance

- `cargo test -p voxel-mesh` (logical codec, mesh shape, AO, determinism,
  goldens at both edges) and `cargo test -p voxel-viewer` (CPU exactness + GPU
  smoke + validation).
- CPU exactness: every decoded record's four corners equal the `Mesh` vertices
  exactly.
- GPU smoke: RGBA difference ≤1 LSB, documented.
- Gate 128³ Minimum: row 6 PASS with p95 ≤ 2,400 KiB and ≤90% of cap; row 1 ≤
  3.20 GiB.
- Gate 64³: row 6 PASS ≤ 600 KiB; record the total and the pool caveat.
- Frame time within the spike threshold; throughput within noise of 34.3
  chunks/s at 128³; scratch budget unchanged.
- ADR-014 merged, pre-registration appendix added, M2 erratum recorded, CI
  goldens/parity job green.

---

## 9. Decisions requested

1. **Primary path:** instanced quads with the 1-day spike (recommended) or the
   8 B vertex fallback first. Both are under the same 10% rule.
2. **Schedule:** re-baseline M3 to ~14–15 d, or move M3-08/M3-12 to make room.
3. **Placement:** new `M3-00a`/`M3-00b` before `M3-01`.
