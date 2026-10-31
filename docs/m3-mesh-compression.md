# Mesh compression plan (M3-00)

**Status:** Proposed — 2026-10-31
**Trigger:** M2 exit, gate row 6. The production AO mesh exceeds the 128³ cap.
**Goal:** bring the measured 128³ mesh p95 under the 2,400 KiB cap, restore the
128³ Minimum memory projection under the 3.20 GiB target, and do it without a
visual change.

---

## 1. The real gap is 52%, not 5%

The M2 exit recorded row 6 as **2,519 KiB p95** and a **119.4 KiB** miss. That
number is the **M1-31 no-AO reference mesh** (`count_greedy_dense`), not the
production mesh. Re-running the production path
(`for_each_block` + `greedy_block` + `mesh_block`) over the same frozen saves
gives:

| Fixture | Reference p95 | Production AO p95 | Ratio | Cap | Miss |
|---|---:|---:|---:|---:|---:|
| 128³ `m2-128` (9 chunks) | 2,519 KiB | **3,645 KiB** | 1.447× | 2,400 KiB | **1,245 KiB (51.9%)** |
| 64³ `m1-representative` (72 chunks) | 753 KiB | **924 KiB** | 1.227× | 600 KiB | **324 KiB (54.0%)** |

The 128³ production total is 346,081 quads / 26,302,156 B over 9 chunks; the
p95 chunk is 49,112 quads. With n = 9 the nearest-rank p95 is the maximum, so
the projection is conservative in that one sense — but it is also a single
terrain archetype, which is the larger risk (§6).

Two consequences:

1. **Row 1 is also wrong today.** `MemoryComponents::total` includes the mesh
   estimate, so the 3.29 GiB projection is built on 2,519 KiB + 1,772 KiB. With
   the production mesh it is `(1,772 + 3,645) × 805 = 4.16 GiB`: still under
   the 4.80 GiB hard fail, but **over** the 3.20 GiB target by ~0.96 GiB.
2. **Row 6 has exactly one lever.** It is a per-chunk cap. Rule 3's "near
   radius, then vertical shell" moves rows 1/2, not row 6. The pre-registered
   sacrifice order therefore has only **mesh compression** for this row; a
   64³ profile cut cannot fix it.

**Recommended immediate record:** add a dated erratum to
[`m2-exit.md`](m2-exit.md) §3 and update the repeated numbers in
`task-list.md`, `m1-gate-decision.md` and the roadmap. Keep the archived M2 run
append-only; record the corrected production measurement in the next run.

---

## 2. Why an 8 B vertex is the primary lever

The current ADR-010 vertex is 16 B: `local u16×3`, `material u16`, `face u8`,
`ao[4] u8`, 3 reserved bytes. Four vertices + six `u16` indices = **76 B/quad**.

An 8 B vertex is the **minimum legal stride for this field set** on wgpu:
`array_stride` must be a multiple of 4, and 5–7 B cannot hold the required
fields (or be a legal stride). It is also
**lossless** for every value the mesher actually emits:

- `local` is `0..=CHUNK_DIM`; 128 fits `u8` (129 values). A 7-bit packing does
  not fit 129, so do not try 21-bit xyz.
- AO is exactly four levels, `[0, 85, 170, 255]`
  (`crates/engine/mesh/src/greedy.rs:186`), so 2 bits per corner is exact.
  ADR-010 rejected a packed vertex on the grounds that quantising position/AO
  would change the merge key; that objection does not apply to this encoding.
- `face` is 0..5, material is a dense atlas slot used as 8 bits by the shader
  already (`chunk.wgsl` computes `material % 16` / `material / 16` on a 256²
  atlas).

### Recommended layout

| Bytes | Field | Encoding |
|---:|---|---|
| 0–2 | `local.x/y/z` | `u8` each, `0..=CHUNK_DIM` |
| 3 | `material` | `u8` atlas slot, `0..=255`; encode **after** `resolve_materials` |
| 4–5 | `packed` | `u16`: bits 0–2 face, bits 3–10 AO levels (2 bits × 4), bits 11–15 version/reserved (zero) |
| 6–7 | reserved | zero |

- `size_of::<GpuVertex>() == 8`, `align_of == 4`.
- GPU: `Uint8x4` at offset 0 (`local.xyz`, `material`), `Uint16x2` at offset 4
  (`packed`, `reserved`), stride 8.
- Shader: `material = local_material.w`, `face = packed & 7u`,
  `ao_level = (packed >> (3 + 2*(vertex_index % 4))) & 3u`,
  `ao = f32(ao_level) / 3.0`. The four-vertex-per-quad corner selection and
  `base_vertex % 4` are unchanged.
- Encoder invariants: material ≤ 255 (block ids can be 65,535 before resolve);
  AO values are canonical 0/85/170/255; `debug_assert` both.

**Quad size: 4 × 8 + 6 × 2 = 44 B** (42.1% smaller).

### Projected result

| | 128³ | 64³ |
|---|---:|---:|
| Production p95 quads | 49,112 | 12,449 |
| Vertex + index at 44 B | 2,110 KiB | 535 KiB |
| Meshlet records + block table | +9 KiB | +1.1 KiB |
| **Honest p95** | **2,119 KiB** | **536 KiB** |
| Cap | 2,400 KiB | 600 KiB |
| **Margin** | **281 KiB (11.7%)** | **64 KiB (10.7%)** |
| Row 1 total (p95 × 805 / 6,440) | **2.99 GiB** (target 3.20) | 5.89 GiB (still fails) |

The 128³ Minimum now passes both the mesh cap and the 3.20 GiB target. The 64³
Minimum fallback still fails at 44 B; see §6 for the strategic note.

---

## 3. Options considered

| Option | Bytes/quad | 128³ p95 | Verdict |
|---|---:|---:|---|
| **8 B packed vertex** (this plan) | **44 B** | **2,119 KiB** | **Phase 1; recommended** |
| Instanced quads (8 B per quad record) | 8 B | ~384 KiB | Phase 2; best long-term, higher risk |
| Per-quad constants + 3 B positions | ~28 B | ~1,343 KiB | Dominated by instanced; skip |
| 12 B vertex (u16 local + u8 material + face/AO) | 60 B | ~2,877 KiB | Still over cap; reject |
| Index compression only (16 B vertex, 64 B/quad) | 64 B | ~3,069 KiB | Not enough; reject |
| Vertex sharing/dedup within a meshlet | 32–40 B | ~1,500–1,900 KiB | Uncertain with AO/face/material; defer |
| More aggressive AO merge | fewer quads | fewer | Visual artifacts; reject |

The 8 B vertex is the lowest-risk change that passes on the measured production
mesh. It keeps geometry, AO, materials and shading identical; only the byte
encoding and the vertex-shader unpack change.

### Phase 2: instanced quads (8 B per quad)

If the real-save density erodes the Phase 1 margin, the next lever is a **quad
record** and a shared index pattern:

- One 8 B record per quad: `origin xyz u8` (3 B), `material u8` (1 B),
  `face 3 bits | extent_u 5 bits | extent_v 5 bits | ao 8 bits` (2 B), 2 B spare.
- A single 12 B static index buffer `[0,1,2,0,2,3]`, shared by every meshlet.
- One draw per meshlet: 6 vertices × `quad_count` instances, with the record
  stream as a `VertexStepMode::Instance` buffer, or a storage-buffer pull.
- The vertex shader reconstructs the four corner positions from
  `origin + face + extent` and selects its corner from `vertex_index % 6`.
- Result: **8 B/quad** (~384 KiB at 128³), below even the 600 KB lower bound.
- Cost: GPU-side corner/winding reconstruction, a new draw contract, and
  meshlet records. It belongs after the M3-02/M3-05 draw path exists, or as an
  explicit fallback if Phase 1 misses its trigger.
- Fallback effect: 64³ Minimum total drops to ~520 KiB per chunk (~3.2 GiB
  before the M6-09a v1-pool removal, ~2.2 GiB after), which re-opens the
  fallback profile.

Do **not** do a 12 B vertex or per-quad-constants-without-instancing: both are
more complex than 8 B and still weaker.

---

## 4. Measurement fix (do this first)

The gate must measure the **production AO mesh**, not the reference:

- In `crates/bench/src/gate/memory.rs`, after expanding the chunk, run
  `for_each_block` + `greedy_block` and count quads (no `mesh_block` needed for
  bytes). Cost is ~0.3 s for 9 chunks at 128³, negligible.
- `mesh` component = `quads × 44 + meshlet records + block table`; add
  `mesh_reference = reference_quads × 76` as an informational component that is
  **not** in `total`.
- `total` (row 1) uses the production mesh. Bump the memory schema and the gate
  report schema; keep `#[serde(default)]` on new fields so archived JSON still
  loads.
- Row 6 string: `"p95 X KiB (max Y); production AO × 44 B (reference Z KiB × 76 B)"`.
- Add `MESH_BYTES_PER_QUAD` sourced from `GpuVertex::SIZE` so the row cannot
  drift from the format.
- Archive new 128³ and 64³ runs after the change. Keep the M1/M2 archived runs
  append-only; add the erratum to `m2-exit.md` and a new history entry.
- Re-run the gate after Phase 1; the acceptance number is the **measured
  production** p95, not a projection.

---

## 5. Implementation plan

### Phase 0 — record the true baseline (0.5–1 d)

1. Add the production mesh count to `gate memory` (above), schema bump.
2. Re-run 128³ Minimum and 64³; archive both.
3. Append the M2 erratum: production 3,645 KiB, miss 1,245 KiB, row 1 4.16 GiB.
4. Update the repeated numbers in `task-list.md`, `m1-gate-decision.md`,
   `voxel-engine-roadmap.md`, `performance-targets.md`.

### Phase 1 — 8 B vertex (≈4 d)

1. **ADR-014** (`ADR-011..013` are taken): *Packed 8 B mesh vertex and 44 B
   indexed quad*. Supersede ADR-010 §4/§5/§6; leave the grain, apron, meshlet
   and draw decisions intact. Add a dated appendix to the frozen
   pre-registration: row 6 basis 76 → 44 B, cap unchanged. Update ADR-001,
   `performance-targets.md` and the roadmap's current-format statements.
2. **Codec**: keep the logical `Vertex` 16 B for goldens/determinism; add
   `GpuVertex` (8 B) with `encode_gpu`/`decode_gpu`, exhaustive AO×face tests,
   material-300 rejection, and `const` size/align asserts. `Mesh` gains a
   GPU byte length (44 B/quad) while its logical byte length stays 76 B.
3. **Viewer**: encode `GpuVertex` after `resolve_materials`; fix
   `chunk_content.rs` `base = vertices.len() / 8`; `from_parts` takes the
   format/stride; `chunk_render` uses two attributes at stride 8; `chunk.wgsl`
   unpacks. Keep the **naive M1.5 path on its own 16 B layout** and point the
   screenshot example at it, so the M1.5 screenshots/goldens do not churn.
4. **Tests**: vertex codec (exhaustive), logical mesh shape, GPU byte size,
   goldens unchanged (logical format), and a **frame-parity test**: render one
   fixed scene through the 16 B and 8 B pipelines offscreen and assert the RGBA
   buffers are equal. This is the real "visual parity" proof. Gate it like
   `validation.rs` (`VOXEL_REQUIRE_GPU=1` in CI).
5. **Gate**: production mesh at 44 B + records; row 6 and row 1 must pass at
   both edges; re-run and archive.

### Phase 2 — instanced quads (contingent, ≈2–3 d)

Only if a trigger below fires. Implement the 8 B quad record, shared index
pattern, per-instance draw and GPU corner reconstruction; add a parity test and
a new gate run. This is the option that also re-opens 64³ Minimum (520 KiB,
~3.19 GiB after the M6-09a v1-pool removal).

---

## 6. Risk, triggers and stop conditions

**The measured fixture is one terrain archetype.** The 11.7% Phase 1 margin is
thin against a real save with more caves, overhangs and mixed-material bands.
Require a **15% margin for a format decision**, and pre-register these triggers:

- **Trigger A (fixture):** if the re-measured 128³ production p95 at 44 B is
  **> 2,160 KiB** (90% of cap), do not close the task; run Phase 2.
- **Trigger B (real save):** if the first real save's row 11 p95 exceeds
  **2,160 KiB**, Phase 2 is mandatory before row 11 closes.
- **Trigger C (64³ fallback):** if 64³ Minimum must stay viable, Phase 1 does
  not suffice (5.89 GiB); Phase 2 brings it to ~3.2 GiB before the M6-09a
  v1-pool removal and ~2.2 GiB after.

**Stop conditions:**

- A parity test that is not byte-equal is a failure of the AO unpack; fix it,
  do not accept a 1-LSB difference.
- Do not close on a projection. Row 6 must be a measured production number in
  an archived run.
- If 8 B is rejected at review, do **not** fall back to 12 B or near radius.
  Go straight to instanced quads (Phase 2); those are the only options that
  can move row 6 and keep the 128³ edge.

**Other risks and mitigations:**

| Risk | Mitigation |
|---|---|
| Material > 255 before resolve | Encode only after `resolve_materials`; `debug_assert`; add a test with material 300 |
| Non-canonical AO | Assert AO ∈ {0,85,170,255}; exhaustive codec test |
| Naive/M1.5 path breaks | Keep a separate 16 B naive layout; screenshot example untouched |
| Golden hash churn | Keep logical `Vertex` 16 B; goldens stay green; add a GPU byte hash test |
| Stale `/16` in the viewer | Compile-time `GpuVertex::SIZE`; test the upload byte count and vertex count |
| Gate schema break | `#[serde(default)]` new fields; append-only archived runs |
| 64³ Minimum still fails | Record explicitly; Phase 2 trigger C |

---

## 7. Acceptance

- `cargo test -p voxel-mesh` (logical codec, mesh shape, AO, determinism,
  goldens at both edges) and `cargo test -p voxel-viewer` (parity + validation).
- Frame parity: 16 B vs 8 B offscreen RGBA byte-equal.
- Gate 128³ Minimum: row 6 **PASS** with p95 ≤ 2,400 KiB (target ≤ 2,160 KiB),
  row 1 ≤ 3.20 GiB, recorded in an archived run.
- Gate 64³: row 6 PASS ≤ 600 KiB.
- Throughput within noise of 34.3 chunks/s at 128³; scratch budget unchanged
  (output buffers shrink, the 64 KiB scratch does not).
- ADR-014 merged, pre-registration appendix added, M2 erratum recorded, CI
  goldens/parity job green.

**Estimated effort:** Phase 0 0.5–1 d; Phase 1 ~4 d (codec 0.75, viewer 1,
naive split 0.5, parity 1, gate/re-run 1); Phase 2 2–3 d if triggered. M3's
9-day budget needs to absorb this explicitly (a new `M3-00a`/`M3-00b` before
`M3-01`), because M3-02's arena is built against the vertex stride.

---

## 8. Decisions requested

1. **Material width:** `u8` (recommended; matches the shader's 8-bit use) or
   `u16` (costs 1 B, pushes the quad to 48 B and still passes at 2.31 MiB, but
   with only 90 KiB margin).
2. **Logical vs GPU vertex:** keep the 16 B logical `Vertex` for goldens
   (recommended) or shrink the one type and regenerate every golden.
3. **Phase 2 timing:** trigger-based after a real save (recommended) or
   implement instanced quads now for the larger margin and 64³ viability.
4. **Placement:** new `M3-00a`/`M3-00b` before `M3-01` (recommended) or a
   separate pre-M3 task.
