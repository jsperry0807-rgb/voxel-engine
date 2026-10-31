wrote target/phase0-64-run/gate/scaleup.json
# M1 gate scale-up (M1-40)

**Fixture:** `m1-representative` (provisional)

**Manifest:** `benchmarks/fixtures/m1-representative/manifest.json`

**Save:** `benchmarks/fixtures/m1-representative/save`

**World hash:** `d077250c2a84b0b42f3fd852c76ed7e43c61d21d21c41381fc8808331d8332e5`

> **Provisional fixture.** Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.

**Source edge:** 64³ (6 bits), 262,144 voxels/chunk, 64 light cells, 64 probes, 7 levels

**Target edge:** 128³ (7 bits), 2,097,152 voxels/chunk, 512 light cells, 512 probes, 8 levels

**Profile:** `default` — N128 = 3,217 chunks at the target edge

**Loaded volume:** N64 = 25,736; N128 = 3,217; 25,736 = 8 × N128

## Measured 64³ input (p95)

| Component | Measured |
|---|---:|
| Edit upload | 211 ns |
| Edit fence | 20 ns |
| Edit flip | 21 ns |
| Edit fixed | 251 ns |
| Edit COW | 1.222 µs |
| Edit light | 3.267 µs |
| Edit dirty | 4.318 µs |
| Edit total | 4.839 µs |
| Probe select | 105.170 µs |
| Probe rebake | 38.172 µs |
| Probe upload | 50 ns |
| Probe update | 144.104 µs |
| Memory v1 pool/chunk | 164.0 KiB |
| Memory total/chunk | 1.37 MiB |

**Measured work:** 360 B/edit uploaded, 304 B/edit COW, 27 light cells/edit, 1 affected probes/edit, 67 rays/edit, 31,095 nodes/chunk

## 128³ projection

| Term | Optimistic | Pessimistic | Limit |
|---|---:|---:|---:|
| Edit fixed | 282 ns | 282 ns | 8.00 ms |
| Edit dirty | 4.664 µs | 71.728 µs | — |
| Edit total | 4.946 µs (PASS, 8.00 ms) | 72.010 µs (PASS, 7.93 ms) | 8.00 ms |
| Probe update | 573.568 µs (PASS, 3.43 ms) | 73.42 ms (MISS, -69.42 ms) | 4.00 ms |
| Memory projected | 21.64 GiB (MISS) | 33.00 GiB (MISS) | 4.80 GiB |
| DAG nodes/chunk | 248,760 (PASS) | 248,760 (PASS) | 1,500,000 |

**Optimistic memory:** 6.89 MiB per chunk × 3,217 chunks = 21.64 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (MISS).

**Pessimistic memory:** 10.50 MiB per chunk × 3,217 chunks = 33.00 GiB; target 3.20 GiB (MISS), hard fail 4.80 GiB (MISS).

**Light cells:** optimistic 27 (one dirty block cell plus apron); pessimistic 512 (the whole target chunk).

**Probe work:** optimistic 4 probes, 268 rays; pessimistic 512 probes, 32,768 rays.


- The model is analytic, not a 128³ measurement. Each p95 component is projected independently and the projected components are summed; the sum of p95s is not itself a p95 of sums.

- Fixed = upload + fence + flip. The fence and flip are per batch and unchanged. Upload follows the COW path, which grows from 7 to 8 levels (1.143), not a whole-chunk transfer. Both projections share the same fixed term.

- Optimistic dirty = one COW path (1.143 scale) and the measured 27 light cells, because a block is still 16³ voxels at the larger edge. The mesh and the affected probe set grow with surface area (×4).

- Pessimistic dirty spans the chunk: COW grows with volume (×8), light evaluates all 512 target block cells (18.96× the measured 27), the probe update rebakes the whole 512 probe grid (32,768 rays), and every memory component grows with volume.

- Memory is compared at equal loaded volume: N64 = 25,736 and N128 = 3,217 (factor 8). The optimistic projection scales the mesh by area and the other components by volume; the pessimistic projection scales every component by volume.

- Fed the measured 64³ inputs and asked for 64³, the model returns those inputs unchanged; that identity is asserted by the unit tests. The projection starts only at a larger edge.

- Fixture limitation, verbatim: Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen. Re-run this projection after the real save before M1-42 quotes it.
