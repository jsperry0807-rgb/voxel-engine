# M2 ray-test occluder (M2-25)

**Status:** implemented behind `--features raytest`; `occlusion.raytest` defaults
off (ADR-006). M2-26 comparison complete: the default stays flood-fill.
**Code:** `crates/engine/mesh/src/raytest.rs`, tests
`crates/engine/mesh/tests/raytest.rs` and `raytest_compare.rs`.
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

## 100-pose comparison (M2-26)

**World:** a 16³-block SVDAG world with terrain, a slit wall, a pillar and a
sealed cave pocket (carved below the minimum terrain height, so its roof is
always solid). 100 deterministic poses: 60 above ground, 20 in front of the
slit, 20 inside the cave. Horizon 4 blocks; release build on the reference host
(AMD Ryzen 5 7600).

The M1-33 fixture is a wider procedural world; building SVDAG chunks for its
100 poses at this horizon needs about two thousand chunks, which is a benchmark,
not a unit-test-sized sweep. This report substitutes a compact SVDAG world with
the same feature classes. The visible/delta counts below are exact for this
world (the test pins them); the cost decision is driven by the horizon scaling,
not by the fixture's exact geometry.

```bash
cargo test --release -p voxel-mesh --features raytest --test raytest_compare -- --nocapture
```

| Metric (64³ compiled edge) | Block flood fill | SVDAG ray-test | Ratio |
| --- | ---: | ---: | ---: |
| p50 per pose | 24.8 µs | 799.1 µs | 32.3× |
| p95 per pose | 28.3 µs | 892.9 µs | 31.5× |
| Visible blocks per pose | 687.9 | 471.0 | — |
| Delta (in-world) | — | 20,227 blocks over 100 poses (**35.3%** of in-world fill-visible) | — |
| Voxel steps per pose | — | 50,794 | — |

At the pre-registered 128³ edge the visible counts are identical and the cost
ratio is 33.4× p50 / 37.2× p95 (ray p50 833.4 µs, p95 1,084.8 µs). Both p50 and
p95 move by a few percent between runs; treat the table as one representative
run, not a fixed constant.

Every pose keeps the ray set a subset of the fill. The delta is the set of
blocks the fill includes that the ray-test does not reach; it is not measured
against a ground-truth oracle. The ray-test costs roughly **32× the fill even at
a 4-block horizon**: it casts one ray per block and walks voxels per ray
(`O(h⁴)`), while the fill is `O(h³)`. The gap grows with the horizon, so the
128 m production horizon is not affordable per frame.

### Recommendation: keep the flood fill as the default

- **Cost:** ~32× slower at horizon 4 and superlinear in the horizon; a per-frame
  128 m pass is out of reach.
- **Accuracy:** the delta is the connectivity over-inclusion ADR-006 chose on
  purpose; the escalation trigger is the M1-34 granularity gate, not a raw count
  of extra blocks. No oracle says every removed block is invisible.
- **Safety:** the default non-air `SvdagScene` treats any non-air voxel as
  opaque, while the fill stops only at a fully opaque block. Water, glass,
  leaves and partially filled blocks (slabs, stairs) can therefore be culled
  behind; a default would need a material-aware scene whose predicate matches
  the fill's rule.
- **Keep the flag:** `occlusion.raytest` stays off by default, available for the
  far-field/shadow-ray path and for experiments. M2-27 wires the content
  pipeline, not this flag.
