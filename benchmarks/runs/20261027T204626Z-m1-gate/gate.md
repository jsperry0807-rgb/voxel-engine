# M1 gate report (M1-39)

**Gate:** **FAIL** — 8 pass, 2 fail, 2 not measured

**Machine:** RTX 4060 reference — AMD Ryzen 5 7600, 6 cores / 12 threads, NVIDIA GeForce RTX 4060, 8188 MiB VRAM, 32 GiB RAM, 1920 × 1080, Linux (primary)

**Host:** AMD Ryzen 5 7600 6-Core Processor / linux / x86_64 — reference host: yes; GPU probed: no

**Profile:** `default` (gated)

**Fixture:** `m1-representative` (provisional) — provisional: yes

**Generated:** unix 1793134001

| # | Pre-registered row | Requirement | Measured | Margin | Result |
|---|---|---|---|---|---|
| 1 | VRAM budget for voxel data | ≤ 4.80 GiB hard fail (3.20 GiB target) | 24.76 GiB (p95 1008.9 KiB × 25,736 chunks) | -19.96 GiB to hard fail | **FAIL** |
| 2 | Target loaded chunk count | N64 = 25,736 = 8 × N128; table predicted N64 = 25,736, N128 = 3,216 | N64 = 25,736 at 64³; N128 = 3,217 at 128³ equal volume | 0 chunks (equal-volume identity) | **PASS** |
| 3 | SVDAG node budget/chunk | ≤ 200,000 nodes/chunk at 64³ | max 36,946 nodes/chunk (p95 32,405); 64³ build (64 voxels/edge) | +163,054 nodes | **PASS** |
| 4 | Light volume/chunk | = 1,024 B/chunk at 64³ (64 cells) | 1,024 B/chunk (64 cells × 16 B); p50 1,024 B | 0 B | **PASS** |
| 5 | Probe grid/chunk | = 64 probes = 768 B/chunk at 64³ | 64 probes/chunk, 768 B; 4,096 rays/full rebake | 0 B | **PASS** |
| 6 | Mesh buffer/chunk | ≤ 600 KiB/chunk at 64³ | p95 753 KiB (max 775 KiB); greedy estimate × 76 B | -153.1 KiB | **FAIL** |
| 7 | Edit latency | p95 ≤ 8.00 ms | p95 2.485 µs (p50 1.322 µs, max 15.219 µs); 1,000 edits | 8.00 ms | **PASS** |
| 8 | Probe-update budget | p95 ≤ 4.00 ms per edited chunk | p95 143.351 µs (p50 54.303 µs, max 288.318 µs); 1,000 edited chunks | 3.86 ms | **PASS** |
| 9 | Culling granularity | ≤ 15% traversed-but-invisible | not measured by this run | n/a | **NOT MEASURED** |
| 10 | Safety factor | hard fail = 4,913 MiB | 4,913 MiB = ceil(1.5 × 3,275 MiB) | 0 MiB | **PASS** |
| 11 | Measurement dataset | real populated save | m1-representative (provisional; synthetic, provisional) | n/a | **NOT MEASURED** |
| 12 | Fallback trigger | pre-registration §4 fallback order | rule 2 triggered by the measured rows | n/a | **PASS** |

## Row detail

1. **VRAM budget for voxel data** — pre-registered: 40% of 8,188 MiB = 3,275 MiB; hard fail at 60% = 4,913 MiB. Source: gate memory (M1-35) + gate residency (M1-36). Projected from the measured p95 total per chunk; the max chunk (1.03 MiB) would project to 25.84 GiB. The target row is 3.20 GiB (-21.56 GiB), the hard-fail row is 4.80 GiB (-19.96 GiB). The gate uses p95 by pre-registration, not the mean, and does not apply the 1.5× factor a second time.
2. **Target loaded chunk count** — pre-registered: Per profile and edge, table §2: N64 = 25,736, N128 = 3,216. Source: gate residency (M1-36). The gate asserts the frozen model's equal-volume identity, N64 = 8 × N128. The table's own N64/N128 are a prediction: measured 25,736 / 3,217 against 25,736 / 3,216 (deltas 0 / +1). Those deltas are the table's nearest-integer rounding and are reported, not folded back into the model. N64 feeds the memory projection.
3. **SVDAG node budget/chunk** — pre-registered: 64³ ≤ 200,000; 128³ ≤ 1,500,000 unique nodes. Source: gate memory (M1-35). The node count is the banded internal-node arena (`dag_node_bytes / 8`), so it is a hard per-chunk cap, not a p95. The other edge's budget is pre-registered but not measured by this build; M1-40 supplies the 128³ projection before M1-42.
4. **Light volume/chunk** — pre-registered: 64³: 64 cells = 1 KiB; 128³: 512 cells = 8 KiB. Source: gate memory (M1-35); ADR-004. One 16 B cell per block, fixed by the compiled chunk edge. The 128³ value is the frozen pre-registration target and is not built in M1.
5. **Probe grid/chunk** — pre-registered: 64³: 64 probes = 768 B; 128³: 512 probes = 6 KiB. Source: gate probes (M1-38) + gate memory (M1-35); ADR-005. One 12 B SH4 RGB probe per block at the near LOD. The update row measures the latency; this row checks both the fixed grid and the byte volume. The 128³ value is pre-registered, not built in M1.
6. **Mesh buffer/chunk** — pre-registered: 64³: 150–600 KB; 128³: 600 KB–2.4 MB. Source: gate memory (M1-35); M1-31 reference mesher. The estimate is greedy quads × 76 B and excludes vertex/index buffer overhead. The predicted lower bound is 150 KiB; a lower estimate is not a failure, but it is not a measured GPU buffer either. The gate uses p95 and reports max separately.
7. **Edit latency** — pre-registered: p95 ≤ 8 ms (consolidate + upload + fence + flip + dirty light). Source: gate edits (M1-37). Fixed = upload + fence + flip; dirty = COW + light. The fence and upload are the CPU side of the M4 protocol; the headless gate has no GPU device time, so this is a lower bound on a real device path, not a claim about GPU completion.
8. **Probe-update budget** — pre-registered: p95 ≤ 4 ms per edited chunk. Source: gate probes (M1-38). Update = select + rebake + upload. Only probes whose rays reach the dirty voxel are rebaked; the upload is the CPU staging copy and excludes the device transfer and the M4 fence/flip.
9. **Culling granularity** — pre-registered: ≤ 15% traversed-but-invisible at chunk boundaries @ max view. Source: M1-34 reference; no archived gate result. The reference instrumentation and its 100-pose test exist, but this generator consumes M1-35…M1-38 only. A granularity run must be attached before M1-41, or this row keeps the gate incomplete; it is not silently passed.
10. **Safety factor** — pre-registered: 1.5× on the memory budget. Source: pre-registration §3; performance-targets §1.1. This is an invariant on the frozen budget, not a second measurement. The factor survives hardware changes because the fraction (40%/60%) does; applying it twice would make the gate stricter than the pre-registration.
11. **Measurement dataset** — pre-registered: Real populated save; median, p95, max per chunk. Source: gate memory fixture (M1-28/M1-35). Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen. The distributions are measured (p50/p95/max per chunk), but the dataset is not admissible as the M1 gate result until a real populated save replaces it.
12. **Fallback trigger** — pre-registered: See §4; pre-registered; no renegotiation inside M1. Source: pre-registration §4. Rule 2: a voxel-memory row failed at 64³. Do not automatically stay at 64³. Run the M1-40 analytic 128³ projection; if 128³ clears, the failure is an argument for the upgrade, not against it.

## Fallback

Rule 2: a voxel-memory row failed at 64³. Do not automatically stay at 64³. Run the M1-40 analytic 128³ projection; if 128³ clears, the failure is an argument for the upgrade, not against it.

## Notes

- The twelve rows are the frozen table in docs/pre-registration.md §3. Every verdict is computed from a measured value and its limit; no row is typed PASS. A row the M1-35…M1-38 harnesses do not measure is NOT MEASURED, not green.
- The memory projection is the measured p95 per-chunk total multiplied by the loaded chunk count at the edge this build uses. It applies the frozen 60% hard-fail line (4,913 MiB); the 1.5× safety factor is already inside that line, so it is not applied twice.
- The headless gate does not open a wgpu adapter. GPU identity is quoted from docs/reference-machine.md and is not re-probed; the host CPU, OS and architecture are read from the running process.
- The fixture is provisional until the first real save. Its distributions are a rehearsal for the gate and must not be quoted as the M1 result; the dataset row stays not measured until a real save replaces it.
- Culling granularity has the M1-34 reference and its 100-pose test, but this generator consumes M1-35…M1-38 only. Attach a granularity run before M1-41 or that row keeps the gate incomplete.
- The fallback is computed from the measured rows with the pre-registration §4 order. It is a preview for M1-42, not a renegotiation of any threshold.
