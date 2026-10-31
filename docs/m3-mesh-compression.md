# Mesh compression plan (M3-00)

**Status:** Proposed (revised) — 2026-10-31
**Trigger:** M2 exit, gate row 6. The production AO mesh exceeds the 128³ cap.
**Goal:** bring the measured 128³ mesh p95 under the 2,400 KiB cap, restore the
128³ Minimum memory projection, and do it without a visual change.

> **Revision note.** The first draft had three real errors: the Phase 2 bit
> budget did not fit 2 bytes, the u16-material tradeoff was not a tradeoff, and
> the 15% margin rule was contradicted by the recommendation. This revision
> fixes those and changes the primary recommendation to instanced quads with a
> parity spike, keeping the 8 B vertex as the fallback.

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
With n = 9, the nearest-rank p95 is also the maximum, which is a second reason
to treat 3,645 KiB as an upper-bound-flavoured number from a single archetype.

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

## 2. Recommended primary: instanced quads (8 B/quad)

### 2.1 Record layout

One 8 B record per quad:

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `origin.x/y/z` | `u8` each, chunk-local, `0..=CHUNK_DIM` |
| 3 | `material` | `u8` atlas slot, `0..=255` |
| 4–7 | `packed` | `u32`: face 3 bits, `extent_u` 7 bits (size−1), `extent_v` 7 bits (size−1), AO 8 bits (2 per corner), 7 spare bits |

The 21-bit version in the first draft did not fit 2 bytes: `face 3 + u 5 + v 5
+ ao 8 = 21`. The corrected record is **25 bits in a 4 B word**, so the 8 B
total survives but there are **no spare bytes**. Extents are stored as
`size − 1`; 7 bits cover up to 128, so the format does not depend on the current
16³ block cap (the current mesher still caps at 16, which only makes the field
easier to fill).

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
- This removes the 12 B/quad index buffer and the chunk-wide `u16` index cap
  (16,384 quads), which the current 128³ chunk (49k quads) cannot satisfy
  anyway. It aligns with M3-02 (arena) and M3-05 (per-chunk draws).

### 2.3 Projected result

| | 128³ | 64³ |
|---|---:|---:|
| Production p95 quads | 49,112 | 12,449 |
| Mesh at 8 B/quad + records | **~393 KiB** | **~98 KiB** |
| Row 6 cap | 2,400 KiB | 600 KiB |
| **Margin** | **~2,007 KiB (84%)** | **~502 KiB (84%)** |
| Row 1 total | **~1.66 GiB** (target 3.20) | **~3.2 GiB before M6-09a** (zero margin); **~2.2 GiB after** |

The 128³ Minimum clears both the mesh cap and the 3.20 GiB target with a large
margin. The 64³ fallback reaches ~3.2 GiB only after the v1-pool removal; do
**not** claim it is re-opened before then.

### 2.4 Cost and risk

- GPU-side corner/face reconstruction and winding become a correctness risk
  that the 8 B vertex does not have. This is why the plan starts with a
  **parity spike** (§4).
- The pre-registered row 6 range is 600 KB–2.4 MB at 128³. 393 KiB is below the
  600 KB lower bound. That lower bound is a prediction from the 76 B/quad
  model, not a hard floor; the gate note already says a lower estimate is not a
  failure. The dated pre-registration appendix must re-base the lower bound and
  keep the cap unchanged.
- Material is `u8` in the record because it is an atlas slot (the current atlas
  is 16 × 16 tiles); the shader already uses 8-bit tile math. If the atlas grows
  past 256 tiles, the material field and the tile math change together.

---

## 3. Fallback: 8 B packed vertex (44 B/quad)

Use this only if the instanced-quad parity spike fails. It is the minimum legal
stride for the current 4-vertex-per-quad field set (`array_stride % 4 == 0`).

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
- 128³ p95: ~2,119 KiB with records (cap 2,400); 64³ ~536 KiB (cap 600).
- Row 1: `(1,772 + 2,119) × 805 = 2.99 GiB`, under 3.20.
- Margin is only **11.7%**. The first draft claimed a 15% rule and then
  recommended this; that was inconsistent. The revised rule: **15% is the
  preference for the primary format; the 8 B fallback is accepted at 10% with
  Trigger A at 2,280 KiB (95% of cap), not 2,160.** A fallback re-measure above
  2,280 KiB forces the instanced path.
- Keep the logical `Vertex` at 16 B for goldens/determinism; add a `GpuVertex`
  (8 B) for upload. The M1.5 naive path keeps its own 16 B layout so the
  screenshot example and its goldens do not churn.

---

## 4. Measurement fix (do this first)

The first draft proposed counting `greedy_block` quads × 44 B. That is the same
proxy mistake one layer down. Run the real path:

- In `crates/bench/src/gate/memory.rs`, after expanding the chunk, run
  `for_each_block` + `greedy_block` + `mesh_block`, and call the GPU encoder's
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

## 5. Revised work plan

### Phase 0 — true baseline (1 d)

1. Add the production `mesh_block` measurement to `gate memory`.
2. Re-run both edges; archive.
3. Append the M2 erratum: production 3,645 KiB, miss 1,245 KiB, row 1 4.16 GiB.
4. Update the repeated numbers in the task list, M1 decision, roadmap and
   performance targets.

### Phase 1 — instanced-quad parity spike (1 d, go/no-go)

Build a single-chunk offscreen prototype: 8 B quad records, non-indexed 6
vertices × instances, GPU corner reconstruction, shared camera/atlas/light.
Render the same scene through the current 16 B indexed pipeline and the new
path; assert the RGBA buffers are equal. If the spike cannot be made
byte-equal, stop and take the 8 B vertex fallback. If it passes, it becomes the
implementation base.

The AO input must be identical: compute `f32(level * 85u) / 255.0` in the shader
(not `f32(level) / 3.0`). `85 * level` is exactly representable, so the two
paths feed the same float numerator into the same divide and the parity test is
not ULP-flaky.

### Phase 2 — instanced quads (2–3 d)

- Record encoder, meshlet records, per-chunk record buffer.
- Shader reconstruction, fixed diagonal, AO corner selection; flip bit reserved.
- Viewer wiring: `ChunkMeshData` holds records; `chunk_mesh`/`chunk_render` use
  the instance layout; no index buffer.
- Naive/screenshot path stays on its own 16 B layout.
- Frame-parity test (16 B indexed vs 8 B instanced) committed and CI-gated.
- Gate re-run at both edges; row 6 and row 1 pass; archive.

### Fallback path (if the spike fails)

8 B `GpuVertex`, 44 B/quad: codec 0.75 d, viewer 1 d, naive split 0.5 d, parity
1 d, gate/re-run 1 d ≈ 4 d. Then re-run and archive with Trigger A at 2,280 KiB.

### Schedule reality

Phase 0 + instanced path ≈ **5–6 d**; the fallback ≈ 5 d. M3's published budget
is 9 d, so this is a **re-baseline**, not absorption: M3 becomes ~14–15 d, or
M3-08/M3-12 move. Do not hide this inside an existing task. The instanced path
is estimated at 2–3 d after the spike because the spike is the risky half; the
first draft's "Phase 1 4 d then Phase 2 2–3 d" double-counted the viewer/parity
work and made the order look cheaper than it is.

---

## 6. Triggers and stop conditions

- **Primary (instanced):** ~84% margin. No margin trigger; the parity test is
  the gate.
- **Fallback (8 B vertex):** 11.7% margin. **Trigger A:** if the re-measured
  128³ production p95 at 44 B is **> 2,280 KiB (95% of cap)**, switch to
  instanced quads. The first draft's 2,160 KiB trigger was the 90% point for a
  15% rule and left only 41 KiB above the projection; 2,280 KiB leaves 161 KiB.
- **Real save:** if the first real save's row 11 p95 is above the selected
  format's trigger, re-open the format. Do not close row 11 on a synthetic
  fixture.
- **Parity:** a non-byte-equal framebuffer is a failure of the AO/face
  reconstruction. Fix the math; do not accept a 1-LSB difference.
- **64³:** instanced reaches ~3.2 GiB only before the M6-09a pool removal (zero
  margin); ~2.2 GiB after. The 8 B vertex fallback does not re-open 64³
  (5.89 GiB).
- **AO-tolerant merging:** possible, but rejected on visual parity; it is not
  an impossibility, just outside this plan.

---

## 7. Acceptance

- `cargo test -p voxel-mesh` (logical codec, mesh shape, AO, determinism,
  goldens at both edges) and `cargo test -p voxel-viewer` (parity + validation).
- Frame parity: current 16 B indexed vs new format, offscreen RGBA byte-equal.
- Gate 128³ Minimum: row 6 PASS with p95 ≤ 2,400 KiB recorded; row 1 ≤ 3.20 GiB.
- Gate 64³: row 6 PASS ≤ 600 KiB; record the total and the pool caveat.
- Throughput within noise of 34.3 chunks/s at 128³; scratch budget unchanged.
- ADR-014 merged, pre-registration appendix added, M2 erratum recorded, CI
  goldens/parity job green.

---

## 8. Decisions requested

1. **Primary path:** instanced quads with the 1-day parity spike (recommended)
   or 8 B vertex first. The u16-material question is no longer a decision; the
   fallback layout uses u16 material in the same 8 B stride.
2. **Row 6 lower bound:** re-base the 600 KB lower bound for the instanced
   path in the pre-registration appendix, or keep the 44 B fallback to stay
   inside the registered range. The cap is unchanged either way.
3. **Schedule:** re-baseline M3 to ~14–15 d, or move M3-08/M3-12 to make room.
4. **Placement:** new `M3-00a`/`M3-00b` before `M3-01`.
