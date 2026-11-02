# M1 gate scale-up (M1-40)

**Fixture:** `m2-128` (provisional)

**Manifest:** `benchmarks/fixtures/m2-128/manifest.json`

**Save:** `benchmarks/fixtures/m2-128/save`

**World hash:** `03c687a5d0967d1e24ec1b7d4e8ef3edd4da8e326cecf2bb45fcb57d042b14be`

> **Provisional fixture.** Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.

**Source edge:** 128³ (7 bits), 2,097,152 voxels/chunk, 512 light cells, 512 probes, 8 levels

**Target edge:** 128³ (7 bits), 2,097,152 voxels/chunk, 512 light cells, 512 probes, 8 levels

**Profile:** `default` — N128 = 3,217 chunks at the target edge

**Loaded volume:** N64 = 25,736; N128 = 3,217; 25,736 = 8 × N128

## Measured 64³ input (p95)

| Component | Measured |
|---|---:|
| Edit upload | 261 ns |
| Edit fence | 21 ns |
| Edit flip | 30 ns |
| Edit fixed | 311 ns |
| Edit COW | 1.563 µs |
| Edit light | 4.389 µs |
| Edit dirty | 6.012 µs |
| Edit total | 6.923 µs |
| Probe select | 605.149 µs |
| Probe rebake | 143.422 µs |
| Probe upload | 70 ns |
| Probe update | 744.474 µs |
| Memory v1 pool/chunk | 657.7 KiB |
| Memory total/chunk | 3.80 MiB |

**Measured work:** 448 B/edit uploaded, 384 B/edit COW, 27 light cells/edit, 1 affected probes/edit, 68 rays/edit, 138,636 nodes/chunk

## 128³ projection

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit fixed | 311 ns | 311 ns | 8.00 ms |
| Edit dirty | 6.012 µs | 6.012 µs | — |
| Edit total | 6.923 µs (PASS, 7.99 ms) | 6.923 µs (PASS, 7.99 ms) | 8.00 ms |
| Probe update | 744.474 µs (PASS, 3.26 ms) | 744.474 µs (PASS, 3.26 ms) | 4.00 ms |
| Memory projected | 11.93 GiB (MISS) | 11.93 GiB (MISS) | 4.80 GiB |
| DAG nodes/chunk | 138,636 (PASS) | 138,636 (PASS) | 1,500,000 |

**Optimistic memory:** 3.80 MiB per chunk × 3,217 chunks = 11.93 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (MISS).

**Pessimistic memory:** 3.80 MiB per chunk × 3,217 chunks = 11.93 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (MISS).

**Light cells:** optimistic 27 (one dirty block cell plus apron); pessimistic 27 (the whole target chunk).

**Probe work:** optimistic 1 probes, 68 rays; pessimistic 1 probes, 68 rays.


- The model is analytic, not a 128³ measurement. Each p95 component is projected independently and the projected components are summed; the sum of p95s is not itself a p95 of sums.

- Fixed = upload + fence + flip. The fence and flip are per batch and unchanged. Upload follows the COW path, which grows from 8 to 8 levels (1.000), not a whole-chunk transfer. Both projections share the same fixed term.

- Optimistic dirty = one COW path (1.000 scale) and the measured 27 light cells, because a block is still 16³ voxels at the larger edge. The mesh and the affected probe set grow with surface area (×1).

- Pessimistic dirty spans the chunk: COW grows with volume (×1), light evaluates all 512 target block cells (18.96× the measured 27), the probe update rebakes the whole 512 probe grid (32,768 rays), and every memory component grows with volume.

- Memory is compared at equal loaded volume: N64 = 25,736 and N128 = 3,217 (factor 8). The optimistic projection scales the mesh by area and the other components by volume; the pessimistic projection scales every component by volume.

- Fed the measured 64³ inputs and asked for 64³, the model returns those inputs unchanged; that identity is asserted by the unit tests. The projection starts only at a larger edge.

- Fixture limitation, verbatim: Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen. Re-run this projection after the real save before M1-42 quotes it.
