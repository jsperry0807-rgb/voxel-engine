# Fire & Heat — Field Spec v0.1

Technological Advancement spec · Root field · Voxel RPG · Game-level only (no engine rules)

## 1. Role

Fire is the root node of the tech tree. Every other field depends on controlled heat, directly or indirectly.

- Depends on: nothing
- Unlocks: Metallurgy & Smithing, Chemistry & Alchemy, Agriculture & Food, Medicine & Healing, Power & Energy, Optics & Measurement (glass), Core Engineering (core refining)

## 2. Design pillars

- **Diegetic:** fires are lit, fed and tended in first person. No menu fires.
- **Skill-based:** player skill sets fire quality, not a stat roll.
- **No tier numbers:** heat is read by glow color and behavior. Better structures allow hotter fires naturally.
- **Real-time:** fires burn down, cool and spread in world time. Unattended fires keep going.
- **Dangerous by design:** burns, spread, smoke, explosive fuels.
- **Shared world system:** NPCs and monsters use fire and fear it.

## 3. Subfields

### 3.1 Ignition

Methods, in unlock order:

1. **Found fire:** lightning strikes, lava, burning monsters. The player carries an ember or torch from it.
2. **Friction:** hand drill, then bow drill.
3. **Percussion:** flint on iron-bearing stone, later flint and steel.
4. **Focused light:** a glass lens in sunlight (needs Optics & Measurement).
5. **Chemical:** phosphorus or blackpowder strikers (needs Chemistry & Alchemy).
6. **Mana spark:** glyph or core ignition (Magitech hook).

First-person action: rhythm and pressure for friction, tinder quality, shielding from wind and wet. Failure modes: damp tinder, wind, a dead ember. Embers can be carried in a clay pot or fire horn.

### 3.2 Fuel

Categories: tinder, kindling, softwood, hardwood, charcoal, peat, coal, oils and fats, gases (marsh and mine gas, hazardous), mana-saturated fuel (later).

Each fuel defines: ignition ease, burn duration, heat ceiling, smoke, residue, weight. Moisture matters: wet wood burns cool and smoky. Seasoning wood takes real time.

Charcoal is the key early unlock: a clamp burn (low-oxygen, controlled) turns wood into a hotter, cleaner fuel.

### 3.3 Fire behavior

- **Burn states:** ember, small flame, established, roaring, dying, ash.
- **Spread:** to adjacent flammable blocks by material. Wind and rain change spread.
- **Smoke:** dims the area, chokes in enclosed spaces, attracts monsters and NPCs.
- **Byproducts:** ash and charcoal. Burned terrain changes (grass to ash, then regrowth).

Realism (decided): fire needs fuel, oxygen and heat, and removing any one puts it out.

- Moisture slows ignition and spread; dry fuel and wind speed both
- Fire climbs slopes faster than it descends; wind carries embers ahead of the front
- Enclosed fires starve for air; opening a door can make them flare
- Smoke rises and pools under ceilings; a poor chimney fills the room
- Heat changes materials: wood chars, clay hardens, hot stone cracks if quenched
- Rain, snow and wet ground resist ignition and spread
- Burned ground recovers over time

### 3.4 Heat control

Heat is read by glow color: dull red, cherry, orange, yellow, white. Named bands, no numbers: **Smolder, Warm, Hot, Forge-hot, Weld-hot.**

Controlled by: fuel type, airflow (draft now, bellows later), enclosure and insulation, fuel load. Unfed fires lose heat. Stone and clay hold heat longer.

This field sets the ambient heat source. Workpiece cooling stays in the forge spec.

### 3.5 Fire structures

Progression: open fire pit, stone-ringed campfire, clay hearth, chimney hearth, pit kiln, bloomery-style smelter, forge furnace (later powered via Power & Energy).

Structure contract (what every structure defines): heat ceiling, airflow slots, fuel capacity, insulation, build blocks, hazards.

Furnace and forge internals are deferred to the Metallurgy & Smithing spec. This spec only defines the heat source and structure contract.

### 3.6 Uses

- Light and warmth (night, cold biomes, torches, lanterns)
- Cooking (hands off to Agriculture & Food)
- Pottery and ceramics (kilns)
- Charcoal making
- Land clearing
- Signaling (smoke, beacons)
- Defense (fire barriers, repelling animals)

### 3.7 Hazards and extinguishing

- Burns from touching hot things (consistent with the forge spec)
- Wildfire spread
- Smoke exposure
- Explosive fuels and gas pockets
- Burned-out wooden supports collapsing

Extinguish by: water, smothering with dirt or sand, removing fuel, rain. Fireproofing: stone, clay, treated wood.

### 3.8 Mana-fire hooks (Magitech)

Hooks only. Detailed design lives in Elemental Engineering.

- Fire cores supply heat without fuel
- Glyphs for ignite, sustain and contain
- Mana fuel burns hotter and behaves oddly

### 3.9 NPC fire discovery and teaching

NPC fire use depends on intelligence level, as in real life: fire is discovered, learned, taught, and can be lost.

Intelligence is an IQ-style number per NPC. The named levels below are bands of that scale:

- **Animal:** fears fire and flees. Never uses it. Most beasts and monsters.
- **Cunning:** tolerates fire, is drawn to warmth and cooked scraps. Can use a fire it finds, but cannot make or keep one.
- **Tool-user:** carries embers, feeds and keeps a fire alive, copies what it sees.
- **Maker:** starts fire by friction or percussion, and can teach it.
- **Engineer:** builds hearths, burns charcoal, runs kilns, improves methods.

How an NPC gets fire:

1. **Discovery:** exposure to natural fire (lightning, lava, burning things) gives a chance to notice its uses. Higher intelligence means a higher chance and a faster result.
2. **Observation:** watching a fire user over time, up close.
3. **Teaching:** by the player or another NPC. Teaching speed scales with the learner's intelligence.

Knowledge spreads through groups and settlements. It can be lost if the people who hold it die, or if fire is no longer available.

Behavior: fire-capable NPCs light fires at night, cook, gather fuel and fight spreading fires. Animal-level creatures panic near wildfires.

## 4. Discovery and unlock path

1. Encounter natural fire, carry an ember or torch
2. Learn friction ignition: fire anywhere
3. Build a stone campfire
4. Burn charcoal: hotter, cleaner fuel
5. Build a clay hearth or pit kiln: ceramics unlock
6. Build a smelter: handoff to Metallurgy & Smithing

Discovery is by observation and experiment, plus NPC teaching, same as the forge spec.

## 5. Skill and mastery

Three levels, mirroring the forge spec:

- **Per method:** e.g. friction ignition
- **Per fire type:** cooking fires, kiln fires, furnace fires
- **Total:** fire mastery

Effects: faster ignition, less fuel waste, steadier heat, safer handling.

## 6. Cross-field links

- **Metallurgy & Smithing:** smelters, forges, fuel for hot work
- **Chemistry & Alchemy:** distillation, blackpowder, glass
- **Agriculture & Food:** cooking, smoking, preserving
- **Medicine & Healing:** sterilizing, herb processing
- **Power & Energy:** steam needs sustained heat
- **Optics & Measurement:** glass for lenses
- **Core Engineering:** core refining needs controlled heat
- **NPC design:** NPCs light, tend and fear fires

## 7. Data sketch (RON)

```ron
Fuel(
    id: "dry_hardwood",
    ignition_ease: Medium,
    burn_duration: Long,
    heat_ceiling: Hot,
    smoke: Low,
    residue: Ash,
)

FireStructure(
    id: "stone_campfire",
    heat_ceiling: Hot,
    fuel_capacity: Small,
    insulation: Low,
    airflow_slots: 1,
    build_blocks: ["stone"],
)
```

## 8. Open questions

- Decided: fire is realistic, with a full weather model in the first release (wind, humidity, seasons, drought).
- Decided: NPCs discover fire by intelligence level and can be taught it. Intelligent monsters can use fire too. Intelligence is an IQ-style number and the named levels are bands of it. Band cutoffs will be set later by you. Open: should a placeholder be used until then?
- Decided: rain affects fire realistically, as long as it does not hurt performance.
- Decided: all fuels ship in the first release. Anything that can burn is usable as fuel.
