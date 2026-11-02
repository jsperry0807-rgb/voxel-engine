# Ceramics & Clay — Field Spec v0.1

Nov 2, 2026 · @Jace

Technological Advancement spec · Voxel RPG · Game-level only (no engine rules)

## 1. Role

Ceramics & Clay turns dug earth into containers, molds, bricks and heat-proof parts. Metallurgy cannot start without crucibles, molds and furnace linings made here.

- Depends on: Fire & Heat (firing), Mining & Prospecting (clay pits, sand), Stoneworking & Basic Tools (shaping tools)
- Unlocks: Metallurgy & Smithing (crucibles, molds, linings), Chemistry & Alchemy (vessels, retorts), Agriculture & Food (jars, cooking pots), Construction & Engineering (bricks, tiles, drains), Medicine & Healing (vessels), Optics & Measurement (glass-making crucibles)

## 2. Design pillars

- **Diegetic:** dig, prepare, shape, dry and fire in first person. No menu crafting.
- **Skill-based:** player skill sets strength, shape and survival through firing.
- **No tier numbers:** better clay, kilns and technique allow harder, hotter, finer ware naturally.
- **Realistic, performance-safe:** same rule as Fire & Heat. Drying and firing are per-piece states, not full simulations.
- **Failure is real:** cracks, warping, blown pots, ruined kiln loads.
- **Shared world:** NPCs make, trade and depend on pottery.

## 3. Subfields

### 3.1 Clay sources and types

Types: earthenware clay, stoneware clay, fire clay (refractory), kaolin (fine white clay, late), slip, and temper materials (sand, ground fired pottery called grog, shell, ash).

Each clay defines: plasticity, shrinkage, strength, heat ceiling, color after firing, impurities. Clay is found by Mining & Prospecting: riverbanks, pits and seams. Quality varies by deposit.

### 3.2 Preparing clay

1. Dig and carry
2. Dry, then soak
3. Remove stones and roots
4. Knead to remove air pockets
5. Add temper to reduce cracking and shrinkage
6. Age the clay (resting improves it)

Bad prep causes failures later: air pockets burst in the kiln, stones crack the pot.

### 3.3 Forming

Methods in unlock order:

1. **Pinch:** shaped by hand from a lump
2. **Coil:** stacked ropes of clay, smoothed
3. **Slab:** flat pieces joined
4. **Molded:** pressed into a mold (bricks, tiles, repeated shapes)
5. **Wheel:** a spinning wheel (needs Mechanisms & Automation)
6. **Slip casting:** liquid clay in a plaster-like mold (late)

Shapes come from free-form experimentation, NPC teaching and quests, like the forge spec. Players can improve designs.

### 3.4 Drying

Stages: wet, leather-hard, bone-dry. Drying runs in real time and depends on weather: humidity, wind, sun, rain. Drying too fast or unevenly cracks the piece. Wet ware must never go straight into fire.

### 3.5 Firing

Firing uses the Fire & Heat rules and structures. Methods in unlock order:

1. **Pit fire:** pots buried in fuel. Low heat, uneven results.
2. **Bonfire or clamp:** stacked fuel and pots, controlled by the player.
3. **Updraft kiln:** a built chamber (a Fire & Heat structure). Hotter and steadier.
4. **Downdraft or muffle kiln:** even heat, protected from flame (late).

Firing stages: preheat slowly (drives out water), ramp up, soak, cool slowly.

Failures: steam bursts, thermal shock cracks, uneven heat, overfiring slumps pieces, kiln collapse.

Each piece tracks its own heat exposure, atmosphere and stress during firing, so results vary across a kiln load.

Atmosphere changes color: oxygen-rich firing gives warm tones, oxygen-starved (reduction) gives dark tones. Fuel choice matters (wood, charcoal).

### 3.6 Glazes and finishes

- **Burnishing:** polishing leather-hard clay, mild sealing
- **Slip decoration:** colored clay layers
- **Ash glaze:** wood ash melting on ware, first found by accident
- **Salt glaze:** salt thrown into a hot kiln (needs Chemistry & Alchemy)
- **Mineral and lead glazes:** richer and waterproof, but some are toxic (hazard, linked to Chemistry & Alchemy)

Unglazed earthenware stays porous. Glazing makes ware waterproof.

### 3.7 Refractories

Refractory ceramics survive extreme heat: fire clay bricks, crucibles, furnace linings, kiln furniture, tuyeres (air nozzles).

Each defines a heat ceiling (using the named heat bands from Fire & Heat) and thermal-shock resistance. This is the key unlock for Metallurgy & Smithing.

### 3.8 Products by use

- **Storage and cooking:** jars, pots, bowls, lamps
- **Building:** bricks, tiles, drains, pipes
- **Industry:** crucibles, molds, linings, retorts, vials, alembics
- **Trade and art:** decorated ware, figurines, tokens

Each product defines: strength, heat tolerance, waterproofing, weight, trade value.

Individual pieces carry flaws (warps, cracks, weak spots) that affect use and value.

### 3.9 Tools

Hands, ribs, paddles, scrapers, cutting wires, molds, wheel, kiln tools. Tools wear slowly and can be made from available materials.

### 3.10 NPC potters and knowledge

Pottery knowledge works like fire and mining knowledge: discovered, observed, taught, spread and lost, with speed scaled by the NPC's IQ-style intelligence number.

- Fire-hardened clay is often found by accident (a pot of clay left in a hearth).
- Low bands: shape and sun-dry simple pieces.
- Middle bands: tempering, pit firing, copying designs.
- High bands: kilns, glazes, refractories, wheel-making.

NPCs run potteries, trade ware, and depend on pots for storage and cooking. A settlement that loses its potters loses its ware.

### 3.11 Mana hooks (Magitech)

Hooks only. Detailed design lives in Mana Materials and Core Engineering.

- Mana-bearing clay
- Glyph-fired kilns
- Mana-resistant crucibles for core refining
- Vessels that hold or store mana

## 4. Discovery and unlock path

1. Play with wet clay, find it holds shape
2. Sun-dry a piece
3. Find fire-hardened clay by accident near a hearth
4. Pit-fire pottery on purpose
5. Learn temper and preparation
6. Build a kiln
7. Make refractories and crucibles: handoff to Metallurgy & Smithing
8. Learn glazes, then wheel and fine ware

Discovery is by observation and experiment, plus NPC teaching, same as the Fire & Heat and Mining & Prospecting specs.

## 5. Skill and mastery

Three levels, mirroring the forge spec:

- **Per technique:** e.g. coiling, kiln firing
- **Per product type:** vessels, bricks, refractories, fine ware
- **Total:** ceramics mastery

Effects: stronger pieces, fewer failures, finer shapes, better kiln loads, hotter safe firing.

## 6. Cross-field links

- **Fire & Heat:** kilns, firing, heat bands, fuel
- **Mining & Prospecting:** clay, sand, temper
- **Metallurgy & Smithing:** crucibles, molds, linings
- **Chemistry & Alchemy:** glazes, vessels, retorts
- **Agriculture & Food:** storage and cooking vessels
- **Construction & Engineering:** bricks, tiles, drains
- **Mechanisms & Automation:** wheel, powered mills
- **NPC design:** potters, trade, settlements

## 7. Data sketch (RON)

```ron
Clay(
    id: "fire_clay",
    plasticity: Medium,
    shrinkage: Low,
    heat_ceiling: ForgeHot,
    color_fired: "buff",
    needs_temper: true,
)

FiredWare(
    id: "crucible_small",
    clay: "fire_clay",
    heat_ceiling: ForgeHot,
    shock_resistance: Medium,
    waterproof: false,
)
```

## 8. Open questions

- Decided: firing results are detailed, tracked per piece (heat, atmosphere, stress).
- Decided: glass-making is its own main field.
- Decided: individual pieces carry quality flaws (warps, cracks) that affect use and value.
- Decided: pottery is not currency itself, but it can be traded for currency.
