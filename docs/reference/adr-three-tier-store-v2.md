# Architecture Decision Record v2.0: Three-Tier Hierarchical Sparse Volumetric Store

**Status:** Proposed (revised)
**Date:** 2026-10-25
**Supersedes:** v1.0 (same date)
**Scope:** Storage substrate for a Minecraft-like game with sub-block voxel detail
**Related:** Voxelis Bible v2.3, VoxTree implementation (`vox_tree_optimized.rs`)

---

## Changes in v2.0

The requirement set (R1–R5) has been **re-derived from the project's actual goals.** The v1.0 report treated Teardown-style destruction as fixed; v2.0 replaces that with a **localized damage model** that is significantly cheaper to build. Consequences:

- **Physics subsystem is out of scope.** No debris fragments, no rigid-body simulation, no client/server physics determinism problem. This removes ~3–6 months from the timeline and eliminates several cons from v1.0.
- **Damage is stored as bounded per-block data**, not as realized SVDAG subtrees. This means damage does _not_ erode dedup over time, which was a v1.0 con.
- **Content authoring scope drops** from ~30–50 hand-modeled Tier 3 blocks to ~10–20 crack/chip patterns, many procedurally generated.
- **R3 (render distance) and R5 (damage) are now correctly framed** — R3 as a genuine goal (not the storage's justification), R5 as localized interaction (not destruction physics).

The rest of the architecture — three tiers, per-chunk interners, overlay, batched COW, GPU SVDAG — is unchanged. The simplifications above make the same architecture **more viable**, not less.

---

## 1. Executive Summary

This document justifies the storage architecture for a voxel game that targets **1 m gameplay blocks** rendered from **6.25 cm voxels**, in a world large enough to require **streaming and multi-level-of-detail rendering**. The chosen architecture is a **three-tier hierarchical sparse volumetric store**:

1. **Tier 3 (disk):** region-keyed KV files holding serialized SVDAGs and edit diffs.
2. **Tier 2 (CPU):** a hash map of chunks, each owning a mutable **O(1) edit overlay** and a live SVDAG.
3. **Tier 1 (GPU):** GPU-resident SVDAG nodes, read-only during rendering, updated only by batched CPU→GPU consolidation.

The central insight is that **SVDAG storage and interactive editing are fundamentally incompatible write patterns** — one wants immutable, deduplicated, hash-consed structure; the other wants O(1) random writes at 60 Hz. This architecture _separates_ the two into different tiers and reconciles them via consolidation. Mining writes to the overlay; the DAG is updated only in batches, amortizing copy-on-write cost across thousands of edits. The GPU renders the consolidated DAG with the overlay composited on top.

The architecture is viable, it is _the_ pattern for a Minecraft-like with sub-block detail, and it is genuinely better than every alternative we evaluated — conditional on resolving the three under-specified details in §10.

---

## 2. Context and Requirements

The design is constrained by **five requirements**. Three are explicit project goals; two are derived from the framing but essential to satisfiability. Each requirement is now stated with its actual rationale.

### 2.1 R1 — 1 m gameplay block grid (world scale fidelity)

**Requirement:** Gameplay blocks are 1 m cubes. Every measurable thing in the game — doors, ceilings, corridors, player height, explosion radii — is defined in 1 m units.

**Why:** A 1 m grid keeps the world **proportionally true to real life**. An adult human is ~1.7 m tall (about 2 blocks). A door is 2 m (2 blocks). A ceiling is ~3 m (3 blocks). A hallway is 1–2 m wide. A bedroom is ~3 × 4 m. A room's sense of scale is intuitively correct because every unit maps to a real-world measurement a player already understands.

This is not merely an aesthetic preference — it is the property that makes the world _legible_. If doors are 3.125 blocks tall (64 cm grid) or 5 blocks tall (20 cm grid), the player's spatial intuition is constantly off, and the game feels wrong in a way players struggle to articulate. Minecraft's 1 m grid is one of the reasons players find it easy to build in: their internal model of "how big things are" transfers directly.

R1 is therefore a **hard requirement tied to the game's core spatial experience.** It is not negotiable without redefining what the game is.

**Constraint on storage:** The block grid must be the alignment axis for chunk boundaries and edit operations. Chunk sizes must be multiples of 1 m so that blocks never straddle chunks. This is why we chose **4 m chunks** (4 blocks per axis) and **6.25 cm voxels** (16 per meter, exactly power-of-2) — the only voxel size at this scale that makes every block placement a single-chunk operation.

### 2.2 R2 — Sub-block voxel detail (with dual purpose)

**Requirement:** Blocks are composed of finer voxels (16³ = 4,096 per block at 6.25 cm) that can hold independent material information. The world can represent detail below block resolution: cracks, wear, blending, sub-block material variation, hit-based damage.

**Why (part one, gameplay):** Sub-block detail is the game's visual and mechanical identity. It is what makes this "Minecraft with depth" rather than "Minecraft." R5 (below) depends on R2: localized damage requires per-voxel state.

**Why (part two, architectural):** Sub-block detail is also the requirement that **justifies the SVDAG storage choice**. A palette array at 1 m blocks stores 2 bytes per block; at 6.25 cm, it would need 262,144 voxel entries per 4 m chunk (512 KB minimum, with no dedup). For a Minecraft-scale world (mostly uniform terrain), that cost is prohibitive — a palette array at sub-block resolution cannot achieve large render distance because it cannot dedup and its per-chunk footprint is enormous.

An SVDAG at sub-block resolution, by contrast, **preserves structural sharing**: a solid stone block is still one leaf node (16 bytes), a uniform 4 m chunk is a compact tree (~50–400 KB, mostly shared), and a repeated structure (a tree, a wall pattern) shares its nodes across the world. The Voxelis Bible reports 113:1–500:1 dedup ratios on real data.

**The correct framing is: sub-block detail _plus_ render distance _together_ justify SVDAG.** Sub-blocks alone don't (a game that only needs sub-block detail for close-ups could use templates + palette arrays). Render distance alone doesn't (Distant Horizons achieves 512-chunk render distance with palette arrays at 1 m). Only **both together at the same time, at Minecraft scale**, force SVDAG — because only SVDAG gives you dedup-preserved sub-block detail that fits in memory at large render distance.

R2 is therefore not merely a visual requirement. It is the architectural anchor. **If R2 is dropped, the storage should be a palette array. If R2 is kept, SVDAG is the correct structure.**

### 2.3 R3 — Large render distance (world visibility)

**Requirement:** The player can see a large portion of the world — hundreds of chunks, kilometers of terrain — without fog-of-war or aggressive fade.

**Why:** Render distance is not merely a performance metric; it is a **design requirement for the world to feel like a world.** The project's stated goal is that the player can _see a large part of their world like real life_, not just the immediate ~32 chunks of a vanilla Minecraft view. When render distance is short, the world feels small, exploration has less payoff, and distant landmarks (mountains, towers, biome edges) are invisible. Extending render distance to 256–512 chunks transforms the game from "a world you walk through" to "a world you survey."

**Important clarification from v1.0:** R3 does _not_ justify the storage choice by itself. Distant Horizons proves render distance is achievable with palette arrays + explicit LOD chunks. What R3 does is **make SVDAG's render-distance advantage valuable** — the GPU SVDAG eliminates the meshing pipeline and provides free LOD via branch aggregation, which makes _holding both R2 and R3 simultaneously_ tractable. R3 alone would not choose SVDAG; R3 in combination with R2 does.

### 2.4 R4 — Interactive editing at 60 Hz

**Requirement:** Block placement and destruction respond within a frame. The player feels the world as directly editable, not as a batched transaction.

**Why:** Every part of the "Minecraft-like" framing assumes this. Minecraft's feel depends on instant block interaction; delays of even 50 ms are perceptible and break the tactile quality of building. Voxelis's single-set path costs 181 µs–5 ms per edit, which for a single block is fast enough but for a multi-voxel destruction (a full 4,096-voxel block break) is unusable.

R4 is the requirement that forces the **edit overlay** (§3.2). Without it, the SVDAG could be edited directly with no overlay tier. With it, the overlay is mandatory: every edit is an O(1) hash-map write, and the DAG is updated only in batches (Voxelis's batch path, ~100–200 µs for 4,096 edits).

### 2.5 R5 — Localized interactive damage (revised in v2.0)

**Requirement:** Blocks are not binary (present / absent). They carry **localized damage state** that reflects how they have been interacted with:

- **Wear over time:** Repeated traffic on a block (walking, running) gradually wears its surface. Footpaths form. Stairs erode. Eventually a surface may become rough or degrade.
- **Explosions cause real damage:** An explosion damages blocks based on distance, angle, and material. Near-range blocks may be partially destroyed; mid-range blocks are cracked from the blast-facing side; far-range blocks show minor surface damage. The damage is geometrically plausible — the blast's origin dictates which faces are damaged.
- **Weapon hits are directional and positional:** Mining a block with a pickaxe or sword leaves cracks that originate from **where on the block face the hit landed**. Hitting the top edge of a stone block cracks it from the top; hitting the center cracks it from the center. Mining progress is visible as accumulating crack geometry; the block breaks when the damage threshold is reached.

**What R5 is NOT (corrected from v1.0):**

- **Not Teardown-style destruction.** Blocks do not fragment into physics debris, they do not spawn rigid bodies, and there is no per-voxel fragment simulation. When a block is destroyed, it is gone — it does not become a pile of chips.
- **Not voxel-realized subtrees.** Damage is not represented by replacing the block's uniform leaf with a unique damaged subtree. Damage is stored as **bounded per-block data** (§3.2, §4.4), which preserves the "95% uniform" memory property.
- **Not a physics subsystem.** No rigid bodies, no debris, no client/server physics determinism problem.

**Why R5 as stated is valuable:**

- **Directional feedback is a tactile improvement over Minecraft.** In Minecraft, mining is a progress bar; the block breaks with no indication of _where_ you struck it. Directional cracking makes the interaction feel physical and grounded — the block responds to _how_ you hit it.
- **Wear creates an organic environmental history.** Roads that wear down from use, thresholds that erode over time — these give the world a sense of having been _lived in_, without a scripted event.
- **Explosion damage extends the spatial logic of Minecraft.** When a creeper explodes, blocks near the blast show proportional damage, not just "gone / not gone." This makes explosions feel less binary and more like real structural failure.

**Why R5 as stated is dramatically cheaper than Teardown-style destruction:**

| Concern                 | Teardown-style                                       | Localized (R5 v2.0)                               |
| ----------------------- | ---------------------------------------------------- | ------------------------------------------------- |
| Physics engine          | Required (3–6 months)                                | Not required                                      |
| Debris fragments        | Per-voxel rigid bodies                               | Not simulated                                     |
| Multiplayer determinism | Client/server physics sync                           | Deterministic given hit data                      |
| Content authoring       | 30–50 unique damaged-block subtrees                  | 10–20 crack/chip patterns                         |
| Dedup impact            | Damaged blocks become unique subtrees (dedup erodes) | Damage is data alongside blocks (dedup preserved) |
| Voxel storage cost      | Realized subtrees (1–8 KB per damaged block)         | Bounded data (~7 bytes per damaged block)         |

R5 v2.0 is **interactive damage as data**, not destruction as physics. It is a significantly smaller project, and it is what the user actually wants.

---

## 3. Architecture

### 3.1 Tier 1 — GPU-Resident SVDAG (Rendering)

**Purpose:** Render the world without CPU meshing.

**Contents:**

- Two SSBOs per chunk: `children: Vec<BlockId>`, `values: Vec<u16>`.
- A per-chunk overlay compositing buffer (small, ~32 KB).
- A per-chunk light buffer (32 KB).

**Rendering:** Ray-marching DDA on the octree, terminating traversal at depth `D(distance)` for LOD. Overlay composited on top of DAG hits.

**Update model:** Static SSBOs with **incremental range updates**. No dynamic GPU hash table (see §9).

### 3.2 Tier 2 — CPU Chunk Store with Edit Overlay and Damage Data (Authoring)

**Purpose:** Hold loaded chunks, serve game-layer reads, absorb edits and damage.

```rs
VoxChunk {
    position:    IVec3,                  // chunk grid coordinate (4 m)
    svdag:       VoxTree<u16>,           // max_depth 6 = 64³ voxels
    overlay:     EditOverlay,            // HashMap<u32, u16>, pending material changes
    damage:      DamageStore,            // bounded per-block damage data (§4.4)
    lod:         [Option<LodChunk>; 3],
    light:       Vec<u8>,                // 32 KB
    dirty:       bool,
}

EditOverlay { edits: HashMap<u32, u16>, count: u32 }   // bounded at ~4096

DamageStore {
    wear:        HashMap<u16, u8>,       // block_offset → wear level (0-255)
    face_damage: HashMap<(u16, u8), (u8, u8)>,  // (block_offset, face) → (damage_level, origin_hint)
}
```

**Overlay:** Every material change (place, destroy) writes to `overlay` in O(1). Same consolidation policy as v1.0 (size 4096 / 30 s idle / save / unload).

**Damage:** Every wear event, weapon hit, or explosion writes to `damage` in O(1) — a per-block counter increment or a per-face damage entry. Damage is **not** applied to the SVDAG until a threshold is reached, and even then, it is applied as a **localized subtree modification only at that location**, not as a whole-block rewrite. The block's uniform leaf is preserved except where damage has actually changed voxel material (§4.4).

**Read path:** `get_voxel(pos)` → check overlay → check damage (if applicable) → check SVDAG → fall back to chunk generator.

**Consolidation:** Same triggers as v1.0. Damage consolidation uses the same batch path, with a small extension: damage data is applied as batched subtree edits before material consolidation.

### 3.3 Tier 3 — Disk Region Store (Persistence)

**Contents:**

- One region file per 16 × 16 chunks (64 × 64 m).
- Per region: index, node pool (content-hash-keyed), chunk table (chunk → root BlockId), per-chunk damage data.
- Compression: zstd level 7.

**Loading:** Decompress node pool, content-hash each node → `get_or_create` into the chunk interner. Auto-dedup across sessions.

**Damage data** is stored as a separate section per chunk (small: a few KB per region for typical damage density).

---

## 4. Key Design Decisions

### 4.1 Why SVDAG (not palette array)

**Forcing requirement:** R2 (sub-block detail) **combined with** R3 (large render distance).

As discussed in §2.2, sub-blocks alone do not justify SVDAG. Render distance alone does not justify SVDAG. **Both together at Minecraft scale do** — because only SVDAG keeps sub-block detail tractable at large render distance, by preserving structural sharing and enabling zero-cost LOD via branch aggregation.

Alternatives and their failure modes:

| Alternative                       | Fails because                                                  |
| --------------------------------- | -------------------------------------------------------------- |
| Palette array at 6.25 cm          | 512 KB/chunk minimum; no dedup; prohibitive at render distance |
| Palette array at 1 m + LOD chunks | Works, but cannot represent sub-block detail (fails R2)        |
| Flat array at 6.25 cm             | Memory O(world volume); 80 GB for 1 km³                        |
| VDB                               | Weaker dedup than pure SVDAG; heavier random-access writes     |

### 4.2 Why per-chunk interners (not global)

Unchanged from v1.0. Per-chunk interners bound memory, bound lock contention, trivialize unload, and bound serialization cost. The Voxelis Bible's own §3.9 flags the global interner as a scaling wall. Per-chunk solves it.

### 4.3 Why edit overlay (not direct DAG edits)

Unchanged from v1.0. R4 (60 Hz editing) requires O(1) edits. The overlay provides them, and consolidation via Voxelis's batch path (16.6×–224× speedups per the Bible's own benchmarks) amortizes cost across thousands of edits.

### 4.4 Why damage-as-data (new in v2.0)

**Forcing requirement:** R5 v2.0 (localized interactive damage) **combined with** R2's dedup preservation.

Damage encoding options:

| Encoding                              | Cost per damaged block | Preserves dedup?                         |
| ------------------------------------- | ---------------------- | ---------------------------------------- |
| **Realized subtree** (Teardown-style) | 1–8 KB                 | ❌ No — unique per instance              |
| **Damage mask + origin** (chosen)     | ~7 bytes               | ✅ Yes — damage is data alongside blocks |
| **Pure texture crack** (Minecraft)    | 0 bytes (visual only)  | ✅ Yes — but no geometry change          |

**We choose the damage mask + origin encoding.**

- **Wear counter:** per-block `u8` (0–255), incremented on player traffic. Six faces × 1 byte per face for per-face wear = 6 bytes. Plus a global wear counter = 7 bytes per worn block.
- **Weapon hit:** `(face, damage_level, origin_quadrant)` — 3 bytes per face-damage entry. Writes when the player hits a block. Crack visual is generated from the data at render time.
- **Explosion damage:** applied as a batch write to the face-damage map, with per-face damage level derived from blast distance and angle.

**How it renders:** the shader (or CPU mesher, in Phase 1) reads the damage data for each visible block, generates crack geometry procedurally from the data, and composites it over the base material. This is the "texture crack overlay" approach extended to voxel geometry. No SVDAG modification is required for most damage.

**When the SVDAG _does_ change:** when damage crosses a threshold that changes _material_ (e.g., a stone block worn until it becomes gravel), the change is applied as a localized subtree edit at consolidation time. This is rare and bounded.

**Consequence:** Damaged blocks cost ~7 bytes of data (vs. 16 bytes for the uniform leaf) and **share the uniform leaf** with all other undamaged blocks of the same material. Dedup is fully preserved. The v1.0 con ("damage erodes dedup") is eliminated.

### 4.5 Why batched COW consolidation (unchanged)

Voxelis's own benchmarks show 16.6×–224× batch speedups. Consolidation uses this directly.

### 4.6 Why GPU-resident SVDAG (unchanged)

R3 (render distance) requires no meshing pipeline in the near range, and no remeshing on edit. GPU raymarch + LOD via branch aggregation.

### 4.7 Why three tiers (unchanged)

Each tier serves a distinct access pattern (bulk persistence, gameplay reads/writes, per-frame rendering). Merging them creates irreconcilable conflicts (see v1.0 §4.6).

---

## 5. Alternatives Considered and Rejected

(Unchanged from v1.0, with one addition.)

### 5.1 Palette arrays + Distant Horizons-style LOD

Cannot satisfy R2 (sub-block detail) — the entire reason this project exists.

### 5.2 One global interner

Fails at scale per the Voxelis Bible's own §3.9.

### 5.3 Per-edit CoW (no overlay)

Fails R4 — 181 µs–5 ms per edit is unusable at 60 Hz.

### 5.4 CPU meshing (no GPU SVDAG)

Fails R3 — meshing cost at 6.25 cm dominates render distance.

### 5.5 Dynamic GPU hash tables

Research-grade; rejected per §9.

### 5.6 Teardown-style destruction (v1.0's implicit assumption)

**Rejected in v2.0.** R5 has been clarified as **localized interactive damage**, not destruction physics. Teardown-style destruction would add a physics subsystem (~3–6 months), a client/server physics determinism problem, and would erode dedup over time. The v2.0 requirement is a smaller, more tractable, and more focused feature set.

---

## 6. Pros

### 6.1 Satisfies all five requirements

| Requirement                            | Tier       | Mechanism                                    |
| -------------------------------------- | ---------- | -------------------------------------------- |
| R1 (1 m grid, real-world proportions)  | CPU        | Chunk alignment: 4 m chunks = 4 blocks       |
| R2 (sub-block detail; justifies SVDAG) | CPU + GPU  | SVDAG voxels; hash-consed structural sharing |
| R3 (large render distance)             | GPU + disk | GPU SVDAG + LOD + streaming                  |
| R4 (60 Hz edits)                       | CPU        | Overlay                                      |
| R5 (localized interactive damage)      | CPU        | Damage-as-data + procedural crack rendering  |

No requirement is deferred. All are addressed by the current architecture.

### 6.2 Single-block edits are O(1)

The overlay reduces every edit and every damage event to a hash-map write. Voxelis's single-set cost is paid only during consolidation, amortized across thousands of edits.

### 6.3 Dedup is preserved — including for damaged blocks

**Strengthened in v2.0.** Because damage is data alongside blocks (not realized subtrees), damaged blocks share their uniform leaf with all undamaged blocks of the same material. The "damage erodes dedup" problem is eliminated.

### 6.4 No physics subsystem

**New in v2.0.** R5 v2.0 does not require rigid-body simulation, debris, or fragment physics. This removes ~3–6 months from the timeline and eliminates the client/server physics determinism problem for networking.

### 6.5 Render distance is achievable

GPU raymarch + LOD + streaming reaches 256–512 chunks (Distant Horizons territory). The binding constraint becomes VRAM — the correct constraint to be bound by.

### 6.6 Networking is clean

Every edit is `(chunk, voxel_offset, material)` — a serializable, deterministic command. Every damage event is `(block, hit_face, hit_position)` or `(blast_center, blast_radius)` — also deterministic. **No physics, no float divergence, no client/server sync issues for block or damage state.**

### 6.7 Content authoring is dramatically smaller

**Strengthened in v2.0.** R5 v2.0 requires ~10–20 crack/chip patterns (mostly procedural), not 30–50 hand-modeled destroyed-block subtrees. Most crack visuals are generated at render time from damage data, not authored.

### 6.8 Failure modes are graceful

Same as v1.0 — no tier's failure invalidates the others. Phase 1 (meshed rendering, no GPU) ships a working game.

### 6.9 Timeline is shorter

**Reduced in v2.0** by removal of the physics subsystem (3–6 months) and reduction of content authoring scope (2–4 months). See §11.

---

## 7. Cons

### 7.1 GPU tier is still a multi-month research risk

Unchanged. 6–9 months of dedicated work, with real chance of not performing well enough at 6.25 cm. Mitigated by Phase ordering (Phase 1 ships without it).

### 7.2 VRAM is the hard limit on render distance

Unchanged. 512–1024 chunks is the realistic ceiling for 6.25 cm. Sub-block detail costs render distance; there is no way to get both.

### 7.3 Content authoring is still a real cost

**Reduced but not eliminated.** R5 v2.0 requires crack patterns, chip patterns, and material variation. Estimate ~2–4 months of content work (down from 3–6). The authoring wall is real; it is smaller but still the project's content bottleneck.

### 7.4 Lighting is orthogonal and unavoidable

Unchanged. 2–4 months of dedicated work. Not affected by the R5 revision.

### 7.5 Damage rendering must be procedurally generated

**New in v2.0.** Because damage is stored as data, it must be _rendered_ as visible crack/chip geometry. This means either:

- **Shader-side procedural cracks**: read damage data in the fragment shader, generate crack pattern via noise functions. Faster, but requires shader work and constrains crack authorability.
- **CPU-side crack mesh**: generate crack geometry per damaged block, upload as additional geometry. More flexible, but requires per-block mesh caching and adds draw-call overhead.

Recommended: **shader-side procedural cracks** for standard damage, **CPU-side crack mesh** for hero blocks. This is a bounded, tractable problem, but it is _additional work_ that v1.0 did not account for.

### 7.6 Overlay and damage must both be visible to GPU

**Extended from v1.0.** The GPU must see both the overlay _and_ the damage data to render correctly. Two small per-chunk buffers. Same architectural pattern as v1.0's overlay compositing, extended to damage.

### 7.7 Consolidation stutters (still apply)

Unchanged — consolidation takes 1–5 ms per chunk. Frame-budgeted consolidation queue mitigates.

### 7.8 Multiplayer determinism (simplified in v2.0)

**Strengthened in v2.0.** With no physics, all damage events are deterministic given hit data. Block placement, block destruction, damage application, explosion damage — all are command-replayable. **No physics determinism problem.** This is a major simplification for multiplayer.

### 7.9 Ref-count debugging is still painful

Unchanged. Invariant checker is required.

### 7.10 Timeline (revised in v2.0)

| Phase                                          | Time                         |
| ---------------------------------------------- | ---------------------------- |
| Phase 1: CPU tiers + meshed rendering + damage | 4–6 months                   |
| Phase 2: GPU SVDAG + raymarch                  | 6–9 months                   |
| Phase 3: GPU streaming/paging                  | 3–6 months                   |
| Lighting                                       | 2–4 months                   |
| Content authoring                              | 2–4 months                   |
| Editor / tooling                               | 4–6 months                   |
| Networking                                     | 2–3 months (no physics sync) |
| **Total (serial)**                             | **~2–2.5 years**             |

Reduced from v1.0's ~3 years. Physics removal and content-authoring reduction account for most of the savings.

---

## 8. Risks and Mitigations

| Risk                                       | Likelihood               | Impact                      | Mitigation                                                   |
| ------------------------------------------ | ------------------------ | --------------------------- | ------------------------------------------------------------ |
| GPU SVDAG doesn't meet performance targets | Medium                   | High (render distance)      | Phase 1 ships without it; fall back to meshing               |
| VRAM budget insufficient                   | Medium                   | High (render distance)      | Aggressive LOD, prefetch, eviction                           |
| Content authoring stalls                   | Medium (reduced in v2.0) | Project-killer if it stalls | Procedural crack patterns; small authored set                |
| Lighting too complex                       | Medium                   | High (2–4 months)           | Bounded BFS per chunk                                        |
| Damage rendering too expensive             | Low-Medium               | Medium (perf)               | Shader-side procedural cracks; CPU-side only for hero blocks |
| Overlay/damage-GPU consistency bug         | Low                      | High (correctness)          | Required compositing buffers; tested in CI                   |
| Ref-count leak                             | Medium                   | Medium (memory)             | Invariant checker in CI                                      |
| Timeline overruns                          | Medium (reduced)         | High                        | Phase 1 shippable; stop-and-ship at any phase boundary       |

---

## 9. Clarification: "Dynamic Hash Tables" on GPU

Unchanged from v1.0. The design specifies **static SSBOs with range updates**, not dynamic GPU hash tables. The overlay and damage data are CPU-side hash maps; the GPU sees read-only buffers. This preserves every benefit of the architecture while eliminating the hardest component.

---

## 10. Under-Specified Details That Must Be Resolved

### 10.1 Overlay visibility on GPU

Unchanged. Per-chunk overlay compositing buffer; shader checks overlay before DAG traversal.

### 10.2 Consolidation policy

Unchanged. Multi-trigger policy: size (4096 entries), time (30 s idle), event (save, unload, LOD transition), budget (2 ms/frame).

### 10.3 LOD strategy

Unchanged. Explicit per-chunk LOD trees (32³, 16³, 8³ voxels per edge), streamed by distance.

### 10.4 Damage rendering strategy (new)

**Question:** How is damage data rendered as visible geometry?

**Recommended:**

- **Standard damage:** shader-side procedural cracks. The fragment shader reads `face_damage` for the current block, uses it to seed a deterministic noise pattern, and modulates the sampled albedo (darkening, crack lines) and optionally the normal map. No CPU work per frame, no additional geometry.
- **Hero damage:** CPU-side crack mesh, generated once per unique `(material, damage_pattern, origin_hint)` combination, cached by content hash. Reused across all blocks with the same damage. Rendered as an additional pass.

**Why this split:** Standard damage (95% of cases) is cheap and shader-side. Hero damage (5%) is more expensive but bounded by cache reuse, which the SVDAG's `BlockId` content-hashing makes natural.

---

## 11. Phased Implementation

### Phase 1 — CPU tiers + meshed rendering + damage (4–6 months)

- Per-chunk interner (sharded).
- SVDAG per chunk.
- Edit overlay + consolidation.
- Damage data (wear, hits, explosions) as bounded per-block store.
- Disk streaming (region files).
- Meshed rendering (Voxelis `generate_mesh_arrays` or custom greedy).
- Game layer: block abstraction, raycast, place/destroy, damage events.

**Ship:** A Minecraft-like with sub-block templates, instant mining, localized damage (cracks, wear, explosion damage), ~32–64 chunk render distance.

### Phase 2 — GPU SVDAG rendering (6–9 months)

- Upload SVDAG nodes to SSBOs.
- Raymarching shader with LOD depth termination.
- Overlay compositing.
- Shader-side procedural crack rendering.
- Streaming between disk and VRAM.

**Ship:** Same game, ~128–256 chunk render distance, no meshing pipeline.

### Phase 3 — GPU streaming and paging (3–6 months)

- Node paging from disk to VRAM.
- Prefetch by camera velocity.
- Aggressive LOD eviction.

**Ship:** 512+ chunk render distance, bounded by VRAM.

**After Phase 1, the game is complete.** Phases 2 and 3 improve render distance and performance. The phase ordering deliberately protects against the GPU tier failing.

---

## 12. Conclusion

The three-tier architecture is **correct** for the stated requirements as of v2.0. It satisfies all five requirements:

- **R1** (1 m grid for real-world proportion) via chunk alignment.
- **R2** (sub-block detail, which justifies SVDAG when combined with R3) via SVDAG voxels and structural sharing.
- **R3** (large render distance to see the world at real-life scale) via GPU SVDAG + LOD + streaming.
- **R4** (60 Hz editing) via overlay.
- **R5** (localized interactive damage — wear, directional weapon cracks, geometric explosion damage) via damage-as-data.

The architecture is **dramatically more tractable than v1.0** because R5 has been clarified from Teardown-style destruction to localized interactive damage. This removes the physics subsystem (3–6 months and a major determinism problem), shrinks content authoring from ~30–50 templates to ~10–20 procedural patterns, and eliminates the "damage erodes dedup" problem entirely.

The architecture remains **conditional on three things:**

1. **Revising "dynamic GPU hash tables" to static SSBOs with range updates** (§9).
2. **Specifying overlay-GPU compositing, consolidation policy, LOD strategy, and damage rendering** (§10).
3. **Committing to the non-storage subsystems** — lighting, content, tooling — which account for the majority of the remaining timeline.

With those addressed, **this is the storage architecture that a Minecraft-like with sub-block detail and localized damage should use, and I recommend it.** The key insight — that SVDAG storage and interactive editing are incompatible write patterns, reconciled via an overlay — remains intact. The v2.0 revision makes the requirements _correct_ and the project _smaller_, but the architecture is unchanged. It is the right architecture, and now it is serving the right requirements.
