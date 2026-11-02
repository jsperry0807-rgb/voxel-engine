# Worldgen tour

How a seed becomes a 1,024 km world with continents, rivers, caves and forests —
and why it can do that on a worker thread without a dense voxel array.

The precise design is [`worldgen-design.md`](../specs/worldgen-design.md). The
measured progress and open items are in the M2.5 reports under
[`milestones/m2.5/`](../milestones/m2.5).

## 1. The world in one page

| Property | Value |
|---|---|
| Shape | Flat finite square, **1,024,000 m** per side |
| Vertical | Floor 0 m, sea level **6,600 m**, peak **12,000 m** |
| Ocean | **35%** of the surface |
| Land | Seed-driven, **1–7 continents** |
| Climate | Compressed: 1,024 km stands in for a planet's pole-to-equator span |
| Chunk | 128³ voxels (8 m) at the target edge |
| Far field | Macro heightfield to **16–32 km** |
| Biomes | **18** — 11 land, 3 ocean, 3 underground, 1 rare |

The compressed climate is intentional. The world reads as one large continent
with strong regional character, not as an Earth slice.

## 2. Two stages: cook once, serve many

World generation is split into a slow, one-time **creation** pass and a fast
per-chunk **runtime** pass.

### Stage 1 — the macro map (once per seed, minutes)

The macro map is a **128 m grid**: 8,000 × 8,000 cells for the whole world.
It stores a handful of channels — height, temperature, humidity, erosion and
flags — at 6 bytes per cell. Raw that is 384 MB; compressed, roughly
120–220 MB.

It is stored in **16 km tiles**. Each tile is 125 × 125 cells, 93.75 KB raw,
and there are **4,096 tiles**. A 32 km view touches at most 3 × 3 tiles, under
1 MB resident. An LRU cache streams them.

The macro map is the **canonical geography**. Chunk generation samples it; it
never invents a new continent.

### Stage 2 — chunk generation (runtime, worker thread, 16 ms)

For one chunk:

1. Load the 3 × 3 macro tiles around it (border-safe).
2. Sample height, climate, erosion, river distance and lake mask at 1 m.
3. Compute biome weights and blend the shape parameters *before* evaluating
   height.
4. Add local detail noise and cliff/plateau shaping.
5. Carve rivers and lake beds from the vector data.
6. Fill 512 blocks using surface rules: soil, filler, stone, beach, underwater,
   slope and altitude overrides.
7. Carve caves, place ores, flora and structures from their anchors.
8. Return a **block grid plus a feature list** — never a dense voxel array.

That last point is the core trick. A 128³ chunk has 2.1 M voxels but only
**512 block samples**. The detail pass expands only the surface blocks, and the
mesher only emits exposed faces.

## 3. Water finds its level

Hydrology is what makes continents feel real, so it runs on the macro grid
before any chunk exists:

1. **Priority-flood** the height field to fill depressions.
2. Compute **D8 flow direction** — each cell drains to its lowest neighbour.
3. Accumulate flow downhill.
4. Extract **lakes** with flat spill surfaces.
5. Extract **rivers as vector polylines**, not raster channels: width, depth,
   order, deltas.
6. Classify coasts — shelf, beach, cliff, fjord.

Rivers are vectors because a 128 m raster would make a 10 m river into a
staircase. The vector data lets a narrow river cut a clean V or U profile
through a chunk.

A metre/kilometre unit bug in the first shape field produced a world with almost
no drainage. After the fix, the largest flow accumulation rose from 48 cells to
**5.7 M**, and each seed produced 1–2 continents. That story is in
[`m2.5-shape-fix.md`](../milestones/m2.5/m2.5-shape-fix.md).

## 4. Eighteen biomes, blended four ways

The 18 biomes:

**Land (11):** beach, plains, forest, taiga, swamp, hills, mountains, alpine,
desert, savanna, tundra.

**Ocean (3):** shelf, deep, trench.

**Underground (3):** aquifer, fungal deep, cavern.

**Rare (1):** geothermal.

Biomes are 8–32 km across with a **300–500 m transition band**. There are no
“transition biomes” and no hard argmax borders. Blending happens in four layers:

1. **Shape** — blend continentalness and erosion *before* the height spline.
2. **Climate weights** — a softmax over biome centres; weights vary smoothly.
3. **Surface** — choose material by weight, with a 1 m hash dither to break up
   the band.
4. **Sub-block** — at a material boundary, pick one of about four canonical
   two-material-plus-debris blocks.

Those transition blocks reuse the parent materials, so they cost **zero atlas
slots** and dedup like any other block.

A few species live only at borders — birch between forest and taiga, scrub
between desert and savanna. That is how the seams read as natural rather than
painted.

## 5. The rest of the world

Once the macro map exists, the per-chunk pass adds the detail:

- **Caves:** five families — spaghetti, cheese, shafts, water/lava, galleries —
  with balanced density, from the surface down to 2,000 m below it.
- **Ores:** depth-tiered veins (coal, iron, gold, diamond), about 1.0% by
  volume, no surface gems.
- **Landmarks:** one per 32 km cell — massif, canyon, volcano, grove,
  monolith/arch — fitted to the terrain.
- **Structures:** ruins, towers, dungeons and bridges on a 2 km anchor grid,
  with most cells empty; prefabs are fitted so nothing floats.
- **Flora:** biome-density plants, 8 m tree cells, seam-only birch/ecotone
  scrub, and rare 30–60 m giants on a 32 km grid.
- **Surface detail:** grass tufts, pebbles, leaves, ripples, moss, snow drift —
  the next task (`M2.5-22`).

See the individual reports in [`milestones/m2.5/`](../milestones/m2.5).

## 6. Determinism is a feature, not a test

The generator is **bit-exact across platforms and thread counts**:

- Noise is integer-hash based with 16.16 fixed-point interpolation.
- No `libm` on the generation path.
- Features come from `hash(seed, cell)`, never a sequential RNG.
- Golden chunk hashes run in CI.
- The save header pins seed, generator, macro and content versions.
- **The generator is frozen per save.** Old generator modules remain callable, so
  an old world always regenerates with the version it was created with.

That last rule is why a content update cannot silently reshape an existing
world. The macro map is stored data; the geography survives even when chunk
detail changes.

## 7. Where worldgen is now

| Area | State |
|---|---|
| Noise contract, macro tiles, splines, climate | Landed (`M2.5-01` … `M2.5-05`) |
| Hydrology, rivers, lakes, tile store | Landed (`M2.5-06` … `M2.5-09`) |
| Biome weights, RON biomes, surface rules, transitions | Landed (`M2.5-10` … `M2.5-13`) |
| Caves, depth, overhangs, ores | Landed (`M2.5-14` … `M2.5-17`) |
| Landmarks, structures, cross-chunk, flora | Landed (`M2.5-18` … `M2.5-21`) |
| Surface detail | **Next** (`M2.5-22`) |
| Far field, water, map | M3.5, not started |

The current measured position is in [`status.md`](../status.md). The full
numbers are in the M2.5 reports and the worldgen design.

## Where to go deeper

- [World generation design](../specs/worldgen-design.md) — the normative spec.
- [M2.5 reports](../milestones/m2.5) — one page per task, with measurements.
- [Worldgen terrain preview](../design/worldgen-preview.md) — a real seed-1
  macro map with statistics.
- [Architecture tour](architecture-tour.md) — how the generated blocks become
  chunks and meshes.
