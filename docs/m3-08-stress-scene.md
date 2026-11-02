# M3-08 stress scene: terrain and canopy variants

**Status:** landed 2026-10-31. The viewer's 256-chunk ring is now a seeded scene
with five terrain variants and two canopy densities, reproducible from one
number, and the viewer can capture its presented frame to a PNG. The committed
image is [`images/m3-08-stress-scene.png`](images/m3-08-stress-scene.png).

## Acceptance

| Requirement | Measured |
|---|---|
| 256-chunk stress scene | the existing 16×16 ring (`RING_CHUNKS = 256`), now with terrain and canopy variants |
| Terrain variants | meadow 110, forest 67, sand bar 29, highland 34, rocky 16 chunks at the default seed |
| Canopy variants | 0/some/many: sand, highland and rock never root a tree; meadow scatters oaks (chance 0.10 per 24² cell); forest is dense (0.85) |
| Reproducible from a seed | `VOXEL_SCENE_SEED=0xCAFE` (or decimal) selects the world; the same seed gives byte-identical volumes, and a different seed moves both the terrain and the trees (`tests/scene.rs`) |
| Screenshot | `docs/images/m3-08-stress-scene.png`, 1920×1080, captured from the presented swap-chain frame |
| Uses only M1.5-14 keys | `scene::atlas_slot` resolves to slots 1–23 only, checked face by face against `faces.ron`; M2.5 did not run, so no M2.5-31 key is used |

## Design

`crates/engine/viewer/src/scene.rs`:

```rust
let scene = Scene::from_env();          // VOXEL_SCENE_SEED, default 0x5EED1234
scene.fill_volume(chunk, &mut volume);  // block ids, not atlas slots
let apron = scene.apron(chunk);         // the same scene on the six planes
```

Everything is a pure function of `(seed, world position)`: value noise over
world columns plus integer hashes. There is no ordered state and no clock, so
two runs with one seed produce identical bytes and the seed is the only input.

### Terrain variants

`biome(x, z)` classifies a column from its **unflattened** altitude, moisture
and erosion noise:

| Variant | Condition | Surface stack |
|---|---|---|
| `Sandbar` | altitude ≤ 19 | 3 sand, 4 sandstone, stone |
| `Highland` | altitude ≥ 31 | 1 snow, stone |
| `Forest` | moisture > 0.62 | 1 grass, 3 dirt, stone; floor blended flat |
| `Rocky` | erosion > 0.78 | 3 gravel/cobblestone, stone |
| `Meadow` | otherwise | 1 grass, 3 dirt, stone |

Bedrock fills y = 0 in every column. Grass and log blocks resolve per face
(`grass_block_top` / `grass_block_side` / `dirt`, `oak_log_top` / `oak_log`), so
a chunk volume stores **block ids** and `Scene` resolves them at mesh time
through `chunk_content::build_resolved_with_apron`.

The classification reads the unflattened altitude on purpose: `height` lowers
forest lowlands, and classifying the lowered surface would turn forests into
sand bars.

### Canopy

Trees sit on a 24-column grid with ±6 columns of jitter:

- `Forest` cells plant with chance 0.85, `Meadow` with 0.10, every other variant
  with 0.
- Trunks are 6–9 blocks of `oak_log`; the canopy adds three layers at radius 3–4
  plus a narrower cap, all `oak_leaves`.
- Trunks end up ≥ 12 columns apart while canopies are ≤ 9 wide, so trees never
  overlap. That is why the volume can stamp them in any order and the apron can
  stamp the same trees onto its planes independently.

The apron is the seam guarantee: `trees_touching(chunk)` enumerates every cell
whose trunk can reach the chunk **or its apron** (cell centre ± jitter ± canopy
radius), so a tree rooted in the next cell over appears in both the volume and
the neighbour's apron. `tests/scene.rs` compares every border voxel and every
apron texel against a per-voxel sampler for chunks that a canopy crosses.

### Why the forest floor is flat

One chunk mesh is indexed by `u16`, so it may emit at most 16,384 quads. A
smooth height field already reaches 14,920 quads on a 128³ chunk, which leaves no
room for a canopy. Forest columns therefore blend toward a flat lowland floor:
greedy meshing merges the floor into few quads and the canopy fits. The ring
peaks at **13,547 quads** (seed 1) and **12,343** (default seed) at 128³, and
4.6k at 64³. A dense canopy over full relief needs meshlets; that remains later
M3 work.

## Screenshot capture

`crates/engine/viewer/src/screenshot.rs` plus a hook in `Gpu::render`:

```sh
VOXEL_SCREENSHOT=docs/images/m3-08-stress-scene.png \
VOXEL_SCREENSHOT_FRAMES=12 \
VOXEL_SCREENSHOT_EYE=14,6,20 \
VOXEL_SCREENSHOT_TARGET=2,1,2 \
cargo run --release -p voxel-viewer --features chunk128
```

- The copy is recorded in the frame's own encoder after the draw pass, so the
  image is the frame that is about to be presented — not a re-render.
- The scan-out texture needs `COPY_SRC` on top of `RENDER_ATTACHMENT`; the
  surface configuration adds it only when a capture is requested.
- The buffer is `COPY_DST | MAP_READ` with rows padded to 256 B, mapped once,
  converted from BGRA to RGBA if the surface is BGRA, and written with `png`.
- The capture waits for `VOXEL_SCREENSHOT_FRAMES` frames **after** the ring is
  resident (default 12), so the image does not depend on how many upload frames
  happened to land. Then the process exits 0.
- `_EYE`/`_TARGET` pin a fixed pose, so the committed image does not depend on
  the interactive start camera. The committed image uses eye `(14, 6, 20)`,
  target `(2, 1, 2)` in world units and the default seed.

## Measured

Release viewer, `--features chunk128`, default seed, 1920×1080:

| | RTX 4060 | Radeon iGPU |
|---|---:|---:|
| Frame rate | 178–180 fps | 79–82 fps |
| GPU pass | 1.43–1.69 ms | 7.79–7.87 ms |
| CPU submit | 1.14–1.21 ms (cull 0.05, upload 0, finish 1.02–1.07) | 0.86–0.94 ms (cull 0.05, finish 0.75–0.81) |
| Visible / drawn / resident | 93 / 93 / 256 | 93 / 93 / 256 |
| Geometry | 12,614 blocks, 864,706 quads, 36.3 MiB | same |
| Ring | 13 pages × 8 MiB = 104.0 MiB for 100.5 MiB of meshes, 3.5 MiB slack, 0 holes | same |
| Ring load | 5 s, 141 upload frames, 0 overruns, max pump 0.08 ms, 0 hitches | 7 s, 181 frames, max pump 0.06 ms, 0 hitches |
| Ring load after M3-09 | 5–6 s, 260 upload frames at the 512 KiB default | — |
| Generation | p50 62.8 ms, p99 130.5, max 137.9 per chunk | — |

Seed `0xCAFE` produces a visibly different world (134 meadow, 38 forest, 27 sand,
36 highland, 21 rocky chunks; 3,129 trees versus 7,112 by the same
counting), which is the reproducibility check made visible.

The geometry is ~16% smaller than the M3-07 scene (864,706 vs 1,032,763 quads
drawn): flat forest floors merge away more than the canopy adds. Both GPUs are
correspondingly faster (iGPU 73 → 80 fps). The stress is still 256 chunks and
100.5 MiB of meshes; the trade is canopy for merged floor.

## Tests

```bash
cargo test -p voxel-viewer --test scene
cargo test -p voxel-viewer --features chunk128 --test scene
```

- determinism: one seed gives identical volumes, aprons and heights; another
  moves at least a quarter of 64 sampled columns and most trees;
- seed parsing: decimal, `0x`/`0X` hex, whitespace; rejects empty, `-1`, junk;
- seam: every border voxel and every apron texel equals a per-voxel sampler for
  chunks a canopy crosses (and the check asserts it saw canopy);
- no overlap: no two trees in a 48×48 cell sample share a voxel;
- materials: every block/face pair resolves inside slots 1–23, and grass/log/
  sandstone/sand/snow/bedrock match the committed `faces.ron` entries;
- variants: all five biomes appear, forest trees > 200 and meadow trees > 20,
  both dense-forest and open chunks > 20;
- index budget: every chunk of the default seed's ring (and three more seeds at
  64³) stays ≤ 16,384 quads. The 128³ run takes ~2 minutes in a debug build.

## Limits

- One seed for the committed screenshot; the image changes with the seed and with
  any change to `scene.rs` (M3-12 owns the diff threshold).
- Trees are one canopy shape (oak); the other M1.5-14 tree keys are unused
  because M2.5 did not run.
- No flowers, grass blades or water in the scene: `tall_grass`, `red_flower` and
  `water_still` would be full cubes under the current mesher, and transparency is
  M3.5-06.
- The scene ends at the ring edge; there is no far field yet (M3.5).
- The screenshot path blocks for one map and exits the process; it is a capture
  tool, not an in-game screenshot key.
