# Mining & Prospecting — Field Spec v0.1

Nov 1, 2026 · @Jace

Technological Advancement spec · Voxel RPG · Game-level only (no engine rules)

## 1. Role

Mining & Prospecting turns the ground into materials. Metallurgy, building, chemistry and fuel all start here.

- Depends on: Stoneworking & Basic Tools (digging tools), Fire & Heat (light, fire-setting)
- Unlocks: Metallurgy & Smithing (ore), Ceramics & Clay (clay pits), Chemistry & Alchemy (sulfur, saltpeter, salts), Power & Energy (coal), Construction & Engineering (stone), Optics & Measurement (sand for glass)

## 2. Design pillars

- **Diegetic:** find, dig and haul in first person. No menu mining.
- **Skill-based:** player skill sets yield, safety and speed, not a stat roll.
- **No tier numbers:** better tools and methods let you reach harder rock and deeper ore naturally.
- **Realistic, performance-safe:** same rule as Fire & Heat. Collapse, gas and flooding are localized to the dug area.
- **Dangerous by design:** cave-ins, bad air, gas explosions, flooding.
- **Shared world:** NPCs and monsters mine, and mines are places things live.

## 3. Subfields

### 3.1 Prospecting

Finding ore before digging. Methods, in unlock order:

1. **Surface clues:** stained rock, outcrops, odd plant growth, loose float stones.
2. **Sampling:** break rock, compare color, weight and streak.
3. **Stream sediment:** panning for heavy grains, tracing them upstream to the source.
4. **Test pits and shallow shafts:** following a vein by digging.
5. **Assay:** heat or chemical tests (needs Fire & Heat, then Chemistry & Alchemy).
6. **Mana sense:** reading mana-rich veins (Magitech hook).

Player knowledge, not a map marker: the player learns what a clue means by experience, NPC teaching or notes.

### 3.2 Extraction methods

In unlock order:

1. **Surface gathering:** loose stones, float ore, clay, sand.
2. **Hand digging:** shovel and pick in soft ground.
3. **Hammer and wedge:** splitting hard rock by hand.
4. **Fire-setting:** heat rock with a fire, then douse with water to crack it. Uses the Fire & Heat rules, so it needs fuel, air and water, and it makes smoke.
5. **Blasting:** blackpowder charges (needs Chemistry & Alchemy). Powerful and risky.
6. **Mechanical:** powered drills and hoists (needs Mechanisms & Automation and Power & Energy).

Mine forms: surface pit, quarry, placer (stream), shaft, drift (horizontal tunnel).

### 3.3 Ore and rock types

Categories: native metals, easy ores (oxides, carbonates), hard ores (sulfides, need roasting), iron ores, coal and peat, clay, sand, salts and sulfur, gems, building stone.

Each rock or ore defines: hardness, vein shape and size, host rock, depth range, byproducts, hazards (some release gas or dust). Veins have real structure: branching, pinching out, faults.

Easy and surface material comes first. Deeper and harder ore needs better tools, support and air.

### 3.4 Tunneling and support

- Rock stability depends on rock type, span width, depth and water.
- Supports: timber props and beams, stone arches, pillars left in place.
- Any unsupported span can collapse; weak rock and wide spans fail sooner. Warnings first: dust, creaks, loose stones.
- Collapse is checked only near edited or stressed areas.
- Fire damages timber supports, linking to Fire & Heat.

### 3.5 Underground hazards

- **Cave-in** and falling rock
- **Bad air:** low oxygen or toxic gas. Open flames burn oxygen, so deep torches go dim.
- **Flammable gas:** explodes if it meets open flame (links to Fire & Heat)
- **Flooding:** breaking into water-filled rock
- **Heat** at depth
- **Darkness**
- **Creatures** nesting in tunnels
- **Falls** into shafts

Warning signs exist for every hazard, so a skilled or informed player can read them.

### 3.6 Light, air and water

- **Light:** torches and lamps (fire), later mana lamps (Magitech hook).
- **Air:** ventilation shafts, doors and bellows early, fans later.
- **Water:** buckets, then drainage channels, then pumps (needs Mechanisms & Automation).

### 3.7 Ore handling

Hauling: baskets, sleds, carts on rails, hoists.

Processing before the furnace: crushing, washing, sorting, roasting.

Smelting and refining belong to Metallurgy & Smithing. This field ends at clean ore, coal, clay and stone.

### 3.8 Tools

Pick, hammer, wedge, shovel, pan, baskets, props. Tools wear and break. Their material follows what the player has access to, so the progression is natural rather than tiered: stone, then copper or bronze, then iron.

Tools can carry souls (see the enchanting spec): a mining tool's glyphs could aid digging or sensing.

### 3.9 NPC miners and knowledge

Mining knowledge works like fire knowledge: discovered, observed, taught, spread and lost, with speed scaled by the NPC's IQ-style intelligence number.

- Low bands: scavenge loose stone, dig soft ground.
- Middle bands: use pick and wedge, copy known methods.
- High bands: read veins, plan shafts, install supports, manage air and water.

NPCs mine, work and settle around mines. Mines can be abandoned, flooded, collapsed or claimed by monsters.

### 3.10 Mana hooks (Magitech)

Hooks only. Detailed design lives in Mana Materials and Core Engineering.

- Mana veins and mana-saturated ore
- Core-bearing rock
- Mana gas pockets
- Glyph-aided digging and sensing

## 4. Discovery and unlock path

1. Pick up surface stones and ore
2. Learn to read rock and surface clues
3. Dig shallow pits and follow veins
4. Learn fire-setting for hard rock
5. Learn timber supports: deeper shafts become safe
6. Learn ventilation and drainage
7. Blasting and mechanical mining arrive with Chemistry and Power

Discovery is by observation and experiment, plus NPC teaching, same as the Fire & Heat and forge specs.

## 5. Skill and mastery

Three levels, mirroring the forge spec:

- **Per method:** e.g. fire-setting, timbering
- **Per mine type:** surface, shaft, drift, placer
- **Total:** mining mastery

Effects: higher yield, less waste, safer tunnels, faster work, better prospecting reads.

## 6. Cross-field links

- **Fire & Heat:** light, fire-setting, gas explosions, timber damage
- **Metallurgy & Smithing:** ore supply, tool metals
- **Ceramics & Clay:** clay pits
- **Chemistry & Alchemy:** sulfur, saltpeter, salts, assay, blackpowder
- **Power & Energy:** coal, pumps, hoists
- **Mechanisms & Automation:** pumps, drills, rails
- **Construction & Engineering:** quarried stone, tunnel and arch design
- **NPC design:** miners, settlements, claims, abandoned mines

## 7. Data sketch (RON)

```ron
OreVein(
    id: "copper_vein",
    ore: "copper_ore",
    host_rock: ["limestone"],
    depth: Shallow,
    shape: Branching,
    size: Small,
    clues: ["green_stain"],
)

MineSupport(
    id: "timber_prop",
    span: Short,
    material: "timber",
    flammable: true,
)
```

## 8. Open questions

- Decided: full cave-in realism. Any unsupported span can collapse, with checks kept local to dug areas for performance.
- Decided: NPCs decide mine ownership themselves (claims, factions, settlements).
- Decided: natural mine monsters (bats, burrowers, cave beasts) and intelligent monsters that mine for themselves both nest in mines, plus monsters drawn to mana veins. Decided: monsters migrate into mines dynamically over time.
- Decided: veins are fixed per world, and new veins form slowly over time.
