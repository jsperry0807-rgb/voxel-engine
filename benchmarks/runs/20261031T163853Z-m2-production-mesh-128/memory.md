wrote benchmarks/runs/20261031T163853Z-m2-production-mesh-128/memory.json
# M1 gate memory (M1-35)

**Fixture:** `m2-128` (provisional)

**Manifest:** `benchmarks/fixtures/m2-128/manifest.json`

**Save:** `benchmarks/fixtures/m2-128/save`

**Format:** 128³ (7 bits), 9 chunks, 7296044 non-air voxels, 0 overlay entries, 4 region files

**World hash:** `03c687a5d0967d1e24ec1b7d4e8ef3edd4da8e326cecf2bb45fcb57d042b14be`

> **Provisional fixture.** Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/specs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.

| Component | p50 | p95 | max | total |
|---|---:|---:|---:|---:|
| DAG | 983224 | 1126288 | 1126288 | 8785990 |
| DAG v1 pool (resident) | 592044 | 673460 | 673460 | 5335952 |
| Overlay | 0 | 0 | 0 | 0 |
| Light | 8192 | 8192 | 8192 | 73728 |
| Probes | 6144 | 6144 | 6144 | 55296 |
| Mesh (production AO) | 3082824 | 3737840 | 3737840 | 26345724 |
| Mesh reference (no AO, informational) | 2137196 | 2579896 | 2579896 | 18261964 |
| Total | 4694408 | 5551924 | 5551924 | 40596690 |

- DAG = 8 B/internal node (banded layout, full-run padding included) + 2 B/leaf material. The transitional v1 Svdag pool is resident for GPU and disk compatibility and is included in the total; M6-09a removes it.

- Overlay = 2048 B block table + 8704 B per edited 16³ block, the allocated GPU layout. This fixture has 0 pending overlay entries, so its overlay component is zero.

- Light = 8192 B/chunk (512 cells × 16 B); probes = 6144 B/chunk at the near LOD. Both are fixed by the compiled chunk edge, not by content.

- Mesh = production AO `mesh_block` output (76 B/quad) plus 16 B per non-empty block meshlet record and the per-chunk `u16` record table. The M1-31 no-AO reference is reported separately as `mesh_reference` and is not in the total; it is the historical gate basis. Both paths use `Apron::new()` (all-air), so interior chunk seams are counted as exposed and the number is a per-chunk upper bound at chunk borders.

- A/B root overhead is not part of this measurement; the DAG row counts the shared banded chunk, not the duplicated path runs.
