# M2 ray-test occluder (M2-25)

**Status:** implemented behind `--features raytest`; `occlusion.raytest` defaults
off (ADR-006).
**Code:** `crates/engine/mesh/src/raytest.rs`, tests
`crates/engine/mesh/tests/raytest.rs`.
**Verify:** `cargo test -p voxel-mesh --features raytest`.

## What it does

The primary occluder is the M1-33 camera-seeded block flood fill: it answers
**connectivity** and over-includes through any one-block gap and around corners.
The ray-test is the flagged escalation. For every block in the same
`(2·horizon + 1)³` Chebyshev cube around the camera block, it casts one ray from
the camera to the block centre and keeps the block visible only when the segment
is clear.

- `ray_test(camera, horizon, is_opaque)` takes a voxel-level opacity predicate.
- `SvdagScene` implements that predicate over resident `Svdag` chunks: a ray
  walks world voxels with an Amanatides–Woo DDA, maps each cell to its chunk and
  local coordinate, and reads the SVDAG. Missing chunks read as air.
- A block's own voxels are not tested, so an opaque boundary block is visible,
  matching the fill's leaf rule. The camera block is always visible; if the
  camera sits inside a solid block, the ray skips that block's voxels (the same
  "resolve visible" degenerate-seed rule as the fill).
- `RayTestFill` uses the same one-bit-per-block layout as the fill, with
  `is_visible`/`is_occluded`, `visible_blocks`, `visible_count`,
  `occluded_count`, plus `rays_cast` and `voxel_steps` for the cost comparison.
- `SvdagScene::new` treats every non-air voxel as opaque.
  `SvdagScene::with_opaque_material` takes the material predicate instead;
  water, glass and leaves must stay transparent or the ray-test culls what is
  visible behind them (a hole, not overdraw).
- `RayTestConfig` mirrors the `occlusion.raytest` key (ADR-006, roadmap M2-25):
  `enabled: false`, `horizon_blocks: 128` by default. The cargo feature gates the
  code; wiring the config into the traversal is M2-26/M2-27 work.

## Semantics

Line of sight is stricter than connectivity: if a ray reaches a block through
air, the traversed blocks are connected, so the ray-test visible set is a subset
of the fill's. The scene-comparison tests assert that relation. With opaque
materials the delta is the set of blocks the fill over-includes; with transparent
materials the fill and the ray-test must use the same material predicate, or the
ray-test can cull genuinely visible geometry behind water or glass.

## Fixture delta

Measured at the compiled chunk edge with a 9³ horizon (4 blocks) on single-chunk
fixtures, `cargo test -p voxel-mesh --features raytest -- --nocapture` (the
12-test suite also covers water, negative chunks, multiple chunks, a camera
inside solid and a camera on a block boundary):

| Fixture | Fill visible | Ray visible | Delta | Rays | Voxel steps |
| --- | ---: | ---: | ---: | ---: | ---: |
| Empty cube (open field) | 729 | 729 | 0 | 729 | 66,620 |
| Closed room inside a 3³ shell | 7 | 7 | 0 | 729 | 11,948 |
| Solid pillar between camera and air behind it | 729 | 566 | **163 (22.4%)** | 729 | 54,032 |
| Finite floor patch, camera above it | 729 | 645 | 84 (11.5%) | 729 | 63,045 |

The pillar and floor deltas are the point: connectivity walks around the
obstacle, line of sight does not. Exhaustive scene agreement (empty cube, closed
room) shows the ray-test is not merely stricter by construction.

## Cost

One ray per block, one DDA per ray: `O(horizon⁴)` voxel steps for a full cube
(729 rays / ~54k steps at a 4-block horizon; the cube grows as the cube of the
horizon, the ray length as the horizon). That is why this stays a flag. M2-26
owns the 100-pose accuracy/cost table and the decision recommendation; unless the
ray-test wins on **both** accuracy and cost, the flood fill stays the default
(ADR-006 kill criterion).
