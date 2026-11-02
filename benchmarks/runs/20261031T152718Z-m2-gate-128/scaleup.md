wrote target/m2-gate/128/scaleup.json
# M1 gate scale-up (M1-40)

**Fixture:** `m2-128` (provisional)

**Manifest:** `benchmarks/fixtures/m2-128/manifest.json`

**Save:** `benchmarks/fixtures/m2-128/save`

**World hash:** `03c687a5d0967d1e24ec1b7d4e8ef3edd4da8e326cecf2bb45fcb57d042b14be`

> **Provisional fixture.** Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/specs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.

**Source edge:** 128³ (7 bits), 2,097,152 voxels/chunk, 512 light cells, 512 probes, 8 levels

**Target edge:** 128³ (7 bits), 2,097,152 voxels/chunk, 512 light cells, 512 probes, 8 levels

**Profile:** `minimum` — N128 = 805 chunks at the target edge

**Loaded volume:** N64 = 6,440; N128 = 805; 6,440 = 8 × N128

## Measured 64³ input (p95)

| Component | Measured |
|---|---:|
| Edit upload | 350 ns |
| Edit fence | 30 ns |
| Edit flip | 50 ns |
| Edit fixed | 431 ns |
| Edit COW | 2.094 µs |
| Edit light | 6.452 µs |
| Edit dirty | 8.146 µs |
| Edit total | 9.378 µs |
| Probe select | 645.596 µs |
| Probe rebake | 144.484 µs |
| Probe upload | 150 ns |
| Probe update | 774.670 µs |
| Memory v1 pool/chunk | 657.7 KiB |
| Memory total/chunk | 4.19 MiB |

**Measured work:** 448 B/edit uploaded, 384 B/edit COW, 27 light cells/edit, 1 affected probes/edit, 68 rays/edit, 138,636 nodes/chunk

## 128³ projection

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit fixed | 431 ns | 431 ns | 8.00 ms |
| Edit dirty | 8.146 µs | 8.146 µs | — |
| Edit total | 9.378 µs (PASS, 7.99 ms) | 9.378 µs (PASS, 7.99 ms) | 8.00 ms |
| Probe update | 774.670 µs (PASS, 3.23 ms) | 774.670 µs (PASS, 3.23 ms) | 4.00 ms |
| Memory projected | 3.29 GiB (PASS) | 3.29 GiB (PASS) | 4.80 GiB |
| DAG nodes/chunk | 138,636 (PASS) | 138,636 (PASS) | 1,500,000 |

**Optimistic memory:** 4.19 MiB per chunk × 805 chunks = 3.29 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (PASS).

**Pessimistic memory:** 4.19 MiB per chunk × 805 chunks = 3.29 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (PASS).

**Light cells:** optimistic 27 (one dirty block cell plus apron); pessimistic 27 (the whole target chunk).

**Probe work:** optimistic 1 probes, 68 rays; pessimistic 1 probes, 68 rays.


- The model is analytic, not a 128³ measurement. Each p95 component is projected independently and the projected components are summed; the sum of p95s is not itself a p95 of sums.

- Fixed = upload + fence + flip. The fence and flip are per batch and unchanged. Upload follows the COW path, which grows from 8 to 8 levels (1.000), not a whole-chunk transfer. Both projections share the same fixed term.

- Optimistic dirty = one COW path (1.000 scale) and the measured 27 light cells, because a block is still 16³ voxels at the larger edge. The mesh and the affected probe set grow with surface area (×1).

- Pessimistic dirty spans the chunk: COW grows with volume (×1), light evaluates all 512 target block cells (18.96× the measured 27), the probe update rebakes the whole 512 probe grid (32,768 rays), and every memory component grows with volume.

- Memory is compared at equal loaded volume: N64 = 6,440 and N128 = 805 (factor 8). The optimistic projection scales the mesh by area and the other components by volume; the pessimistic projection scales every component by volume.

- Fed the measured 64³ inputs and asked for 64³, the model returns those inputs unchanged; that identity is asserted by the unit tests. The projection starts only at a larger edge.

- Fixture limitation, verbatim: Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/specs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen. Re-run this projection after the real save before M1-42 quotes it.
