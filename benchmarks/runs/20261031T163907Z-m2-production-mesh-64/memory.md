wrote benchmarks/runs/20261031T163907Z-m2-production-mesh-64/memory.json
# M1 gate memory (M1-35)

**Fixture:** `m1-representative` (provisional)

**Manifest:** `benchmarks/fixtures/m1-representative/manifest.json`

**Save:** `benchmarks/fixtures/m1-representative/save`

**Format:** 64³ (6 bits), 72 chunks, 7036505 non-air voxels, 0 overlay entries, 4 region files

**World hash:** `d077250c2a84b0b42f3fd852c76ed7e43c61d21d21c41381fc8808331d8332e5`

> **Provisional fixture.** Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.

| Component | p50 | p95 | max | total |
|---|---:|---:|---:|---:|
| DAG | 37730 | 259246 | 305202 | 8663470 |
| DAG v1 pool (resident) | 31472 | 167968 | 193324 | 5672704 |
| Overlay | 0 | 0 | 0 | 0 |
| Light | 1024 | 1024 | 1024 | 73728 |
| Probes | 768 | 768 | 768 | 55296 |
| Mesh (production AO) | 474224 | 947732 | 1085320 | 28417620 |
| Mesh reference (no AO, informational) | 329156 | 771172 | 794200 | 21128456 |
| Total | 562946 | 1439794 | 1527984 | 42882818 |

- DAG = 8 B/internal node (banded layout, full-run padding included) + 2 B/leaf material. The transitional v1 Svdag pool is resident for GPU and disk compatibility and is included in the total; M6-09a removes it.

- Overlay = 256 B block table + 8704 B per edited 16³ block, the allocated GPU layout. This fixture has 0 pending overlay entries, so its overlay component is zero.

- Light = 1024 B/chunk (64 cells × 16 B); probes = 768 B/chunk at the near LOD. Both are fixed by the compiled chunk edge, not by content.

- Mesh = production AO `mesh_block` output (76 B/quad) plus 16 B per non-empty block meshlet record and the per-chunk `u16` record table. The M1-31 no-AO reference is reported separately as `mesh_reference` and is not in the total; it is the historical gate basis. Both paths use `Apron::new()` (all-air), so interior chunk seams are counted as exposed and the number is a per-chunk upper bound at chunk borders.

- A/B root overhead is not part of this measurement; the DAG row counts the shared banded chunk, not the duplicated path runs.
