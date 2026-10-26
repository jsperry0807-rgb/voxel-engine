# World Generation Design

**Status:** Draft v1 — decisions from the 26 October interview
**Date:** 2026-10-26
**Depends on:** spec v2.2, roadmap v3
**Supersedes:** the worldgen assumptions in roadmap M1-28 (synthetic fixture) and the 64 m world height in ADR-009

---

## 1. The world in one page

| Property | Value |
|---|---|
| Shape | Flat finite square, **1,024,000 m per side** ("1024 km") |
| Edge | Last 20–50 km deep ocean; a circumnavigating storm band and outward current turn boats back; no invisible wall in normal play |
| Ocean | **35%** of the surface |
| Land | Seed-driven, **1–7 continents** with inland seas and island chains |
| Floor | y = 0 |
| Sea level | **y = 6,600 m** |
| Peak | **y = 12,000 m** |
| Land relief | 5,400 m above sea; 6,600 m of ocean below |
| Voxel | 6.25 cm |
| Block | 1 m (16³ voxels) |
| Chunk | 8 m (128³ voxels) at the target edge; 4 m baseline |
| View | **16–32 km** far field; near voxels to 512 m |
| Map | Map-Atlases-style craftable atlas item |
| "From space" | Menu orbit view only; not a gameplay renderer |

Climate zones are **compressed**: 1,024 km stands in for a planet's 10,000 km pole-to-equator distance, so the poles are ~10× closer than Earth's. This is deliberate; the world reads as a large continent, not an Earth slice.

---

## 2. Locked design decisions

| Area | Decision |
|---|---|
| Terrain | Hybrid: 2D macro heightfield + 3D carve. Mixed, seed-driven character; old, rounded erosion |
| Macro map | **128 m cells, tiled**, plus **vector rivers**; 16 km tiles; minutes-long creation |
| Height | Sea 6,600 m / peaks 12,000 m / floor 0 m |
| Caves | Balanced density; all five types; **local surface down to 2,000 m below** |
| Cave types | Spaghetti tunnels, cheese chambers, vertical shafts, water/lava caves, walkable galleries |
| Overhangs | Occasional |
| Biomes | 11 land + 3 ocean + 3 underground + 1 rare = **18**; grounded and stylized |
| Biome size | 8–32 km |
| Transition | 300–500 m gradient; materials dither at 1 m; canonical sub-block mixes |
| Climate | Latitude + noise, ocean distance, altitude lapse rate |
| Cold | Permanent polar caps and altitude snow; no seasonal cycle at launch |
| Water | Oceans, rivers, small and large lakes, rare waterfalls; static bodies + visual current; transparent at launch |
| Rivers | Flow-varying width 10–200 m; navigable on the large ones; deltas |
| Coasts | Mixed: beaches, cliffs, occasional fjords |
| Structures | Ruins, towers, dungeons, bridges/roads; prefab stamps on a deterministic anchor grid; no villages at launch |
| Landmarks | Massifs, canyons/falls, volcanoes, giant trees, monoliths/arches; one per 20–50 km |
| Ores | Common metals + rare gems, tiered by depth |
| Soil | 3–6 blocks default, biome/altitude override |
| Flora | Moderate default, biome-dependent; trees 4–12 m with rare 30–60 m giants |
| Voxel detail | Grass tufts, pebbles, fallen leaves, sand ripples, moss/lichen, snow drift/ice |
| Transition blocks | Two materials + debris, ~4 canonical variants per material pair |
| Materials | 16–24 surface materials; all support wear/damage |
| Determinism | **Bit-exact across platforms**; integer-hash noise |
| Versioning | **Generator frozen per save** |
| Content | Code for shape, **RON for biome content** |
| Macro storage | Tiled streaming with LRU |
| Chunk budget | **2 ms** per chunk on a worker |
| Streaming | **Surface-relative shell + cave cache** |
| Far field | **Heightfield mesh** from the macro map; near/far seam at 512 m |
| Map | Fog-of-war atlas item, discovered as you explore |
| Space | Menu orbit view (pre-rendered or 2.5D) |

---

## 3. Numbers

### 3.1 Macro map

- 1,024,000 m / 128 m = **8,000 cells per axis**.
- 8,000² = **64,000,000 cells**.
- Channels: height `u16` (0.25 m units), temperature `u8`, humidity `u8`, erosion `u8`, flags `u8` = **6 B/cell**.
- Raw size: **384 MB**. Compressed expectation: 120–220 MB.
- Tile: 16 km = 125 cells → **125 × 125 cells**, 93.75 KB raw; **4,096 tiles**.
- A 32 km view touches at most 3 × 3 tiles (one border tile of slack), under 1 MB resident.
- Rivers are **vector** data, not a raster: polylines with width, depth, order, and a 1–4 km spatial index.

### 3.2 Vertical and streaming

- 12,000 m = 12,000 one-metre blocks = 192,000 voxels = **1,500 chunks** of 8 m.
- Near shell: 128 m radius, **±16 m around the surface** → ~3,200 chunks (disc).
- LOD1 at 256 m, LOD2 at 512 m; heightfield beyond.
- Cave cache: a 128 m box around the player when below the surface, evicted on ascent.
- At 1.2–3.2 MiB/chunk the near shell is **3.8–9.8 GiB**; the far field adds < 50 MB.
  The 40% target is 3,275 MiB and the fail line is 4,913 MiB, so the Default profile
  is the risk. See `pre-registration.md` §5.

### 3.3 Generation

- 128³ chunk = **512 block samples**, not 2.1 M voxels.
- 2 ms/chunk on a worker → ~500 chunks/s/core.
- Detail pass is LOD0-only and block-granular.
- World creation: minutes (target ≤ 10 min for a full 8,000² hydrology pass).

---

## 4. Pipeline

### 4.1 World creation (once per seed, minutes, resumable)

1. **Macro fields** at 128 m: continentalness, erosion, base height, temperature, humidity. Integer-hash noise, fixed-point interpolation.
2. **Hydrology**: priority-flood depression fill → D8 flow direction → flow accumulation. Extract:
   - **lakes** with spill levels,
   - **rivers** as vector polylines with width/depth/order,
   - **deltas** where order and gradient drop.
3. **Coast shaping**: shelf, beach, cliff, fjord classification from gradient and wave exposure.
4. **Landmarks** on a jittered 20–50 km grid: massif, canyon/falls, volcano, giant grove, monolith/arch. Deterministic type by terrain and biome.
5. **Structure anchors** on a 1–4 km jittered grid: ruins, towers, dungeons, bridges/roads. Most cells are empty by hash.
6. **Write tiles**: channels + tile-local river segments + landmark/anchor references; header with seed, versions, CRC.

The macro map is the canonical geography. Chunk generation never invents macro features.

### 4.2 Chunk generation (runtime, worker, 2 ms)

1. Load the 3 × 3 macro tiles around the chunk (border-safe).
2. Sample at 1 m: height, climate, erosion, river distance/width, lake mask.
3. Compute biome weights (softmax over the 18 biome definitions) at 4 m and interpolate; shape parameters are blended before evaluation.
4. Add local detail noise (50–500 m wavelength) and cliff/plateau shaping; old, rounded erosion.
5. Carve rivers and lake beds from the vector data (V/U profile, water surface downstream-monotonic).
6. Fill the 512-block grid using surface rules: topsoil, filler, stone, beach, underwater, slope and altitude overrides.
7. Carve caves with a depth-relative field: all five types; depth 0–2,000 m below the local surface.
8. Place ores by depth tier and vein.
9. Place flora and structures from anchors; features generate from their anchor in every chunk they touch.
10. Return a **block grid + feature list**, never a dense voxel array.

### 4.3 Detail pass (LOD0 only)

- Expand surface blocks to voxels with canonical patterns: grass tufts, pebbles, leaves, ripples, moss, snow drift.
- At material boundaries, choose one of ~4 canonical **two-material + debris** blocks per pair by hash.
- Templates reuse the parent materials, so they cost **zero atlas slots** and dedup like any block.
- Depth of detail: 1–2 blocks below the surface; deeper is uniform.

### 4.4 Far field

- The macro heightfield at 128 m is drawn as a distant mesh: 32 km → 250 × 250 cells.
- Colour comes from biome weights plus river/lake masks; fog hides the lack of detail.
- Near/far seam at 512 m: a 64–128 m blend band plus a skirt; near chunks fade into the heightfield.
- This is also the map's source for unexplored terrain.

---

## 5. Biomes and blending

### 5.1 The 18

**Land (11):** beach, plains, forest, taiga, swamp, hills, mountains, alpine, desert, savanna, tundra.
**Ocean (3):** shelf, deep, trench.
**Underground (3):** aquifer, fungal deep, cavern.
**Rare (1):** geothermal.

Ocean zones are depth bands: shelf 0–200 m below sea level, deep 200–4,000 m, trench 4,000–6,600 m. Underground biomes key off depth below the local surface and proximity to water/lava.

### 5.2 Blending in four layers

1. **Shape** — blend continentalness/erosion before evaluating the height spline. Never blend finished heights.
2. **Climate weights** — softmax over biome centres in `(temperature, humidity, erosion, continentalness)`. Weights vary over 300–500 m.
3. **Surface** — per-biome material palette; choose by weight with a per-block hash dither. The dither grain is 1 m, the band is 300–500 m.
4. **Sub-block** — at each material boundary, pick a canonical transition block by hash. Debris (pebbles, roots) breaks the pattern.

Ecotones: a few species live only at borders (birch between forest and taiga, scrub between desert and savanna). No transition biomes; no texture blending; no hard argmax.

### 5.3 Altitude

Universal rock-to-snow above the treeline, regardless of biome; desert mountains lose their red rock near the peak. Permanent polar caps at the north and south edges.

---

## 6. Determinism and versioning

- **Integer-hash noise** with 16.16 fixed-point interpolation; no `libm` in the generation path.
- Feature placement from `hash(seed, cell)`; never a sequential RNG.
- Same output for 1, 2, 8 workers; golden chunk hashes in CI.
- World header pins: seed, generator version, macro version, content version.
- **Freeze per save**: old generator modules remain in the tree; a save always regenerates with the version it was created with.
- The macro map is stored data, so geography survives content changes even if chunk detail does not.

---

## 7. Content schema (RON)

Code owns shape; RON owns content:

```ron
BiomeDef(
    key: "voxel:forest",
    climate: (temperature: (0.35, 0.7), humidity: (0.5, 0.9), erosion: (0.2, 0.7)),
    surface: (top: "voxel:grass", filler: "voxel:dirt", stone: "voxel:stone", beach: "voxel:sand"),
    soil_depth: (3, 6),
    flora: [(species: "voxel:oak", density: 0.08), (species: "voxel:fern", density: 0.2)],
    ecotone: ["voxel:birch"],
    structures: ["ruins", "towers"],
    map_color: (72, 110, 58),
    fog: (0.55, 0.62, 0.58),
)
```

The 18 definitions are content; the shape functions and weight math stay in code.

---

## 8. Open questions

1. **World floor**: solid bedrock at y = 0, or void with a kill plane?
2. **Weather and seasons**: not decided; the current design has no seasonal cycle.
3. **Fauna**: out of scope so far; biomes are defined by flora and terrain only.
4. **Multiplayer**: determinism is ready, but the session model is not designed.
5. **Structure density and loot**: anchors exist; gameplay content does not.
6. **Exact biome list**: the 18 are a baseline; names and materials are placeholders.
7. **Camera far plane and fog**: a 32 km far field needs a logarithmic or reversed-Z depth setup; not yet chosen.
8. **Map atlas UI**: data model is cheap; the item and UI are a game feature.

---

## 9. Scope impact

This design adds two workstreams to the roadmap:

- **M2.5 — Worldgen** (~29 tasks, 45 raw days): macro map, hydrology, biomes, caves, structures, detail pass, determinism, versioning.
- **M3.5 — Far field, water, map** (~14 tasks, 17 raw days): heightfield mesh, seam, transparent water, current, atlas data and UI, orbit view.

Planning with contingency: **+63 days for worldgen, +27 days for far field/water**, taking the one-engineer plan from ~41 weeks to **~59 weeks (~14 months)**. With two engineers, roughly 9–10 months. The 12 km vertical and 16–32 km view are the two decisions that buy most of that cost; both are reversible only by shrinking the world.

The task tables live in `docs/task-list.md` (M2.5 and M3.5), and the spec is amended in v2.3.
