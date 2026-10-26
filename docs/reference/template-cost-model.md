# The Cost Model First: Why Templates Are Expensive

Before listing what deserves templates, you need to know what a template actually costs, because the answer falls out of the cost model.

**Three costs, in order of severity:**

| Cost                  | What it is                                                                                   | Scaling                                        |
| --------------------- | -------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| **1. Authoring time** | 3D modeling each template in MagicaVoxel/Blender                                             | One-time, per-template                         |
| **2. Raymarch cost**  | Each templated block = a subtree traversal, not a leaf lookup                                | Per-frame, per-templated-block-visible         |
| **3. Damage cost**    | A damaged templated block becomes a _unique_ subtree (no dedup with other damaged instances) | Per-damaged-instance, permanent until repaired |

Cost #1 is the wall for a solo dev. Cost #2 is the wall for a rendering-heavy game. Cost #3 is the wall for a destruction-heavy game. **All three say the same thing: templates should be rare, deliberate, and justified.**

The inverse is also true: **most blocks should be uniform leaves with a 256² texture.** That's the design that keeps the whole system tractable. If you find yourself authoring templates for 50% of blocks, you've drifted into a different (and much more expensive) game.

---

## Three Tiers of Block Representation

Not all "non-uniform blocks" need a full voxel template. There are three tiers, ordered by cost:

### Tier 1 — Uniform leaf + texture (95% of blocks)

```txt
SVDAG:      One leaf node (MaterialId) — 16 bytes, shared globally
Textures:   One 256² slice in the TextureArray, per face
Meshing:    One quad per exposed face, samples the 256² texture
Raymarch:   One leaf lookup, one texture sample
Damage:     Texture crack overlay (like Minecraft)
```

**Cost: essentially free.** Structural sharing means 1,000,000 stone blocks cost 16 bytes total + 256² texture.

### Tier 2 — Shape-masked single-material (non-cubic geometry)

```txt
SVDAG:      A depth-4 subtree where each voxel is either the material OR empty
            (all 4,096 voxels share ONE MaterialId — no per-voxel variety)
Textures:   Same 256² slice
Meshing:    More triangles (the shape has more faces), but 1 material
Raymarch:   Small subtree traversal, single texture lookup at the end
Damage:     Per-instance cost ONLY for damaged instances
```

**Cost: cheap.** Every stone stair in the world points to the same template `BlockId` — 1 template = 1 shared subtree, ~5–15 KB, and 1,000,000 stairs = 1,000,000 pointers (8 bytes each) + the one template. **These are effectively free per-instance** and should be authored freely.

### Tier 3 — Full voxel template (multi-material sub-voxel detail)

```txt
SVDAG:      A depth-4 subtree with MULTIPLE materials (wood grain + knot, ore + stone)
Textures:   Multiple 256² slices, one per material in the template
Meshing:    Many triangles, many materials
Raymarch:   Full subtree traversal, multiple texture/material lookups
Damage:     Per-instance cost + template realization
```

**Cost: expensive.** One template = ~20–60 KB of authored data, and every instance is a subtree traversal. This is where the "95% should be uniform" rule bites.

---

## Concrete Lists

### Tier 1 — Uniform (no template)

These should be **uniform leaves with a good 256² texture**. Do not template them.

**Terrain solids:**

- Stone, cobblestone, andesite, diorite, granite, deepslate, tuff
- Dirt, coarse dirt, grass block (see note below), podzol, mycelium
- Sand, red sand, sandstone, gravel
- Clay, terracotta (all colors), concrete (all colors), wool (all colors)
- Bedrock, obsidian, netherrack, end stone, soul sand, magma
- Snow (full block), ice, packed ice, blue ice
- Smooth stone, polished stone, polished andesite/diorite/granite

**Fluids and gasses:**

- Water, lava, air (air is `MaterialId::EMPTY`, no texture)

**Simple blocks:**

- Glass (transparent but uniform), glass panes (Tier 2, actually — panes are thin)
- Bookshelf (texture handles the books; unless you want physical books sticking out — Tier 3)
- Crafting table (texture handles top; unless you want the physical lid — Tier 3)

**Note on grass blocks:** Minecraft's grass block is a _texture_ solution (top face green, side face dirt-with-grass-edge). This is Tier 1 — the 256² texture handles the transition. You do **not** need a template for grass blocks unless you want the _fringe_ (the spiky grass bits sticking up above the block surface) — that's Tier 3 and optional.

### Tier 2 — Shape-masked single-material (author freely)

These are non-cubic shapes made of one material. **Author templates for all of these** — they're cheap, necessary, and define the game's architectural vocabulary.

**Building shapes:**

- Slabs (bottom, top, double) — for every material that has them
- Stairs (4 rotations × 2 halves) — for every material that has them
- Walls (all connections), fences (all connections), fence gates
- Doors (open/closed, upper/lower), trapdoors (open/closed)
- Ladders, scaffolding, chains, rails (all types)
- Buttons, levers, pressure plates
- Torches, lanterns, candles (light sources with non-cubic geometry)
- Signs, banners (the physical post — the text is a separate overlay)
- Beds, cauldrons (partially — the shape is non-cubic)
- Pots, composters, barrels, beehives

**Plants and small decoration:**

- Flowers (each type), saplings, mushrooms, grass tufts, ferns
- Sea pickles, coral (fan/block types), kelp (top/middle)
- Sugar cane, bamboo
- Crops (growth stages)

**Note on stairs/slabs:** these are the _cheapest_ Tier 2 templates — the shape is algorithmic (30 lines of code to generate a stair pattern). You don't hand-model them in MagicaVoxel; you generate them procedurally in the engine. Same for fences/walls (procedural from connection state).

### Tier 3 — Full voxel templates (author sparingly)

These are where templates earn their real keep. **Author 20–40 of these for a shipped game**, not 200.

**Organic detail (high visual payoff):**

- Leaves with actual gaps (so light and shafts of sky shine through) — but see transparency note below
- Coral (branching, actual 3D structure)
- Mushroom blocks (giant mushroom stems/caps with real geometry)
- Chorus plant / chorus flower
- Organic ore veins in stone (ore nugget as actual 3D shape, not just a texture spot) — this is one of the biggest visual wins for a Minecraft-like
- Crystal formations (amethyst, etc.)

**Sub-voxel blending (where texture can't do it):**

- Grass-dirt transition with fringe (optional; texture can approximate)
- Snow layers (partial layers are non-cubic)
- Moss over stone (texture can approximate; geometry is nicer)
- Bark-to-wood transition on logs (especially for cut logs showing rings)

**Hero / detail blocks:**

- Statues, decorative carvings, totems
- Machines, complex crafting blocks (if they have interior detail visible from outside)
- Player-authored art (of course — the player's creations are always Tier 3)

**Not recommended as Tier 3** (even though tempting):

- Wood planks — texture handles grain perfectly; don't template
- Bricks — texture handles it; don't template
- Chiseled stone — texture handles the carvings; don't template unless close-up detail is the point
- Smooth stone / polished stone — explicitly flat

The rule: **if a 256² texture can represent it convincingly at 1 m viewing distance, don't template it.** Templates are for geometry that a texture _cannot_ fake — non-cubic shapes, sub-voxel blends, organic detail, and shapes that must be seen in silhouette.

---

## The Transparency Exception

Transparent/translucent blocks (leaves, glass, fences, chains) are a special case:

- **Leaves** — Tier 3 (gaps are geometry), but the raymarcher must handle translucent hits and the mesher must emit back-faces.
- **Glass** — Tier 1 (uniform), but transparent — needs a separate render pass and back-face culling.
- **Fences/chains** — Tier 2, but transparent — needs the same treatment.

Transparency is orthogonal to template tier, but **every transparent block is inherently more expensive to render**, regardless of tier. Budget your transparent block count carefully.

---

## The Authoring Budget

Realistic template counts for a shipped game:

| Team size                      | Total templates | Tier 2 (shape)           | Tier 3 (full voxel) |
| ------------------------------ | --------------- | ------------------------ | ------------------- |
| Solo dev, 12-month scope       | ~40             | ~30 (mostly procedural)  | ~10                 |
| Small team (3–5), 2-year scope | ~120            | ~80 (mostly procedural)  | ~40                 |
| Studio, 3-year scope           | ~300            | ~200 (mostly procedural) | ~100                |

**Most of Tier 2 is procedural** — stairs, slabs, fences, walls, doors, ladders, rails are all algorithmic (state → shape). You write ~500 lines of generator code and get ~50 templates for free.

**Most of Tier 3 is hand-authored** — the authoring budget is really the Tier 3 budget. If you can't model 10 detailed blocks in MagicaVoxel/Blender, you can't ship this game. This is the content wall from earlier messages, and it lives entirely in Tier 3.

---

## Special Cases

### Damage

Two damage modes, and the choice determines block cost over time:

| Mode                                  | Visual                                  | Storage                                          | Template tier impact     |
| ------------------------------------- | --------------------------------------- | ------------------------------------------------ | ------------------------ |
| **Texture crack overlay** (Minecraft) | Crack texture drawn over the block      | 0 (just a per-block `damage: u8` in the overlay) | Works with Tier 1        |
| **Voxel damage** (Teardown)           | Actual voxels removed, geometry chipped | Per-instance subtree                             | Realizes Tier 1 → Tier 3 |

**Recommendation:**

- **Tier 1 blocks** (stone, dirt, wood) — texture crack overlay only, no voxel damage. Preserves the uniform-leaf collapse. This is what keeps the whole system tractable.
- **Tier 2 blocks** (stairs, slabs) — texture crack overlay only; shape is preserved.
- **Tier 3 blocks** (leaves, ores, statues) — voxel damage applies; the template is already a subtree, so damage just prunes it.

**This mixed damage model is the key to keeping the "95% uniform" rule intact.** If every block can be voxel-damaged, every block eventually becomes a subtree, and you've lost the collapse. Keep voxel damage for blocks that _already_ have geometry.

### LOD

- **Tier 1** at any LOD — it's already a uniform leaf, aggregate to itself.
- **Tier 2** at any LOD — the shape degrades to a solid block (fine at distance).
- **Tier 3** at LOD 0–1 — visible. At LOD 2+, **collapse to the dominant material as a uniform leaf.** A detailed wood-grain block at distance should render as plain wood, not as 2³ voxels of aggregated noise.

Consequence: **Tier 3 detail is only visible in the near range.** Don't rely on it for silhouette or distant appearance. Design your hero blocks so that the _texture_ of the dominant material still reads at distance; the geometry is the near-range reward.

---

## The One-Line Summary

**Templates are for geometry that a texture can't fake: non-cubic shapes (stairs, slabs, fences — Tier 2, procedural, free) and sub-voxel blends/organic detail/hero assets (Tier 3, hand-authored, ~30–50 for a Minecraft-like).** Everything else — stone, dirt, wood, brick, glass, wool — is a uniform leaf with a 256² texture. If you find yourself templating a solid cube, stop: the texture is the right tool, and the template is buying you nothing but cost.
