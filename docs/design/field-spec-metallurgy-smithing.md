# Metallurgy & Smithing — Field Spec v0.1

Nov 2, 2026 · @Jace

Technological Advancement spec · Voxel RPG · Game-level only (no engine rules)

## 1. Role

Metallurgy & Smithing turns ore into metal and metal into usable things. This field covers metal production, alloys, casting, heat treatment and general smithing, including the furnace and forge internals deferred from Fire & Heat.

The sword crafting spec stays the detailed first-person forging spec for swords. This field is upstream of it: it supplies the metals and processes, and the forge spec consumes them.

- Depends on: Fire & Heat (heat), Mining & Prospecting (ore, coal), Ceramics & Clay (crucibles, molds, linings, tuyeres), Stoneworking & Basic Tools
- Unlocks: Weapons & Siege, Textiles & Armor (mail, plate), Construction & Engineering (nails, hinges, tools), Mechanisms & Automation (gears, springs), Mining & Prospecting (better tools), Power & Energy (boilers), Optics & Measurement (instruments), Core Engineering and Enchanting (metal fittings, soul-bound tools)

## 2. Design pillars

- **Diegetic:** smelt, pour, hammer and quench in first person. No menu crafting.
- **Skill-based:** player skill sets purity, strength and finish, not a stat roll.
- **No tier numbers:** better furnaces, fuel and technique allow harder metals naturally.
- **Realistic, performance-safe:** same rule as the other fields. Heat and cooling are per-workpiece states.
- **Real-time cooling:** unfinished workpieces stay in the world and cool in real time, as in the forge spec.
- **Failure is real:** flawed pieces, cracked quenches, ruined melts.
- **Shared world:** NPCs smelt, forge and trade metal.

## 3. Subfields

### 3.1 Metals and ores

Categories: native metals, copper, tin, lead, iron, gold, silver, and mana metals (later).

Each metal defines: melting behavior, hardness, workability, brittleness, corrosion, weight, color, sources, and fume hazards.

Easy metals come first. Native metals can be cold-hammered. Iron is harder to produce but is the long-term backbone.

### 3.2 Ore processing

Before smelting: crushing, washing, sorting, and roasting for hard (sulfide) ores. Mining & Prospecting ends at clean ore; this field starts at processing it.

Processing leaves slag and waste that affect yield and furnace life.

### 3.3 Smelting

Methods in unlock order:

1. **Cold working:** hammering native metal, no smelting
2. **Open-fire reduction:** ore and fuel in a hearth, small and impure results
3. **Crucible melting:** ore or scrap melted in a clay crucible, then cast
4. **Bloomery:** iron ore reduced to a spongy bloom that must be hammered clean
5. **Shaft furnace:** taller, steadier, higher yield (needs bellows)
6. **Blast furnace:** continuous, powered air (needs Power & Energy)
7. **Crucible steel and cementation:** controlled carbon (late)

Yield and purity depend on ore quality, fuel, airflow, furnace and player skill. Slag must be drawn off.

### 3.4 Alloys

Examples: bronze (copper and tin), brass, pewter, steel (iron and controlled carbon), electrum.

Alloy quality depends on proportions, mixing and purity. Alloys are discovered by experimentation and NPC teaching, like shapes in the forge spec.

Each alloy has an ideal proportion range. Quality falls off smoothly outside it, and landing in a known range names the alloy.

Steel: each piece carries one carbon value, set by smelting method and time in the charcoal. Low carbon is soft (wrought iron), the right amount is hard but tough (steel), and too much is brittle (cast iron). Heat treatment then sets hardness.

### 3.5 Casting

Molds: clay, sand, stone, and later metal. Casting steps: heat metal, prepare mold, pour, cool, break out and finish.

Defects: porosity, cold shuts (poor joins), shrinkage cavities. Hazards: molten metal spills and steam bursts if a mold is damp.

### 3.6 Forging and working

Hammering, drawing out, upsetting, bending, punching, forge welding (at the hottest heat band), riveting, filing, grinding.

Heat is read by glow color using the named bands from Fire & Heat. Metal has a working window; below it, the piece must be reheated.

General smithing (tools, hardware, fittings, armor parts) lives here. Weapon forging details live in the sword crafting spec.

### 3.7 Heat treatment

- **Annealing:** slow cooling to soften
- **Hardening:** quenching in water, oil or brine
- **Tempering:** reheating to balance hardness and toughness

Risks: cracking, warping, uneven hardness. Results are tracked per piece. Individual pieces can carry flaws (inclusions, cracks, soft spots) that affect use and value, matching Ceramics & Clay.

### 3.8 Furnaces and forges

Structures follow the contract from Fire & Heat: heat ceiling, airflow slots, fuel capacity, insulation, build blocks, hazards.

Progression: forge hearth, bloomery, crucible furnace, shaft furnace, blast furnace. Aids: bellows (hand, then water-driven), tuyeres, anvils, tongs, hammers, quench tubs.

No tier numbers: a better furnace simply reaches hotter and steadier. Linings wear and can fail.

### 3.9 Scrap and recycling

Scrap can be remelted or reforged. Mixed scrap contaminates alloys. Materials come from ore, scrap and NPC merchant trade.

Each batch has one purity value. Scrap keeps the properties of its source, each remelt loses a little metal to oxidation, and mixing metals lowers purity in proportion to the foreign metal. A few combinations are ruinous (lead in bronze or steel). Rusty or slag-covered scrap loses extra yield. Skilled smiths sort scrap by color, weight and sound to keep purity up.

### 3.10 Finishing and corrosion

Polishing, oiling, patina, rust. Plating and coatings come later with Chemistry & Alchemy.

### 3.11 Hazards

- Burns from hot metal and tools
- Molten metal spills
- Steam bursts from damp molds
- Toxic fumes (lead, zinc, arsenic) that need ventilation
- Furnace failure and fires
- Flawed pieces breaking in use

### 3.12 NPC smiths and knowledge

Metal knowledge works like the other fields: discovered, observed, taught, spread and lost, with speed scaled by the NPC's IQ-style intelligence number.

- Low bands: cold-hammer native metal, reuse scrap
- Middle bands: smelt copper, cast, forge simple tools
- High bands: bloomeries, alloys, heat treatment, furnace building

NPCs run smithies, hold forges, trade metal and goods, and depend on tools. A settlement that loses its smiths loses its tools.

### 3.13 Mana hooks (Magitech)

Hooks only. Detailed design lives in Mana Materials and Core Engineering.

- Mana-forged alloys that only form in glyph-heated furnaces
- Core-driven furnaces
- Mana-saturated ore
- Metal affinity for glyphs and souls (see the enchanting spec)

## 4. Discovery and unlock path

1. Cold-hammer native metal
2. Notice metal forming from ore in a hot fire
3. Smelt copper in a crucible and cast it
4. Discover alloys (bronze)
5. Reduce iron in a bloomery and hammer out a bloom
6. Learn heat treatment (quench and temper)
7. Learn steel (carbon control)
8. Learn powered air and blast furnaces

Discovery is by observation and experiment, plus NPC teaching, same as the other fields.

## 5. Skill and mastery

Mastery follows the forge spec's three levels where they overlap, with metal-specific layers:

- **Per process:** smelting, casting, forging, heat treatment
- **Per metal:** copper, bronze, iron, steel
- **Total:** blacksmith mastery, shared with the forge spec

Effects: higher yield and purity, fewer flawed pieces, safer handling, finer control of heat.

## 6. Cross-field links

- **Fire & Heat:** heat bands, fuel, furnace structure contract
- **Mining & Prospecting:** ore, coal, tools
- **Ceramics & Clay:** crucibles, molds, linings, tuyeres
- **Sword crafting spec:** forging swords from these metals
- **Enchanting spec:** metal tools carrying souls
- **Chemistry & Alchemy:** flux, plating, acids
- **Mechanisms & Automation and Power & Energy:** bellows, hammers, blast air
- **Weapons & Siege, Textiles & Armor, Construction & Engineering:** metal parts
- **NPC design:** smiths, trade, settlements

## 7. Data sketch (RON)

```ron
Metal(
    id: "copper",
    melts_at: Hot,
    hardness: Soft,
    workability: High,
    corrosion: Medium,
    fume_hazard: None,
)

Alloy(
    id: "bronze",
    parts: [("copper", 9), ("tin", 1)],
    hardness: Medium,
    needs_flux: false,
)
```

## 8. Open questions

- Decided: alloys are proportion-based, with an ideal range per alloy and quality falling off smoothly outside it.
- Decided: medium carbon model. Each piece has one carbon value, set by smelting method and time in the charcoal.
- Decided: metal properties and per-piece flaws carry into finished swords.
- Decided: one purity value per batch. Mixing metals lowers purity, each remelt loses a little metal, and skilled smiths sort scrap to keep purity up.
