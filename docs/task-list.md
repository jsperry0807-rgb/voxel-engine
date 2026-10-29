# Roadmap Task List

**Status:** v1.6 — derived from spec v2.3 and roadmap v3.2
**Date:** 2026-10-29
**Owner:** engine
**Current position:** M1 complete. M1.5 code complete; only `M1.5-17` (texel-density pair) remains open. M2 in progress: `M2-01` done, `M2-02` next.
**Units:** `d` = one focused day, `h` = hours. Estimates are ±50%; they exist to size work, not to promise dates.

---

## 1. How to use this list

- **One task, one PR.** Commit messages quote the ID: `storage: intern runs by mask+children (M1-04)`.
- **A task is done** only when its *Done when* cell is true **and** the *Verify* command passes on the reference machine.
- **Tick the Done column** (`[ ]` → `[x]`) in the same PR that completes the task. GitHub does not render interactive checkboxes inside tables, so the marker is edited by hand; search for `| [ ] |` to find open work.
- **Never start** a task whose *Depends* are unresolved. Dependencies are real code dependencies, not preferences.
- **WIP limit:** two in-progress tasks per engineer.
- **Gate tasks are binding.** M1-35…M1-43 are frozen before measurement (M0-02); they are not edited during M1.
- **Update the docs with the code.** If a task changes a number in the spec, M0-02 is amended in a dated section, never silently.
- **Add tasks, don't widen them.** A task that needs a second sentence of scope is two tasks.
- **Textures follow §2.1.** A block key without a material-manifest entry fails the manifest test; art work is a milestone task (`M1.5-14`, `M2.5-30/31`, `M3.5-15`), not a side effect of a code PR.

### Definition of done (applies to every task)

1. Code compiles with `-D warnings`; no new clippy lints.
2. The named test or benchmark exists and passes; if the task has a numeric budget, the measured value is at or under it (see *Benchmark cadence and stop-the-line rule*).
3. Any new public item has rustdoc with the invariant it relies on.
4. If the task changes a documented number, the spec and this list are updated in the same PR.
5. No `TODO` without an owner and a task ID.

### Task template (for additions)

```txt
ID | Task | Depends | Done when | Verify | Est | Done
```

`Done` is `[ ]` or `[x]`. No other values — partial work is not a state.

### Reference machine

The machine that produced `benchmarks/HISTORY.md` (AMD Ryzen 5 7600; GPU model to be recorded in `docs/reference-machine.md` by M0-02). Every number quoted by a gate must name this machine. Record GPU, driver, RAM and OS at the top of the gate report.

### Benchmark cadence and stop-the-line rule

A benchmark that cannot stop the line is decoration. Every numeric budget in this list is a **pass/fail gate for the task that touches it**, not a note for later.

**When to run benchmarks.** Benchmarks are tied to events, not the calendar. If none of these events happened, there is no reason to run the full suite.

| When | What to run | Must be green before |
| --- | --- | --- |
| Every task whose **Verify** column is a bench, `scripts/bench.sh` or `voxel-bench` command | That task's benchmark, archived with a label | The task is ticked |
| Every PR touching a hot path: layout, COW, allocation, meshing, light, probes, traversal, upload or sync | The targeted bench **before and after**; record both runs | The PR is merged |
| Every sprint close (end of S1…S19) | Every bench touched during that sprint; one full `scripts/bench.sh` run if any hot path changed | The next sprint starts |
| Every milestone exit | Full `scripts/bench.sh` plus that milestone's gate command | The exit tag is created; the next milestone starts |
| M1 gate, end of S4 | `LABEL=m1-gate scripts/bench.sh` after M1-39 and M1-40 | The gate decision (pass or fallback) is written |
| Nightly, once M4 soak/parity exists | Soak and GPU parity jobs | Advisory until 10 stable runs, then required |
| Any threshold miss | The identical bench after the optimization | The project continues past the stop |

Do not run the full suite on every commit. One clean, recorded baseline per milestone is the minimum. Rules for every run: same host and power profile, one change per run, commit recorded with a clean worktree, fixed case set (suite fingerprint), **p95 not the mean**, archived through `scripts/bench.sh` — never quoted from an ad-hoc `target/bench` run. Wall-clock microbenchmarks stay advisory, but memory, p99 latency and generation p95 become hard once the machine has 10 stable runs ([`performance-targets.md`](performance-targets.md) §6).

**Benchmark schedule and thresholds.** Lower is better for time, memory and bytes; higher is better for throughput and reduction ratios. A task is only done when its own *Done when* number is met; the rows below are the checkpoints that block a sprint or a milestone.

| When (run point) | Benchmark / lens | Threshold |
| --- | --- | --- |
| Every frame-reporting run | Frame time, Default profile | ≤ 16.6 ms average; 1% low ≤ 22.2 ms; any frame > 33 ms fails |
| Every memory-reporting run | Voxel VRAM, Default profile | ≤ 3,275 MiB target; > 4,913 MiB is a hard fail |
| M1-18, S3 | Layout cost report | Recorded in `HISTORY.md`; 64³ ≤ 200 k nodes/chunk |
| M1-34, S4 | Granularity instrumentation | ≤ 15% traversed-but-invisible at chunk boundaries |
| M1-37, S4 | Gate edit latency | p95 ≤ 8 ms |
| M1-38, S4 | Gate probe update | p95 ≤ 4 ms per edited chunk |
| M1-41, end of S4 | Full gate report (`LABEL=m1-gate`) | All 12 rows pass; memory and latency per pre-registration; otherwise apply §4 fallback |
| M2-02, S6 | Apron vs dense 130³ | ≤ 25% of the dense path's bytes and time |
| M2-08, S6 | AO merge ratio | Reported for AO on/off; no AO artifacts in goldens |
| M2-17, S7 | Full vs incremental light | Incremental p95 ≤ 20% of full on one edit |
| M2-21, S8 | Probe determinism and update | p95 ≤ 4 ms per edited chunk |
| M2-24, S8 | Scrub latency (edit → remesh + light + probe) | ≤ 4 ms p95 |
| M2-28, S8 | Terrain meshing throughput | ≥ 20 chunks/s single-threaded |
| M2-29, S8 | Worker scratch | ≤ 64 KiB per worker; no chunk-wide densify |
| M2.5-23, M2.5 exit | Chunk generation | p95 ≤ 2 ms per chunk on the reference machine |
| M3-09, S9 | Upload hitch over 1,000 frames | p99 ≤ 4 ms; no single frame > 8 ms |
| M3-10, S9 | Frame rate at gated edge | 60 Hz average, 45 fps 1% low at 1080p |
| M3-11, S9 | 256-chunk VRAM and host memory | Within the 40% VRAM budget with the 1.5× safety margin |
| M3.5-05, M3.5 exit | 32 km view | ≥ 60 Hz average at 1080p |
| M3.5-13, M3.5 exit | Streaming hitch over 2 km flight | p99 ≤ 4 ms; no frame > 16.6 ms |
| M4-13, S11 | Soak, 1,000 edits/s for 10,000 frames | No torn frame; per-frame hash matches the CPU model |
| M4-15, S11 | Leak detector over the soak | Refcounts, pool live count and RSS flat within 1% |
| M4-16, S11 | Commit cost | ≤ 0.1 ms CPU per frame; ≤ 1 fence wait per frame |
| M4-17, S11 | Root-pair overhead | ≤ 5% of chunk DAG |
| M5-18, S14 | CPU parity, 500 poses × 3 fixtures | Zero mismatches |
| M5-21, S14 | Cull cost at 4,096 chunks | Fill + broad + traversal ≤ 1.5 ms |
| M5-22, S14 | Draw-work reduction vs M3 | ≥ 3× on terrain and canopy; < 1.5× triggers the kill criterion |
| M6-01, S15 | LOD policy experiment | Measured table for both paths; ADR decision written |
| M6-17, S17 | VRAM at 128/256/512 m rings | Within 40% of VRAM on the reference machine |
| M6-18, S17 | Streaming hitch over 2,000-frame flight | p99 ≤ 4 ms; no frame > 16.6 ms |
| M6-19, S17 | Reopen | ≤ 50 ms per chunk p95 |
| M7-04, S18 | Regression vs last tagged run | > 10% on a stable metric fails review |
| M7-05, S18 | Memory-budget enforcement | A seeded over-budget fixture fails the build |
| M7-10, S19 | Overnight soak | 8 h pass; invariants checked every minute |
| M7-12, S19 | Clean-checkout smoke | Build, viewer and one recorded bench run pass |

Nightly jobs are advisory until the machine has 10 stable runs, then required. A run that is not archived in `benchmarks/HISTORY.md` is a local experiment and cannot satisfy any row above.

**Stop-the-line rule.** A *miss* means above the threshold for time, memory and bytes, and below it for throughput and reduction ratios. If a measured value misses, the following is mandatory and is not negotiable with schedule pressure:

1. **Stop.** Do not tick the task, do not merge the PR, and do not start the next task or milestone.
2. **Archive the failure.** Record the run with a label that names the task and the miss, and attach the report to the PR.
3. **Classify it.** Use the lenses in [`benchmarks/README.md`](../benchmarks/README.md): memory, CPU, latency, GPU or throughput. Do not optimize before you know which one missed.
4. **Optimize now, in this milestone.** Open a new task with the next free ID that cites the failing bench and the budget; do not widen the original task. Profile first, then take the cheapest lever. The sacrifice ladder (performance-targets §4) is the fallback after code-level levers, not the first move.
5. **Re-run the identical bench** on the same host with the same case set, and record a separate run. A different bench is not evidence.
6. **Continue only when both are true:** the missed metric is on the passing side of its budget, and no other lens regressed by more than 10%. A second regression is a second stop.
7. **If the threshold is frozen** (pre-registration, gate table), the resolution is the pre-registered fallback in pre-registration §4. Changing the number requires a dated amendment, and an amendment after a gate run invalidates that run.

"Temporarily accept", "re-baseline later", "the machine is slow" and "fix it in M7" are not passing states. The WIP limit still applies: an optimization task counts as in-progress work.

**Recording and naming.** Use `LABEL=<milestone>-<task>-<what-changed>`, for example `LABEL=m2-04-greedy` for the optimization and `LABEL=m2-04-greedy-before` for its baseline. `benchmarks/HISTORY.md` is the only history that counts; a run that is not in it did not happen. Compare like with like: same fixture, same case set, same host, clean commit.

---

## 2. Sprint plan

Two weeks per sprint, one engineer. The gate sprint (S4) is the only one that cannot move.

| Sprint | Weeks | Focus | Tasks | Exit | Benchmark checkpoint |
| --- | --- | --- | --- | --- | --- |
| S1 | 1–2 | M0 + layout start | M0-01…M0-24, M1-01…M1-02 | M0 exit: table frozen | — (docs and CI only) |
| S2 | 3–4 | Layout | M1-03…M1-12 | bands, run dedup, COW | — (no perf task) |
| S3 | 5–6 | Layout + parameterize | M1-13…M1-28 | both edges compile | M1-18 layout cost report |
| S4 | 7–8 | References + gate | M1-29…M1-43 | **M1 gate decided** | M1-35…M1-43 full gate run |
| S5 | 9–10 | Vertical slice | M1.5-01…M1.5-17 | one textured chunk on screen | M1.5-13 at 60 Hz/1080p |
| S6 | 11–12 | Greedy mesh + AO | M2-01…M2-09 | golden meshes, AO seams | M2-02 apron; M2-08 AO merge ratio |
| S7 | 13–14 | Block light | M2-10…M2-18 | incremental == full | M2-17 full vs incremental light |
| S8 | 15–16 | Probes + integration | M2-19…M2-30 | M2 exit | M2-21; M2-24; M2-28; M2-29 |
| S9 | 17–18 | Raster scale-up | M3-01…M3-13 | 256 chunks @ 60 Hz | M3-09; M3-10; M3-11 |
| S10 | 19–20 | Versioned GPU A | M4-01…M4-09 | flip protocol works | M4-04/M4-09 byte counts |
| S11 | 21–22 | Versioned GPU B | M4-10…M4-18 | soak clean | M4-13; M4-15; M4-16; M4-17 |
| S12 | 23–24 | Culling A | M5-01…M5-08 | occlusion + broad | M5-01 parity baseline |
| S13 | 25–26 | Culling B | M5-09…M5-17 | traversal + indirect draw | — (correctness tasks) |
| S14 | 27–28 | Culling C | M5-18…M5-24 | ≥ 3× draw reduction | M5-18; M5-21; M5-22; M5-23 |
| S15 | 29–30 | LOD + streaming A | M6-01…M6-08 | LOD decision | M6-01 LOD experiment |
| S16 | 31–32 | Persistence A | M6-09…M6-14 | journal equality | M6-13 reopen |
| S17 | 33–34 | Persistence B + seams | M6-15…M6-20 | M6 exit | M6-17; M6-18; M6-19 |
| S18 | 35–36 | Hardening A | M7-01…M7-07 | CI + budgets | M7-04; M7-05 |
| S19 | 37–38 | Hardening B | M7-08…M7-14 | v0.1 | M7-10; M7-12 |
| S20 | 39–40 | Buffer | — | contingency, not planned work | Re-run any red benchmark |

If the worldgen track runs, M2.5-23 (generation p95 ≤ 2 ms) is a checkpoint at the M2.5 exit. If the far-field track runs, M3.5-05 (≥ 60 Hz at 32 km) and M3.5-13 (hitch p99 ≤ 4 ms) are checkpoints at the M3.5 exit.

**Art timing.** M1.5-14 (slice textures) sits in S5; M2.5-30 and M2.5-31 sit in the first two sprints of M2.5; M3.5-15 sits inside M3.5; the manifest freezes in M3.5-16. With one engineer, S5 needs **14.5 raw days** (10.5 code + 4 art) rather than 10.5, so the slice exit moves to early S6 unless the slice set is sourced externally. The gate at S4 is unaffected; no art task is allowed into the gate sprint.

**Gate date.** Bottom-up, M1 is 33.5 raw days (6.7 weeks), not the roadmap's 5-week estimate, because the reference implementations (M1-27…M1-34) and the gate harness are real work. The gate therefore lands at the end of **S4 (week 8)**, not week 5. The gate content is unchanged; only its date moves. This is the one place the task list overrides the roadmap's schedule, and it is deliberate: a gate measured with borrowed oracles is not a gate.

**With a second engineer:** Track A takes M1 layout then M4 while Track B builds the M1 references and M1.5; they meet at M5. Critical path drops to ~22 sprints. The gate stays at S4 regardless — it needs both tracks' inputs.

**Worldgen track (added v2.3).** M2.5 is 45 raw days of code plus 8 art days (M2.5-30/31), and M3.5 is 17.75 plus 1.5 art days (M3.5-15). With one engineer they insert after M2 and M3, taking the plan to **~27 engineering sprints plus ~1.5 art sprints**; with two, M2.5 runs beside M2–M3 and the critical path grows by ~6 sprints. The gate at S4 is unchanged.

### 2.1 Material and texture schedule

Textures are content, not a side effect of a code task. Two rules are binding:

- **No block key without a texture.** A key that is not in the material manifest fails the manifest test; the registry never falls back to a missing face.
- **No texture authored inside a code PR.** If a milestone needs a face that is not in the manifest, the matching milestone art task is opened first and the milestone names it as a dependency.

The schedule is by milestone, not by calendar. The *Consumed by* column is a hard dependency: a milestone may not mark a set final before its art task is ticked. The *Slots* column is the new atlas ids the set may add, not a target.

| When | Consumed by | Set | Task | Slots |
| --- | --- | --- | --- | --- |
| S5, before M1.5-07 | M1.5-07…M1.5-17 | Vertical-slice faces | M1.5-14 | 23 |
| M2.5 sprint 1, before M2.5-11 | M2.5-11, M2.5-12, M2.5-13 | Biome surface palettes | M2.5-30 | ≤ 24 new |
| M2.5 sprint 1–2, before M2.5-15/19/21 | M2.5-15, M2.5-19, M2.5-21, M2.5-28 | Underground, structures, flora | M2.5-31 | ≤ 32 new |
| M3.5, before M3.5-06 | M3.5-06…M3.5-08, M3.5-12 | Water and current | M3.5-15 | ≤ 3 new |
| with M3.5-10 | M3.5-10 | Map UI sprites (not atlas tiles) | inside M3.5-10 | 0 |
| M3.5-16, before the tag | all later milestones | Material freeze | inside M3.5-16 | 0 |
| M4, M5, M6, M7 | — | No new textures; reuse the frozen set | — | 0 |

#### Per-material deadlines

The deadline is the art task in the last column; its output must be ready before the consuming task (M1.5-07, M2.5-11, M3.5-06). "Before M1.5-07" means the 256² source is in the manifest and bakes to the 16² tile; M1.5-15 is the screenshot and reproduction checkpoint (the full set has landed), and M1.5-17 is the remaining visual acceptance comparison. A face may be a marked placeholder earlier, but no milestone may call the set final before its row is done.

**Slice set.** All 23 faces are authored in **M1.5-14** and must bake before **M1.5-07**; M1.5-17 reviews the final art against the texel-density comparison.

| Texture | Applies to | Done in task |
| --- | --- | --- |
| `stone` | stone | M1.5-14 |
| `cobblestone` | cobblestone | M1.5-14 |
| `gravel` | gravel | M1.5-14 |
| `bedrock` | bedrock | M1.5-14 |
| `dirt` | dirt; grass block bottom | M1.5-14 |
| `grass_block_top` | grass block top | M1.5-14 |
| `grass_block_side` | grass block sides | M1.5-14 |
| `sand` | sand; beaches | M1.5-14 |
| `sandstone_top` | sandstone top | M1.5-14 |
| `sandstone_side` | sandstone sides | M1.5-14 |
| `sandstone_bottom` | sandstone bottom | M1.5-14 |
| `water_still` | water | M1.5-14 |
| `ice` | ice | M1.5-14 |
| `snow` | snow | M1.5-14 |
| `oak_log` | oak log sides | M1.5-14 |
| `oak_log_top` | oak log ends | M1.5-14 |
| `oak_leaves` | oak leaves | M1.5-14 |
| `tall_grass` | tall grass | M1.5-14 |
| `red_flower` | red flower | M1.5-14 |
| `coal_ore` | coal ore | M1.5-14 |
| `iron_ore` | iron ore | M1.5-14 |
| `gold_ore` | gold ore | M1.5-14 |
| `diamond_ore` | diamond ore | M1.5-14 |

`water_still` is authored here even though transparency lands in M3.5; the slice draws it as a normal face.

**Later sets.** Keys marked `*` are proposed; M2.5-30/M2.5-31 freeze the exact name. The deadline is the art task in the last column.

| Texture / material | Applies to | Done in task |
| --- | --- | --- |
| `dry_grass_top`*, `dry_grass_side`* | plains and savanna dry grass | M2.5-30 |
| `savanna_grass_top`*, `savanna_grass_side`* | savanna | M2.5-30 |
| `taiga_grass_top`*, `taiga_grass_side`* | taiga | M2.5-30 |
| `swamp_grass_top`*, `swamp_grass_side`* | swamp surface | M2.5-30 |
| `swamp_mud`* | swamp filler | M2.5-30 |
| `red_sand`* | desert | M2.5-30 |
| `alpine_rock`* | mountains and alpine | M2.5-30 |
| `tundra_moss`* | tundra | M2.5-30 |
| `deep_ocean_floor`* | deep and trench | M2.5-30 |
| `clay`* | shelf and swamp | M2.5-30 |
| `deep_stone`* | deep caves and trench walls | M2.5-31 |
| `fungal_cap_top`*, `fungal_cap_side`* | fungal deep cap | M2.5-31 |
| `fungal_stem`* | fungal deep stem | M2.5-31 |
| `lava_still`*, `lava_flow`* | water/lava caves and geothermal | M2.5-31 |
| `geothermal_rock`* | geothermal | M2.5-31 |
| `obsidian`* | geothermal and lava contact | M2.5-31 |
| `oak_planks`* | towers, bridges, ruins | M2.5-31 |
| `stone_bricks`* | ruins, dungeons | M2.5-31 |
| `cracked_stone_bricks`* | ruins | M2.5-31 |
| `mossy_cobblestone`* | ruins, dungeons | M2.5-31 |
| `birch_log`*, `birch_log_top`*, `birch_leaves`* | birch ecotone | M2.5-31 |
| `spruce_log`*, `spruce_log_top`*, `spruce_leaves`* | taiga and spruce | M2.5-31 |
| `fern`* | forest and taiga undergrowth | M2.5-31 |
| `cactus_top`*, `cactus_side`*, `cactus_bottom`* | desert | M2.5-31 |
| `dead_bush`* | desert and savanna | M2.5-31 |
| `water_flow`* | rivers and current | M3.5-15 |
| `foam_strip`* | river foam | M3.5-15 |
| `caustics`* | underwater light, only if not procedural | M3.5-15 |
| `map_item_icon`* | map item icon (UI sprite) | M3.5-10 |
| `player_marker`* | map marker (UI sprite) | M3.5-10 |

**Biome surfaces (≤ 24 new tiles).** Top, filler, stone and beach faces for the 18 biome palettes, plus each biome's `map_color` (data, not a tile). Reuse the slice set first; add only what the palette cannot express. Transition blocks and debris reuse these parents and add zero slots. M2.5-30 freezes the proposed keys in the table.

**Underground, structures and flora (≤ 32 new tiles).** The per-material rows above are the deadline list. Lava and geothermal tiles carry the emissive material flag; that is a property, not a texture rule. Giant-tree detail reuses the oak/spruce keys; transition and detail templates add zero slots.

**Water and current (≤ 3 new tiles).** `water_still` exists from M1.5-14; add `water_flow` and `foam_strip`. The flow axis and UV-scroll rate are documented with the tiles. Caustics are procedural unless M3.5-15 adds a `caustics` tile; the decision is recorded with M3.5-15, and no caustics texture is invented in the shader task.

**Map UI (0 slots).** The atlas item icon and player marker are UI sprites, not material tiles. They are authored inside M3.5-10 and never enter the 256-slot table.

**Slot budget.** The atlas has 256 ids; id 0 is air and never samples, so 255 are drawable. The slice uses 23. M2.5-30 may add at most 24, M2.5-31 at most 32 and M3.5-15 at most 3, for a ceiling of 82 drawable ids and at least 173 free. The reserve exists for face variants and defects, not for new content. The manifest records every id, key, source, hash, alpha mode and slot; `bake-atlas` rejects a gap in the used range or a count over the cap.

**Texture definition of done.** Source at 256² RGBA8, the historical face resolution retained for the 320² padding fallback, downsampled to the 16² tile; alpha is binary for `Cutout` and smooth for `Translucent`; the tile reads at a 1 m face (6.25 cm per texel) and at mip 4; the source, licence and hash are in the manifest; the atlas bake hash and a screenshot are updated in the same PR. A face that only looks right at 256² is not done.

**What is not a texture task.** Damage is geometric (voxel removal), not a damage texture. AO, block light, probe light and face shading are vertex or light data. Transition and detail blocks are voxel templates that reuse parent tiles. The far field and the map use `map_color` from the biome RON, not atlas tiles. UI sprites are UI assets. M4–M7 add no textures; a mid-milestone need reopens M2.5-31 or M3.5-15 rather than slipping a face into a code PR.

**Where the files live.** The manifest is `crates/engine/block/data/textures.ron`; sources are `crates/engine/block/textures/block/<name>.png`. The manifest test compares its key set against the `voxel:block/*` keys in `blocks.ron`, so a block cannot be added without a face.

Art tasks are milestone tasks, not a separate track: **M1.5-14** (slice), **M2.5-30** (biome surfaces), **M2.5-31** (underground, structures, flora), **M3.5-15** (water/current). The manifest freeze is part of **M3.5-16**. That is the complete art set; there is no `MAT-*` namespace.

---

## 3. M0 — Pre-registration, constants, ADRs

**Goal:** the gate is frozen in writing and the constants exist in code.
**Exit:** M0-24. **Budget:** 7.5 d. **Depends:** none.

### 3.1 Documentation

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M0-01 | Consolidate docs into `docs/` with `docs/README.md` index; move external ADR, material spec and cost model to `docs/reference/`; commit them | — | `docs/` contains spec, roadmap, task list, README, reference; nothing doc-like remains outside the repo; `git log` shows the docs commit | `git status --short` clean for `docs/`; links resolve | 0.25 d | [x] |
| M0-02 | Freeze the pre-registration into `docs/pre-registration.md`: profiles, budgets, residency model, fallback rules, amendment rule, reference machine | M0-01 | Frozen and dated; reference machine named; fallback rules separate memory from latency; amendment rule stated | `git log -1 -- docs/pre-registration.md`; doc review | 0.5 d | [x] |
| M0-03 | Residency model `N(edge, R, V)`, equal-volume rule, and the per-edge/per-profile table | M0-02 | Formula and table in the pre-registration; 64³ vs 128³ chunk counts shown; labelled as prediction, not result | arithmetic checked in the doc | 0.25 d | [x] |
| M0-04 | Create `voxel-core::constants` with `VOXEL_SIZE`, `BLOCK_DIM/BITS`, `CHUNK_BITS`, `ROOT_LEVEL`, `BLOCK_LEVEL`, `CHUNK_DIM`, `CHUNK_VOLUME`, `BLOCKS_PER_CHUNK`, `LEVEL_SIZE`, `aabb_extent(level)`; add `voxel-core` to `voxel-storage` deps; use it in at least one call site | M0-03 | Consts compile in a `const` context; `const` assertions cover every relationship; storage imports them | `cargo test -p voxel-core constants`; `cargo check -p voxel-storage` | 1 d | [x] |
| M0-05 | Node-word layout constants and helpers: `NODE_MASK_SHIFT/BITS`, `NODE_LEVEL_SHIFT/BITS`, `NODE_BASE_SHIFT/BITS`, `encode_node`, `decode_node`; bit-field tests | M0-04 | Round-trips all masks 0–255, levels 0–7, bases 0, 1, 2^48−1; reserved bits stay zero | `cargo test -p voxel-core node_word` | 0.5 d | [x] |
| M0-06 | Atlas constants and helpers: `ATLAS_SIZE=256`, `TILE_SIZE=16`, `TILES_PER_EDGE=16`, `MAX_MATERIALS=256`, `MAX_MIP=4`, `tile_origin(id)`, `uv(tile, face_pos)` | M0-04 | Origin stays in bounds for 0–255; UV lands inside the tile and in `[0,1)`; mip cap asserted; tests | `cargo test -p voxel-core atlas` | 0.5 d | [x] |
| M0-07 | Light, probe and occlusion constants: `LIGHT_CELL_VOXELS=16`, `LIGHT_BYTES_PER_CELL=16`, `PROBE_BYTES=12`, probe ring radii, `OCCLUSION_HORIZON_M=128`, `BLOCK_OCCLUSION_BYTES` | M0-04 | Constants agree with spec §4.2/§7; derived sizes asserted (1 KiB/8 KiB light, 768 B/6 KiB probes); tests | `cargo test -p voxel-core content_constants` | 0.5 d | [x] |
| M0-08 | Add the `chunk128` cargo feature and cfg'd `CHUNK_BITS`; add a temporary `compile_error!` guard until M1-24 so a 7-bit build cannot silently run with 6-bit code | M0-04 | `cargo check --features chunk128` fails with the guard message, not a wrong result; default build unchanged | `cargo check -p voxel-core --features chunk128` (expected guard); `cargo check` (pass) | 0.25 d | [x] |

### 3.2 Decision records (one page each)

Each ADR is Context / Decision / Consequences / Rejected alternatives, cross-linked to the spec section, with no open questions. One page maximum.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M0-09 | ADR-001 render path: raster | M0-02 | States the four specced components the mesher feeds; records raymarch as deferred | review | 0.2 d | [x] |
| M0-10 | ADR-002 chunk edge: 64³ baseline, 128³ pre-registered | M0-03 | Cites the gate, the fallback trigger and the "do not build 128³ in M1" rule | review | 0.2 d | [x] |
| M0-11 | ADR-003 atlas: 256², 16×16 tiles, UV derivation, mip cap | M0-06 | States the 256-slot cap, the per-face variant model and the 6.25 cm texel consequence | review | 0.2 d | [x] |
| M0-12 | ADR-004 light: per-block cells + vertex AO | M0-07 | Rejects per-voxel (4 MiB/chunk) and per-4³-cluster with numbers | review | 0.2 d | [x] |
| M0-13 | ADR-005 probes: per-block near, half-res mid, none far | M0-07 | Defines invalid-in-solid fallback and LOD transition as a content event | review | 0.2 d | [x] |
| M0-14 | ADR-006 occlusion: block flood-fill primary, ray-test M2 | M0-07 | Explicitly says connectivity, not line-of-sight; names the granularity escalation and the kill criterion | review | 0.2 d | [x] |
| M0-15 | ADR-007 A/B roots over the shared pool | M0-04 | Shows root-pair overhead is duplicated path, not 2× chunk; states the recycle rule | review | 0.2 d | [x] |
| M0-16 | ADR-008 lifecycle: visibility × lifecycle, no readback | M0-02 | Lists what the CPU may and may not learn; maps old states to new | review | 0.2 d | [x] |
| M0-17 | ADR-009 render distance and LOD policy | M0-03 | Fixes 128 m LOD0 / 512 m LOD2 / 32 m gated surface shell; retires the 64 m world height; names what M6 measures | review | 0.2 d | [x] |
| M0-18 | ADR-010 mesh granularity and vertex format | M0-04 | 16³ + apron; 16 B vertex; no UV attribute; 76 B indexed quad | review | 0.2 d | [x] |
| M0-19 | ADR-011 node layout: bands, 8 B nodes, run dedup, compaction | M0-05 | States run-granular sharing and its COW cost (≤ 64 B per level) | review | 0.2 d | [x] |
| M0-20 | ADR-012 traversal: workgroup-per-chunk, frontier-synchronous, warp-per-node | M0-02 | Names frontier capacity, overflow policy, four-dispatch order | review | 0.2 d | [x] |
| M0-21 | ADR-013 state ownership: GPU per-frame, event-driven CPU | M0-16 | Lists the event set that wakes the ECS; forbids per-frame readback | review | 0.2 d | [x] |

### 3.3 Repo and CI

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M0-22 | Add `.github/workflows/ci.yml`: fmt, clippy `-D warnings`, `cargo test --workspace`, naga shader tests; add a `chunk128` job marked `continue-on-error` with a TODO to flip at M1-24 | M0-08 | Workflow runs on push; default job green; chunk128 job visible and expected-failing | `act` or first CI run | 0.5 d | [x] |
| M0-23 | Add `scripts/check-constants.sh` grep guard for 6-bit literals outside `constants` (`>> 6`, `& 63`, `ROOT_LEVEL = 6`, `BLOCKS_PER_CHUNK = 64`); warn-only in M0 | M0-04 | Script lists current offenders and exits 0 in warn mode; allow-list documented | `scripts/check-constants.sh` | 0.25 d | [x] |
| M0-24 | M0 exit: tag `m0`, run the checklist (table frozen, ADRs merged, constants used, CI green), record the tag in the roadmap | M0-01…M0-23 | Checklist signed in the PR; tag exists | `git tag -n m0` | 0.25 d | [x] |

---

## 4. M1 — Layout at 64³ and the measurement gate

**Goal:** the spec's layout exists at the 64³ baseline, and the pre-registered gate has real numbers.
**Exit:** M1-43. **Budget:** 33.5 d raw. **Depends:** M0.

### 4.1 Layout — node word and bands

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M1-01 | Implement `Node` encode/decode over the M0 bit fields plus a `NodeRef` newtype (level + index) | M0-05 | All fields round-trip; `NodeRef` never crosses bands; tests | `cargo test -p voxel-storage node_word` | 0.5 d | [x] |
| M1-02 | `Bands` arena: one append-only arena per level, `levelStart[ROOT_LEVEL+1]`, leaf arena; `append`, `range`, `len`, `iter_level` | M1-01 | Ranges stay contiguous after 10k appends; level bounds assert on every access | `cargo test -p voxel-storage bands` | 1 d | [x] |
| M1-03 | Run allocator: allocate a run of `popcount(mask)` nodes; 64 B align full runs; record padding bytes per run | M1-02 | Every mask `0xFF` run starts 64 B aligned; padding is queryable; tests over 1k random masks | `cargo test -p voxel-storage run_align` | 0.5 d | [x] |
| M1-04 | Run interning: `intern(level, mask, children) -> base`, content-hash keyed; idempotent; collision-safe equality check | M1-03 | Same key twice returns the same base; different masks do not collide over 10k runs; deallocation path exists | `cargo test -p voxel-storage run_intern` | 1 d | [x] |
| M1-05 | Popcount child addressing: `child_slot(mask, c)`; allocate/fetch helpers | M1-04 | Matches a naive present-child enumeration for all 256 masks × 8 children | `cargo test -p voxel-storage popcount_slots` | 0.25 d | [x] |
| M1-06 | Leaf band: 8 `u16` materials per run, air = 0 absent, shared leaf runs | M1-04 | Leaf runs dedup; air reads as 0; tests | `cargo test -p voxel-storage leaf_runs` | 0.5 d | [x] |
| M1-07 | `from_dense` builds in BFS order: root first, then each level, leaves last | M1-06 | Band membership is level-correct for 5 fixtures; dense rebuild equality | `cargo test -p voxel-storage bfs_build` | 1 d | [x] |
| M1-08 | `get(x,y,z)` on the new layout | M1-07 | Equal to dense on every coordinate of 3 fixtures and 1M random coords on a noise fixture | `cargo test -p voxel-storage get_matches_dense` | 0.5 d | [x] |
| M1-09 | `expand` to a dense buffer | M1-08 | Byte-equal to the source dense chunk on 5 fixtures | `cargo test -p voxel-storage expand_matches` | 0.5 d | [x] |
| M1-10 | COW `apply_edits`: append one run per touched level; return a counter of runs/bytes written | M1-09 | 10k random batches equal a dense apply; bytes/path reported; no old run mutated | `cargo test -p voxel-storage cow_paths` | 2 d | [x] |
| M1-11 | Empty and uniform collapse: all-air root `NONE`; uniform chunk one leaf run; mask 0 nodes never allocated | M1-07 | Empty build allocates 0 nodes; uniform build ≤ 1 run per level; `get` correct | `cargo test -p voxel-storage uniform_collapse` | 0.5 d | [x] |
| M1-12 | Compaction: renumber one band, remap parent bases, rebuild leaf refs; panic if any live root or in-flight frame references the chunk | M1-10 | Post-compaction `get` sweep equals pre-compaction on 100k coords; `#[should_panic]` test with a live root | `cargo test -p voxel-storage compaction` | 2 d | [x] |
| M1-13 | Serialization v2: header (magic, version, `CHUNK_BITS`, root, level lengths), bands, leaf arena; reject `CHUNK_BITS` mismatch | M1-12 | Round-trip on 5 fixtures; corrupt truncation returns `None`, never panics; mismatch test | `cargo test -p voxel-storage bytes_v2` | 1 d | [x] |
| M1-14 | Invariant checker `check()`: band bounds, level match, alignment, dedup bijection, parent reachability, refcounts | M1-13 | Injected corruptions (wrong level, bad base, unaligned full run) are each detected | `cargo test -p voxel-storage invariants` | 1 d | [x] |
| M1-15 | Keep the public `Svdag` API stable: `from_dense`, `get`, `expand`, `apply_edits`, `to_bytes`, `from_bytes`, `words`, `empty`, `is_empty` | M1-14 | All 16 existing `tests/store.rs` tests pass unchanged | `cargo test -p voxel-storage --test store` | 0.5 d | [x] |
| M1-16 | Property test: 10k random edit batches vs dense reference, including carve-to-air and revert | M1-15 | Passes with 1 seed fixed and 1 random per run; shrinks on failure | `cargo test -p voxel-storage --release property_edits` | 0.5 d | [x] |
| M1-17 | Fuzz `from_bytes`: arbitrary bytes never panic, only `Ok` or `None` | M1-13 | 1M random buffers, no panic; corpus saved | `cargo test -p voxel-storage --release fuzz_bytes` | 0.5 d | [x] |
| M1-18 | Layout cost report: bytes/internal node including padding, run count, dedup ratio vs the old encoding, COW bytes per edited path | M1-16 | Numbers printed by a bench case and recorded in `benchmarks/HISTORY.md` | `scripts/bench.sh LABEL=m1-layout` | 1 d | [x] |

### 4.2 Parameterization — no literal chunk size

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M1-19 | Derive `local_index`/`local_xyz` from `CHUNK_BITS` | M0-04 | Round-trip on all indices of both edges; no shift literals | `cargo test -p voxel-storage index_roundtrip` | 0.25 d | [x] |
| M1-20 | Derive `morton` from `CHUNK_BITS` | M1-19 | Morton round-trip and locality test at both edges | `cargo test -p voxel-storage morton` | 0.5 d | [x] |
| M1-21 | Derive GPU block indexing and `BLOCKS_PER_CHUNK`; keep overlay words per block fixed | M1-19 | Overlay slot math agrees with CPU at both edges | `cargo test -p voxel-storage block_index` | 0.5 d | [x] |
| M1-22 | Chunk table: slot words, position shifts, hash; capacity growth independent of edge | M1-21 | 1024-chunk insert/lookup/remove at both edges; no tombstone leak | `cargo test -p voxel-storage chunk_table` | 0.5 d | [x] |
| M1-23 | WGSL mirror: generate or assert `ROOT_LEVEL`, `BLOCKS_PER_CHUNK`, overlay constants from Rust; a test fails on divergence | M1-22 | Changing a Rust const without the WGSL update fails the test | `cargo test -p voxel-render shader_constants` | 1 d | [x] |
| M1-24 | Remove the `chunk128` guard; make the 7-bit build compile and pass the full suite | M1-19…M1-23 | `cargo test --workspace --features chunk128` green; CI job flipped to required | CI run | 1.5 d | [x] |
| M1-25 | Turn `scripts/check-constants.sh` from warn to error and clear the allow-list | M1-24 | Script exits non-zero on a reintroduced literal; CI runs it | `scripts/check-constants.sh` | 0.25 d | [x] |
| M1-26 | 128³ smoke test (compile-only + one build/get/serialize), excluded from benchmarks | M1-24 | Passes at `--features chunk128`; default build unaffected | `cargo test -p voxel-storage --features chunk128 smoke128` | 0.5 d | [x] |

### 4.3 Reference implementations (slow, correct, reused as oracles)

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M1-27 | Save-scan tool: walk a save directory, report per-chunk node bytes, voxel counts, material histogram, p50/p95/max | M1-13 | Runs on a generated fixture and on an empty dir without panic; emits JSON | `cargo run -p voxel-bench -- save-scan --path <dir>` | 1 d | [x] |
| M1-28 | Freeze a representative fixture world (terrain, caves, overhangs, structures, mixed materials) with a manifest stating provenance and the "provisional until first real save" limitation | M1-27 | Fixture is reproducible from the manifest; hash recorded; `benchmarks/fixtures/` documented | `cargo run -p voxel-bench -- fixture verify` | 1 d | [x] |
| M1-29 | Block-light flood-fill reference: full pass over block cells with 1-block apron, two channels, deterministic order | M0-07 | Reproduces a hand-computed 3-case fixture (open sky, cave, torch); no flake | `cargo test -p voxel-storage --test reference_light` | 1 d | [x] |
| M1-30 | Probe-bake reference: 512 block-centre probes, 64 hemisphere rays, SH4 RGB8, deterministic seed | M1-29 | Same bytes across 10 runs and 1/4 threads; invalid-in-solid flagged | `cargo test -p voxel-storage --test reference_probe` | 1 d | [x] |
| M1-31 | Count-only greedy mesher reference: quads and bytes, no vertex buffer; same-material merge, face orientation | M1-08 | Quad counts match a brute-force per-face counter on 5 fixtures; merge ratio reported | `cargo test -p voxel-storage --test reference_mesh` | 1 d | [x] |
| M1-32 | CPU hierarchical frustum reference traversal: chunk root → octants → block level, stops at LOD depth | M1-07 | Output set equals a brute-force per-block plane test on 100 poses | `cargo test -p voxel-storage --test reference_cull` | 1 d | [x] |
| M1-33 | Block-occlusion reference: camera-seeded 6-connected fill through non-opaque blocks, horizon-bounded, per-block bits | M1-29 | Enclosed cave is occluded; open field is not; never marks a visible block occluded in 100 poses | `cargo test -p voxel-storage --test reference_occlusion` | 1 d | [x] |
| M1-34 | Granularity instrumentation: traversed block volume vs reference-visible volume at chunk boundaries | M1-32, M1-33 | Emits the ratio per pose; unit test on a synthetic 2-chunk case | `cargo test -p voxel-storage granularity` | 0.5 d | [x] |

### 4.4 Gate harness and decision

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M1-35 | Per-chunk memory distribution over the fixture: DAG, overlay, light, probes, mesh estimate, p50/p95/max | M1-28, M1-31 | Report includes each component and a total; no synthetic-only input | `cargo run -p voxel-bench -- gate memory` | 1 d | [x] |
| M1-36 | Measure `N` (loaded chunk count) for the configured view/band at 64³ and compute the 128³ equivalent at equal volume | M0-03, M1-35 | `N64 = 8 × N128` asserted; both counts printed with the config | `cargo run -p voxel-bench -- gate residency` | 0.5 d | [x] |
| M1-37 | Edit-latency harness split into fixed (upload/fence/flip) and dirty-region (COW, light) terms, p50/p95/max | M1-10, M1-29 | Both terms reported per edit; 1k edits on the fixture; no synthetic padding | `cargo run -p voxel-bench -- gate edits` | 1 d | [x] |
| M1-38 | Probe-update harness: p50/p95/max per edited chunk | M1-30 | Reported with the edit that triggered it; budget comparison in output | `cargo run -p voxel-bench -- gate probes` | 0.5 d | [x] |
| M1-39 | Gate report generator: one MD + one JSON with every pre-registered row, pass/fail, margin and the machine | M1-35…M1-38 | Report has all 12 rows; pass/fail computed, not typed; machine named | `cargo run -p voxel-bench -- gate report` | 1 d | [x] |
| M1-40 | Analytic 128³ scale-up model: fixed vs variable terms, pessimistic (dirty spans chunk) and optimistic; unit tests on the arithmetic | M1-37 | Model reproduces the measured 64³ numbers when fed 64³ inputs; both projections printed | `cargo test -p voxel-bench scaleup` | 1 d | [x] |
| M1-41 | Run the gate, record it through `scripts/bench.sh LABEL=m1-gate`, attach the report to the PR | M1-39, M1-40 | `benchmarks/HISTORY.md` has the run; report is linked from it | `scripts/bench.sh LABEL=m1-gate` | 0.5 d | [x] |
| M1-42 | Write the gate decision: pass or fallback, with the measured numbers next to each threshold; if pass, amend ADR-002 to greenlight 128³ for M2; if fail, record stay-64³ | M1-41 | Decision doc names every row and its margin; no "renegotiation" language | review | 0.5 d | [x] |
| M1-43 | M1 exit: tag `m1`, update the roadmap's milestone status, freeze the gate cases as permanent benches | M1-42 | Tag exists; gate cases run in CI as advisory | `git tag -n m1` | 0.25 d | [x] |

---

## 5. M1.5 — Vertical slice

**Goal:** one textured chunk in a window. **Exit:** M1.5-17. **Budget:** 10.5 d code + 4 d art (M1.5-14). **Depends:** M1-15 (layout API), M0-06 (atlas math); M1.5-14 before M1.5-07.
**Status:** Code complete — M1.5-01…M1.5-16 done. The remaining exit artifact is M1.5-17 (before/after texel-density pair); the full M1.5-15 screenshot set and viewer README have landed. M2 work has started before the final visual checkpoint was closed.
**Art:** M1.5-14 authors the 23 slice faces; M1.5-17 is the exit art checkpoint.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M1.5-01 | Create `crates/engine/viewer` (`voxel-viewer`), add to workspace, `main.rs`, error type | M1-24 | `cargo run -p voxel-viewer` opens an empty window | run | 0.5 d | [x] |
| M1.5-02 | wgpu init: instance, adapter, device, queue, surface, config, resize handling | M1.5-01 | Window survives 20 resizes without validation errors | run | 0.5 d | [x] |
| M1.5-03 | Event loop, close/escape, frame timing, log adapter info | M1.5-02 | Quit cleanly; adapter string printed at startup | run | 0.5 d | [x] |
| M1.5-04 | Camera: position, yaw/pitch, view/proj, uniform buffer; WASD + mouse-look | M1.5-03 | Camera matrices validated against a known look-at case | `cargo test -p voxel-viewer camera` | 1 d | [x] |
| M1.5-05 | Depth buffer and depth pass | M1.5-04 | Depth test visible on two overlapping quads | run | 0.5 d | [x] |
| M1.5-06 | Atlas builder: 256×256 RGBA8, 16×16 tiles, mips capped at 4, tile-origin helper | M0-06 | All 256 tiles addressable; mip count = 5; no tile crosses a mip boundary in a test | `cargo test -p voxel-viewer atlas` | 1 d | [x] |
| M1.5-14 | Author the 23 slice textures: 256² RGBA8 sources, manifest entries, alpha matching each `RenderType`, licence/provenance | M1.5-06 | Every `voxel:block/*` key in `blocks.ron` has a source and manifest entry; dimensions, alpha and key set match; no unmarked placeholder remains | `cargo test -p voxel-block texture_manifest` | 4 d | [x] |
| M1.5-07 | Downsample tool: M1.5-14's 256² material faces → 16×16 tiles; deterministic and documented | M1.5-14, M1.5-06 | Every M1.5-14 key bakes; `cargo run -p voxel-viewer -- bake-atlas` writes a reproducible PNG/hash; no tile crosses a mip boundary | run + hash | 1 d | [x] |
| M1.5-08 | Registry cap: freeze errors above 256; per-face texture sets become material variants; migration notes | M1.5-07 | Registering 257 fails with a clear error; the 23 slice tiles plus face variants fit under 256 and the slot table is recorded; existing 64-block fixture still freezes | `cargo test -p voxel-block cap_256` | 1 d | [x] |
| M1.5-09 | Upload atlas, sampler, bind group; nearest/linear policy documented | M1.5-06 | Texture sampled in a full-screen test draws the expected tile | `cargo test -p voxel-viewer --test gpu` | 0.5 d | [x] |
| M1.5-10 | Naive per-face mesher (correct, not greedy) producing the M0 vertex format; quad struct shared with M2 | M1.5-04 | Emits exactly 6 quads for one isolated voxel; 0 for enclosed | `cargo test -p voxel-viewer naive_mesh` | 1 d | [x] |
| M1.5-11 | Vertex/index buffer creation and upload for one chunk | M1.5-10 | Buffer sizes match the quad count; no validation error | run | 0.5 d | [x] |
| M1.5-12 | Shader: UV from world position and face axes, atlas tile from material, face shading, AO placeholder | M0-06, M1.5-09 | A 1 m block face shows exactly one 16×16 tile; screenshot | visual | 1 d | [x] |
| M1.5-13 | Draw one generated chunk end to end | M1.5-11, M1.5-12 | Terrain visible; 60 Hz at 1080p on the reference machine; placeholder tiles are allowed here, M1.5-14 supplies the final art before the M1.5-15 screenshot | run | 0.5 d | [x] |
| M1.5-15 | Screenshot set in `docs/images/` plus run instructions | M1.5-07, M1.5-13, M1.5-14 | D9 screenshot set committed; README explains how to reproduce | review | 0.25 d | [x] |
| M1.5-16 | Zero-validation run and naga shader test in CI | M1.5-13 | CI job passes with `RUST_LOG=wgpu_core=warn` | CI | 0.5 d | [x] |
| M1.5-17 | Before/after texel-density screenshots (old 256² face vs 16×16 tile) so the regression is a decision | M1.5-14, M1.5-07 | Two images in `docs/images/` with a one-paragraph note; art matches M1.5-14 | review | 0.25 d | [ ] |

---

## 6. M2 — Production content pipeline

**Goal:** deterministic mesh + AO + block light + probes, at the 128³ Minimum edge.
**Exit:** M2-30. **Budget:** 27.0 d raw. **Depends:** M1 gate, M1.5 vertex/atlas format, M1.5-14 (texture set).
**Status:** In progress — M2-01 done; M2-02 next. Target edge: **128³ Minimum** (ADR-002 / [`m1-gate-decision.md`](m1-gate-decision.md)); 128³ Default is not greenlit.
**Art:** none beyond M1.5-14; M2 reuses the slice atlas.

**M1 gate carry-over — blocks M2-30:**

- Mesh compression must bring the 64³ p95 mesh under 600 KiB before the 128³
  estimate can meet its 2.4 MiB cap. The greedy/AO work in M2-04…M2-09 is the
  first lever; measure it rather than assuming the ratio.
- Run the **128³ Minimum** gate after M2-29 and record memory, edit, probe and
  throughput on the reference machine.
- Close gate rows 9 and 11: wire the M1-34 granularity reference into the report
  and replace the provisional fixture with a real populated save.
- If the 128³ run misses the 4.80 GiB hard-fail line, apply the pre-registered
  fallback: 64³ Minimum, then mesh compression, near radius, vertical shell.

### 6.1 Apron and mesher

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2-01 | Apron API: six 1-voxel face planes (128² u16 each) from the world, including cross-chunk neighbours | M1-15 | 6 planes equal a direct dense read of the 130³ padded volume on 3 fixtures | `cargo test -p voxel-storage apron` | 1 d | [x] |
| M2-02 | Apron cost benchmark vs a dense 130³ copy; record the winner | M2-01 | Face-plane path ≤ 25% of the dense path's bytes and time | `cargo bench -p voxel-storage apron` | 0.5 d | [ ] |
| M2-03 | Block-granular driver: iterate 16³ blocks, build a bounded scratch per block (18³), no chunk-wide densify | M2-01 | Peak scratch ≤ 64 KiB/worker on the noise fixture | `cargo test -p voxel-mesh scratch_budget` | 1 d | [ ] |
| M2-04 | Create `crates/engine/mesh` (`voxel-mesh`); greedy bitmask core over one axis at a time | M2-01 | Quad count ≤ visible-face count; matches the M1-31 reference on 5 fixtures | `cargo test -p voxel-mesh greedy_matches_reference` | 2 d | [ ] |
| M2-05 | Merge key: `(material, orientation, 4×AO)`; quads split on any difference | M2-04 | Mixed-material fixture splits exactly at material changes; no over-merge | `cargo test -p voxel-mesh merge_key` | 1 d | [ ] |
| M2-06 | Backface cull at emit: only the outward quad per exposed face; no interior faces | M2-04 | Isolated voxel → 6 quads; two adjacent → 10; enclosed → 0 | `cargo test -p voxel-mesh backface` | 0.5 d | [ ] |
| M2-07 | AO sampling: 3 neighbours per corner, from the apron, before merge; reference brute force | M2-01 | Matches brute force on 10k random corner configurations | `cargo test -p voxel-mesh ao_matches_bruteforce` | 1.5 d | [ ] |
| M2-08 | AO in the merge key and in the vertex; measure merge-ratio cost of AO-safety | M2-05, M2-07 | Merge ratio reported for AO-on vs AO-off; no AO artifacts in the golden images | `cargo test -p voxel-mesh ao_merge_ratio` | 0.5 d | [ ] |
| M2-09 | AO seam test: identical neighbours on both sides of a chunk border produce equal corner AO | M2-07 | Fails if the apron is off by one; passes with it | `cargo test -p voxel-mesh ao_seam` | 0.5 d | [ ] |
| M2-10 | Vertex packing to 16 B (local u16 ×3, material u16, face u8, AO u8 ×4) with encode/decode tests | M2-04 | Size asserted at compile time; round-trip all fields | `cargo test -p voxel-mesh vertex_pack` | 0.5 d | [ ] |
| M2-11 | Five golden mesh fixtures with content hashes (flat, cave, overhang, mixed material, chunk-border) | M2-10 | Hashes stable across runs; mismatch prints the first differing quad | `cargo test -p voxel-mesh golden` | 1 d | [ ] |
| M2-12 | Determinism across thread counts and chunk order | M2-11 | Same hash for 1, 2, 4, 8 workers; 10 repeats | `cargo test -p voxel-mesh --release determinism` | 0.5 d | [ ] |

### 6.2 Block light

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2-13 | Production full-pass block-light flood fill over 16³ cells with 2 channels | M1-29 | Equals the M1-29 reference on 5 fixtures | `cargo test -p voxel-mesh light_full` | 1.5 d | [ ] |
| M2-14 | Cross-chunk seeding from 1-block aprons; light does not leak through opaque blocks | M2-13 | Border fixture matches a single-chunk reference on both sides | `cargo test -p voxel-mesh light_apron` | 1 d | [ ] |
| M2-15 | Incremental light: dirty cells only, seeded from the edit, with a removal pass | M2-14 | Equals full recompute on 1 000 random edit sets, removals included | `cargo test -p voxel-mesh light_incremental` | 2 d | [ ] |
| M2-16 | Escalation rule: relight the chunk when the removal front reaches the dirty expansion boundary | M2-15 | An adversarial test forces escalation and still equals full | `cargo test -p voxel-mesh light_escalation` | 0.5 d | [ ] |
| M2-17 | Light border fixture and benchmark: full vs incremental cost | M2-16 | Incremental p95 ≤ 20% of full on a single edit; recorded | `cargo bench -p voxel-mesh light` | 0.5 d | [ ] |

### 6.3 Probes

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2-18 | Production SH4 bake at block centres, deterministic, budgeted across frames | M1-30 | Byte-equal to the reference on 3 fixtures | `cargo test -p voxel-mesh probe_bake` | 1.5 d | [ ] |
| M2-19 | Invalid-in-solid flag and 3×3×3 nearest-valid fallback | M2-18 | Sampling inside a wall returns the nearest valid probe; no NaNs | `cargo test -p voxel-mesh probe_fallback` | 0.5 d | [ ] |
| M2-20 | Probe LOD: near per block, mid half-res, far none; transition is a content event | M2-19 | Ring transition rebuilds once; counts match 512/64/0 per chunk | `cargo test -p voxel-mesh probe_lod` | 1.5 d | [ ] |
| M2-21 | Probe determinism and update budget: p95 ≤ 4 ms per edited chunk | M2-18 | Measured on the fixture; recorded | `cargo run -p voxel-bench -- gate probes` | 0.5 d | [ ] |

### 6.4 Integration, ray-test, and exit

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2-22 | Dirty bitset: per-block dirty flags derived from the COW diff (added/removed/changed) | M1-10 | Editing one voxel dirties exactly the block plus the AO/light apron | `cargo test -p voxel-mesh dirty_bitset` | 0.5 d | [ ] |
| M2-23 | Budgeted remesh scheduler: dirty blocks → mesh batches under a per-frame budget | M2-22 | Backlog drains; no frame exceeds the budget in the test | `cargo test -p voxel-mesh scheduler` | 1 d | [ ] |
| M2-24 | Scrub latency: single-block edit → remesh + light + probe ≤ 4 ms p95 | M2-23 | Measured; recorded | `cargo run -p voxel-bench -- gate edits` | 0.5 d | [ ] |
| M2-25 | Ray-test occlusion behind `occlusion.raytest` flag, using the SVDAG; default off | M1-33 | Flag on/off produce comparable visible sets; documented delta | `cargo test -p voxel-mesh --features raytest` | 2 d | [ ] |
| M2-26 | Ray-test vs flood-fill comparison report: accuracy and cost on 100 poses | M2-25 | Table in `docs/` with the decision recommendation (still default flood-fill) | review | 0.5 d | [ ] |
| M2-27 | Wire M2 into the viewer: replace the naive mesher; light + probes uploaded | M2-12, M2-21 | Visual parity with M1.5 except better merge ratio; 60 Hz; no new material keys, the M1.5-14 atlas is unchanged | run | 1 d | [ ] |
| M2-28 | Throughput: ≥ 20 terrain chunks/s single-threaded | M2-27 | Measured over 100 chunks; recorded | `cargo bench -p voxel-mesh build` | 0.5 d | [ ] |
| M2-29 | Scratch-memory budget test: ≤ 64 KiB per worker, no chunk-wide densify anywhere | M2-03 | Counting allocator test fails if exceeded | `cargo test -p voxel-mesh scratch_budget` | 0.5 d | [ ] |
| M2-30 | M2 exit: tag `m2`, update roadmap, freeze goldens as CI artifacts; record the 128³ Minimum run and close gate rows 9/11 | M2-29; M1 gate carry-over | Tag; CI runs the goldens; 128³ memory/edit/probe numbers and gate rows 9/11 recorded | `git tag -n m2`; gate report | 0.5 d | [ ] |

---

## 7. M2.5 — Worldgen

**Goal:** a deterministic 1024 km world with biomes, rivers, caves, landmarks and detail.
**Exit:** M2.5-32. **Budget:** 45.25 d code + 8 d art (M2.5-30/31), plus 40% contingency. **Depends:** M1-24, M2-12.
**Runs in parallel with M2–M3** with a second engineer; otherwise insert after M2.
**Art:** M2.5-30 (biome surfaces) and M2.5-31 (flora, structures, underground), scheduled in §2.1.
**Design:** [`worldgen-design.md`](worldgen-design.md).

### 7.1 Foundation and macro geography

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2.5-01 | Create `crates/engine/worldgen` (`voxel-worldgen`); deterministic integer-hash 2D/3D gradient noise with 16.16 fixed-point interpolation | M1-24 | Same bytes for 1/2/8 threads and on two machines; golden vectors committed | `cargo test -p voxel-worldgen noise` | 2 d | [ ] |
| M2.5-02 | Macro grid data model and 16 km tile format: height u16, temp/humidity/erosion/flags u8, header with seed + versions + CRC | M0-04 | Tile round-trips; 125² × 6 B = 93.75 KB; corrupt tile rejected | `cargo test -p voxel-worldgen tile` | 1 d | [ ] |
| M2.5-03 | Continentalness/erosion/height spline tables for 0–12,000 m; sea level 6,600 | M2.5-01 | Splines monotonic where required; height clamped; unit table tests | `cargo test -p voxel-worldgen splines` | 1.5 d | [ ] |
| M2.5-04 | Climate fields: compressed latitude (cold edges, warm equator), humidity from ocean distance, altitude lapse rate | M2.5-03 | Poles cold, coasts humid, peaks cold; 2D map test | `cargo test -p voxel-worldgen climate` | 1 d | [ ] |
| M2.5-05 | Macro tile generator at 128 m, seeded and content-hashed | M2.5-02…04 | 4,096 tiles generate in minutes; hash stable across runs | `cargo run -p voxel-worldgen -- macro --seed 1` | 2 d | [ ] |
| M2.5-06 | Hydrology: priority-flood depression fill, D8 flow, accumulation | M2.5-05 | Lakes at spill level; flow graph acyclic; hand-checked fixture | `cargo test -p voxel-worldgen hydro` | 2 d | [ ] |
| M2.5-07 | River extraction: vector polylines with width/depth/order from catchment; deltas | M2.5-06 | Every river reaches sea or a lake; width 10–200 m; no uphill segments | `cargo test -p voxel-worldgen rivers` | 2 d | [ ] |
| M2.5-08 | Lake extraction, spill levels, shoreline and underwater terrain | M2.5-06 | No leaking basins; shoreline continuous; tests | `cargo test -p voxel-worldgen lakes` | 1 d | [ ] |
| M2.5-09 | Macro tile store: tiled streaming, LRU, 3×3 border for interpolation | M2.5-02 | 32 km view holds ≤ 9 tiles; no seams at tile borders | `cargo test -p voxel-worldgen tile_store` | 1.5 d | [ ] |

### 7.2 Biomes, surface and transitions

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2.5-10 | Biome weight function: softmax over 18 centres in climate/erosion/continentalness; shape parameters blended before evaluation | M2.5-04, M2.5-05 | Weights sum to 1; transition width 300–500 m; no argmax curve in a map dump | `cargo test -p voxel-worldgen weights` | 2 d | [ ] |
| M2.5-11 | RON biome schema and the 18 definitions (11 land, 3 ocean, 3 underground, geothermal) | M2.5-30, M2.5-10 | All definitions validate; every climate cell resolves; every material key resolves in the manifest; content is data-only | `cargo test -p voxel-worldgen biome_ron` | 2 d | [ ] |
| M2.5-12 | Surface rules: 3–6 block soil default, biome/altitude override, slope and underwater rules | M2.5-30 | Cliffs expose stone; beaches ring water; every top/filler/stone/beach key comes from M2.5-30; tests | `cargo test -p voxel-worldgen surface` | 2 d | [ ] |
| M2.5-13 | 1 m material dither and canonical two-material + debris transition blocks | M2.5-12 | Seam is mixed, not a line; ≤ 4 variants per pair; zero new atlas slots, parents come from M2.5-30 | `cargo test -p voxel-worldgen transitions` | 2 d | [ ] |
| M2.5-30 | Author biome surface textures: the top/filler/stone/beach faces the 18 palettes need beyond M1.5-14; freeze each biome's `map_color`; record the slot table | M2.5-10 | A palette table names the top/filler/stone/beach key for each biome; every key is in the manifest; ≤ 24 new tiles; transitions add zero | `cargo test -p voxel-block texture_manifest`; `cargo run -p voxel-viewer -- bake-atlas`; review | 4 d | [ ] |

### 7.3 Caves, ores, landmarks and structures

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2.5-31 | Author underground, structure and flora textures: deep stone, fungal, lava, geothermal; planks, bricks, mossy cobble; birch, spruce, fern, cactus, giant variants | M2.5-30 | Every new key is in the manifest and referenced by the flora/structure/underground tables; at least one fixture uses each key; ≤ 32 new tiles; lava/geothermal carry the emissive flag | `cargo test -p voxel-block texture_manifest`; `cargo run -p voxel-viewer -- bake-atlas` | 4 d | [ ] |
| M2.5-14 | 3D cave carve: spaghetti, cheese, vertical shafts, galleries, water/lava caves | M2.5-12 | All five types present; balanced density; no caves in the first 2 m under water | `cargo test -p voxel-worldgen caves` | 3 d | [ ] |
| M2.5-15 | Cave depth relative to the local surface (down to −2,000 m) and underground biome assignment | M2.5-31, M2.5-14 | Caves follow terrain; aquifer/fungal/cavern assigned by depth and water; every underground material key resolves | `cargo test -p voxel-worldgen cave_depth` | 1 d | [ ] |
| M2.5-16 | Occasional overhangs and arches from 3D density; no disconnected floating rock | M2.5-14 | Arch fixture passes; no unsupported islands at the surface | `cargo test -p voxel-worldgen overhangs` | 1 d | [ ] |
| M2.5-17 | Ore placement by depth tier and vein | M2.5-14 | Metals + gems; no surface gems; vein continuity test | `cargo test -p voxel-worldgen ores` | 1 d | [ ] |
| M2.5-18 | Landmark anchors every 20–50 km: massif, canyon/falls, volcano, giant grove, monolith/arch | M2.5-05 | One landmark per cell; type fits terrain; map dump shows distribution | `cargo run -p voxel-worldgen -- map --landmarks` | 2 d | [ ] |
| M2.5-19 | Structure anchors on a 1–4 km jittered grid; prefab stamps for ruins, towers, dungeons, bridges/roads | M2.5-31, M2.5-18 | Stamps fit terrain; no floating foundations; most cells empty; every prefab uses M2.5-31 keys | `cargo test -p voxel-worldgen structures` | 2.5 d | [ ] |
| M2.5-20 | Cross-chunk feature determinism: a tree, structure or river crossing a border is identical from both sides | M2.5-19 | Byte-equal overlap from two adjacent chunks; 1,000 sampled features | `cargo test -p voxel-worldgen cross_chunk` | 1.5 d | [ ] |
| M2.5-21 | Flora placement: species, moderate density, biome-dependent, ecotones, rare giants | M2.5-31, M2.5-20 | Birch/scrub only at seams; giants 1 per 20–50 km; every species key comes from M2.5-31; density tests | `cargo test -p voxel-worldgen flora` | 1.5 d | [ ] |
| M2.5-22 | Surface detail pass: grass tufts, pebbles, leaves, ripples, moss, snow drift; canonical templates | M2.5-13 | Six types; templates dedup; detail only in LOD0; no new atlas keys, parents come from M2.5-30/31 | `cargo test -p voxel-worldgen detail` | 2 d | [ ] |

### 7.4 Pipeline, determinism and exit

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M2.5-23 | Chunk generation pipeline: block grid + feature list, no dense voxel array; p95 ≤ 2 ms | M2.5-21, M2.5-22 | 512 blocks/chunk; p95 ≤ 2 ms on the reference machine | `cargo bench -p voxel-worldgen chunk` | 2 d | [ ] |
| M2.5-24 | Worker pool, cancellation and prefetch integration; never generate on the edit path | M2.5-23 | Editing a distant voxel never blocks on generation; stale jobs cancel | `cargo test -p voxel-worldgen workers` | 1.5 d | [ ] |
| M2.5-25 | Generator versioning: save header pins seed, generator, macro and content versions; old versions remain callable | M2.5-23 | A v1 save regenerates v1 terrain after v2 lands | `cargo test -p voxel-worldgen versioning` | 1 d | [ ] |
| M2.5-26 | Determinism harness: golden chunk hashes across seeds, thread counts and two machines | M2.5-23 | 1/2/8 workers equal; 10 seeds; hashes committed | `cargo test -p voxel-worldgen --release determinism` | 1 d | [ ] |
| M2.5-27 | World creation CLI: progress, resume, macro map dump, 4k × 4k PNG preview | M2.5-05…09 | Full world in ≤ 10 min; resume after kill; preview renders | `cargo run -p voxel-worldgen -- create --seed 1` | 1 d | [ ] |
| M2.5-28 | Golden world fixtures: 4 × 4 km map render and a 1 km walkthrough strip | M2.5-30, M2.5-31, M2.5-23 | Screenshots in `docs/images/`; hashes stable; final art, no placeholder | review | 1 d | [ ] |
| M2.5-32 | M2.5 exit: world tour video, budget numbers recorded, tag `m2.5` | M2.5-28, M2.5-30, M2.5-31 | Tag; design doc updated with measured numbers; art tasks ticked | `git tag -n m2.5` | 0.25 d | [ ] |

---

## 8. M3 — Raster scale-up

**Goal:** a real world at real distance without GPU culling. **Exit:** M3-13. **Budget:** 9 d raw. **Depends:** M2.
**Art:** none; M3 reuses M1.5-14 and, if M2.5 ran, M2.5-31.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M3-01 | Streaming shell in the viewer: load ring, evict ring, budgeted per frame | M2-27 | 256 chunks load without a hitch > 4 ms | run + log | 1.5 d | [ ] |
| M3-02 | Mesh buffer pool: one arena with suballocation, or per-chunk buffers; document the choice | M3-01 | No per-frame allocation after warm-up; fragmentation reported | `cargo test -p voxel-viewer pool` | 1.5 d | [ ] |
| M3-03 | Upload queue with a per-frame byte budget | M3-02 | A 10 MB backlog drains without exceeding the budget | `cargo test -p voxel-viewer upload_budget` | 0.5 d | [ ] |
| M3-04 | CPU frustum cull of chunk AABBs — the stopgap and the M5 reference | M3-01 | Equals the M1-32 reference set on 100 poses | `cargo test -p voxel-viewer cpu_cull` | 1 d | [ ] |
| M3-05 | Draw loop over visible chunks with per-chunk draws | M3-04 | Correct image; no double-draw | run | 0.5 d | [ ] |
| M3-06 | Instrumentation: CPU submit, GPU pass, visible chunks/blocks/quads/bytes, upload bytes | M3-05 | Overlay or log line per frame; values match the scene | run | 0.5 d | [ ] |
| M3-07 | Frame breakdown report: where the 16.6 ms goes | M3-06 | Table in the PR; draw vs cull vs upload vs content | review | 0.5 d | [ ] |
| M3-08 | 256-chunk stress scene with terrain and canopy variants | M3-01 | Scene reproducible from a seed; screenshot; uses only M1.5-14 and, if M2.5 ran, M2.5-31 keys | run | 0.5 d | [ ] |
| M3-09 | Hitch test: upload p99 ≤ 4 ms over 1 000 frames of camera motion | M3-03 | Measured; no single frame > 8 ms | `cargo run -p voxel-bench -- hitch` | 1 d | [ ] |
| M3-10 | 60 Hz at 1080p at the gated edge on the reference machine | M3-09 | Recorded average and 1% low | run | 0.5 d | [ ] |
| M3-11 | Memory report: VRAM and host for the 256-chunk scene, rows split by DAG/mesh/overlay/light/probes | M3-10 | Fits the 40% VRAM budget with the 1.5× margin | run | 0.5 d | [ ] |
| M3-12 | Visual regression screenshot committed | M3-10 | Image in `docs/images/`; diff threshold documented | review | 0.25 d | [ ] |
| M3-13 | M3 exit: tag `m3`, freeze the scene config | M3-12 | Tag; config checked in | `git tag -n m3` | 0.25 d | [ ] |

---

## 9. M3.5 — Far field, water and map

**Goal:** 16–32 km view with a seamless near/far join, transparent water, and the atlas item.
**Exit:** M3.5-16. **Budget:** 17.75 d code + 1.5 d art (M3.5-15), plus 50% contingency. **Depends:** M2.5-09, M2.5-23, M3-13.
**Art:** M3.5-15 (water/current); M3.5-16 freezes the manifest.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M3.5-01 | Macro heightfield mesh builder with LOD rings (128/256/512 m) | M2.5-09, M3-13 | 32 km view ≤ 65 k vertices; rings stitch without cracks | `cargo test -p voxel-viewer farfield` | 2 d | [ ] |
| M3.5-02 | Biome colour and river/lake masks on the far mesh | M3.5-01 | Far colours match the near biome palette at the seam | visual | 1.5 d | [ ] |
| M3.5-03 | Near/far seam: 64–128 m blend band plus skirt; near chunks fade | M3.5-01 | No visible crack or pop over a 2 km flight | `cargo test -p voxel-viewer seam` | 2 d | [ ] |
| M3.5-04 | Far-field fog and lighting matched to the near renderer | M3.5-03 | Horizon reads as one world; screenshots | visual | 1 d | [ ] |
| M3.5-05 | 32 km view performance: frame time and draw budget | M3.5-03 | ≥ 60 Hz at 1080p at the target view | `cargo bench -p voxel-bench --bench render` | 1 d | [ ] |
| M3.5-06 | Transparent water: depth sorting, alpha, depth-based colour | M3.5-15, M3.5-03 | Two overlapping water planes sort correctly; no z-fighting; uses `water_still`/`water_flow` from M3.5-15 | `cargo test -p voxel-viewer water` | 2 d | [ ] |
| M3.5-07 | Underwater fog, caustics and surface view from below | M3.5-06 | Underwater reads as underwater; screenshots; caustics are procedural or the M3.5-15 tile | visual | 1 d | [ ] |
| M3.5-08 | River current: flow direction drives UV scroll and foam lines | M3.5-15, M3.5-06 | Rivers visibly flow; direction matches the macro network; only `water_flow`/`foam_strip` are new tiles | visual | 1 d | [ ] |
| M3.5-09 | Map atlas data: explored-chunk bitset, zoom levels, biome colours, landmarks | M2.5-23 | Atlas data round-trips; exploration fills it in | `cargo test -p voxel-worldgen atlas` | 1.5 d | [ ] |
| M3.5-10 | Map atlas item and UI: craftable, zoom, player marker | M3.5-09 | Usable in the viewer; screenshots; item icon and marker are UI sprites, not atlas tiles | run | 1.5 d | [ ] |
| M3.5-11 | Menu orbit view of the square slab and ocean rim | M2.5-27 | Pre-rendered/2.5D view in the menu; no gameplay renderer | run | 1 d | [ ] |
| M3.5-12 | Visual regression set: seam, water, far field, night | M3.5-15, M3.5-08 | Four screenshots with a documented diff threshold; final M3.5-15 water art | review | 1 d | [ ] |
| M3.5-13 | Streaming hitch test over a 2 km flight with water and far field | M3.5-03 | p99 ≤ 4 ms; no frame > 16.6 ms | `cargo run -p voxel-bench -- hitch` | 1 d | [ ] |
| M3.5-15 | Author water and current textures: `water_flow`, `foam_strip`, optional `caustics`; alpha/tint policy; flow axis and UV-scroll rate | M1.5-14 | Keys are in the manifest and atlas; alpha is smooth; flow axis and UV-scroll rate documented; caustics are procedural or a named tile | `cargo test -p voxel-block texture_manifest`; `cargo run -p voxel-viewer -- bake-atlas`; visual | 1.5 d | [ ] |
| M3.5-16 | M3.5 exit: tag `m3.5`, budgets recorded, material manifest frozen | M3.5-13, M3.5-15 | Tag; docs updated; manifest accounts for ids 0..N−1 with no gap and the unused tail; CI rejects a block key without a texture | `git tag -n m3.5` | 0.25 d | [ ] |

---

## 10. M4 — Versioned GPU resources and COW sync

**Goal:** the frozen fence/flip protocol, once, for roots and meshes. **Exit:** M4-18. **Budget:** 17.25 d raw. **Depends:** M1-13, M3-02.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M4-01 | Chunk-table slot v2: `rootA`, `rootB`, `versionIndex`, `block_table`; migrate readers | M1-22 | All lookups go through the version index; old slots rejected | `cargo test -p voxel-storage slot_v2` | 1 d | [ ] |
| M4-02 | Root pair over the shared pool: write inactive root, retire the old through the epoch list | M4-01 | Two roots coexist; `get` follows the index; refcounts balanced | `cargo test -p voxel-storage root_pair` | 1.5 d | [ ] |
| M4-03 | `VersionedResource` abstraction `{active, inactive, inFlight[2], fence}` shared by roots and meshes | M4-02 | One type, two users; state transitions asserted | `cargo test -p voxel-storage versioned` | 1 d | [ ] |
| M4-04 | Staging pass: all dirty chunks into their inactive targets, one pass per frame boundary | M4-03 | Batch of 64 chunks produces one staging pass; bytes counted | `cargo test -p voxel-storage staging` | 1 d | [ ] |
| M4-05 | Fence: submit, signal, CPU poll only at the boundary; no mid-frame wait | M4-04 | At most one fence wait per frame in the soak | `cargo test -p voxel-storage fence` | 1 d | [ ] |
| M4-06 | Flip: write `versionIndex` for every chunk in the batch only after the fence signals; lifecycle `READY` | M4-05 | A deliberately early flip fails the test; no chunk flips twice | `cargo test -p voxel-storage flip` | 1 d | [ ] |
| M4-07 | In-flight tracking per root (not a global counter) | M4-06 | A lagging frame delays recycle of exactly that root | `cargo test -p voxel-storage inflight` | 0.5 d | [ ] |
| M4-08 | Recycle rule `currentFrame − inFlightFrame >= 2`; a test that expects a third root if lag is 3 | M4-07 | Rule enforced; starvation case documented | `cargo test -p voxel-storage recycle` | 0.5 d | [ ] |
| M4-09 | Level-band upload: dirty run ranges per band, coalesced; no whole-chunk upload | M4-02 | Bytes uploaded proportional to changed runs, asserted | `cargo test -p voxel-render band_upload` | 1.5 d | [ ] |
| M4-10 | Versioned mesh buffers with the same protocol | M4-03, M3-02 | Mesh and root flip in the same batch; no frame sees a mismatched pair | `cargo test -p voxel-viewer mesh_version` | 1.5 d | [ ] |
| M4-11 | Shader reads `versionIndex` once per workgroup and binds that root for the whole dispatch | M4-01 | A synthetic mid-dispatch flip does not change the root read | `cargo test -p voxel-render version_once` | 0.5 d | [ ] |
| M4-12 | Compaction precondition wired to both roots and in-flight frames; assert plus fuzz | M1-12, M4-07 | Compaction during residency panics; fuzz finds no violation | `cargo test -p voxel-storage compaction_guard` | 1 d | [ ] |
| M4-13 | Soak harness: 1 000 edits/s for 10 000 frames with a per-frame content hash | M4-10 | No torn frame; hash sequence matches the CPU model | `cargo run -p voxel-bench -- soak --frames 10000` | 2 d | [ ] |
| M4-14 | Torn-frame detector: compare GPU readback of a sample region against the CPU model each frame | M4-13 | Detects a deliberately injected early flip | `cargo test -p voxel-bench torn` | 1 d | [ ] |
| M4-15 | Leak detector: refcounts, pool live count, VRAM/host RSS over the soak | M4-13 | Flat within 1% over 10k frames; invariant checker every 100 frames | soak output | 1 d | [ ] |
| M4-16 | Commit-cost benchmark: CPU per frame, fence waits, bytes | M4-13 | ≤ 0.1 ms CPU/frame, ≤ 1 wait/frame | `cargo bench -p voxel-storage commit` | 0.5 d | [ ] |
| M4-17 | Root-pair overhead benchmark | M4-02 | ≤ 5% of chunk DAG; recorded | `cargo bench -p voxel-storage root_pair` | 0.5 d | [ ] |
| M4-18 | M4 exit: tag `m4`, freeze the soak as an overnight job | M4-17 | Tag; soak script in CI/manual job | `git tag -n m4` | 0.25 d | [ ] |

---

## 11. M5 — GPU-driven culling

**Goal:** four dispatches, measured win. **Exit:** M5-24. **Budget:** 18.25 d raw, plus 50% GPU contingency. **Depends:** M4-10, M2-27, M1-33.

### 11.1 Occlusion and broad phase

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M5-01 | Port the block flood-fill to a compute pass; camera seed, horizon, 6-connected | M1-33 | Bit output equals the CPU reference on 100 poses | `cargo test -p voxel-render occlusion_parity` | 2 d | [ ] |
| M5-02 | Block-bit buffer layout (1 bit/block), clear and read helpers | M5-01 | Size matches `blocks/chunk × chunks`; no out-of-range writes | `cargo test -p voxel-render block_bits` | 0.5 d | [ ] |
| M5-03 | Horizon and seed policy: camera block, frustum entry cells, beyond-horizon = visible | M5-01 | Objects beyond 128 m are frustum-culled only; asserted | `cargo test -p voxel-render horizon` | 0.5 d | [ ] |
| M5-04 | Broad phase: one workgroup per resident chunk, root AABB vs 6 planes | M5-02 | Equal to CPU chunk cull on 500 poses | `cargo test -p voxel-render broad_parity` | 1 d | [ ] |
| M5-05 | Candidate compaction with an atomic counter; deterministic order or order-independent output | M5-04 | No gaps, count matches the CPU set size | `cargo test -p voxel-render compaction` | 0.5 d | [ ] |
| M5-06 | Indirect args buffer: workgroup count for traversal, vertex count for draw | M5-05 | Args match the candidate and append counts in a test | `cargo test -p voxel-render indirect_args` | 0.5 d | [ ] |

### 11.2 Traversal kernel

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M5-07 | Traversal kernel scaffold: 4 warps, one chunk per workgroup, version read once | M5-06, M4-11 | Runs on one chunk; no validation error; empty chunk exits | `cargo test -p voxel-render traversal_smoke` | 1 d | [ ] |
| M5-08 | Double-buffered groupshared frontier, 1 024 nodes, barrier per level | M5-07 | Level-by-level BFS matches a CPU BFS node list on 3 fixtures | `cargo test -p voxel-render frontier` | 1 d | [ ] |
| M5-09 | Warp-per-node, lanes 0–7 child mapping, lane 0 broadcast | M5-08 | Child set equals the mask expansion for all masks | `cargo test -p voxel-render warp_map` | 1 d | [ ] |
| M5-10 | Popcount child addressing in WGSL, matching M1-05 | M5-09 | WGSL test vectors match the Rust helper | `cargo test -p voxel-render popcount_wgsl` | 0.5 d | [ ] |
| M5-11 | Implicit AABB from origin and level, register-resident origin | M5-10 | Boxes match the CPU `aabb_extent` for all levels | `cargo test -p voxel-render aabb` | 0.5 d | [ ] |
| M5-12 | Frustum plane test with parent-inside skip flag | M5-11 | Equal to the CPU reference for 500 poses; skip flag counted | `cargo test -p voxel-render frustum_parallel` | 1 d | [ ] |
| M5-13 | Block bit test at `BLOCK_LEVEL`; skip the entire block before descending | M5-12 | Occluded blocks emit nothing; visible set equals reference | `cargo test -p voxel-render block_occlusion` | 0.5 d | [ ] |
| M5-14 | `lodMaxDepth` cutoff from the LOD policy | M5-13 | Stops at the configured depth; one extra level not read | `cargo test -p voxel-render lod_cutoff` | 0.5 d | [ ] |
| M5-15 | Append surviving blocks to the instance buffer with a stable layout | M5-13 | Count and contents equal the CPU reference | `cargo test -p voxel-render append` | 0.5 d | [ ] |
| M5-16 | Overflow policy: spill frontier as conservative visible blocks, count per chunk | M5-08 | Forced overflow emits a superset, never a subset | `cargo test -p voxel-render overflow` | 0.5 d | [ ] |
| M5-17 | Indirect draw consuming the append buffer | M5-15, M5-06 | Image equals the non-indirect path; zero extra draws | `cargo test -p voxel-viewer indirect_draw` | 1 d | [ ] |

### 11.3 Parity, tooling, and exit

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M5-18 | CPU parity harness: 500 poses × 3 fixtures, block-set equality | M5-17 | Zero mismatches; failure prints the first differing block | `cargo run -p voxel-bench -- cull-parity` | 1 d | [ ] |
| M5-19 | False-negative debug mode: draw everything and compare pixel coverage | M5-18 | No block visible in "draw all" is missing in culled mode | `cargo test -p voxel-render false_negative` | 1 d | [ ] |
| M5-20 | Visualisers: frontier occupancy, overflow, culled blocks, candidate list | M5-17 | Toggleable overlay; screenshot in docs | run | 1 d | [ ] |
| M5-21 | Cull performance: fill + broad + traversal ≤ 1.5 ms at 4 096 chunks | M5-17 | Measured on the reference GPU | `cargo bench -p voxel-bench --bench render` | 1 d | [ ] |
| M5-22 | Draw-work reduction ≥ 3× vs M3 on terrain and canopy | M5-17 | Measured; both fixtures | same | 0.5 d | [ ] |
| M5-23 | Kill-criterion report: if reduction < 1.5× after the timebox, stop and record why | M5-22 | Written decision either way | review | 0.5 d | [ ] |
| M5-24 | M5 exit: tag `m5`, freeze parity as a CI case | M5-23 | Tag; parity job runs on GPU runner or skips cleanly | `git tag -n m5` | 0.25 d | [ ] |

---

## 12. M6 — LOD, streaming, persistence

**Goal:** scale and survive a restart. **Exit:** M6-20. **Budget:** 22.25 d raw, plus 50% streaming contingency. **Depends:** M5, M3.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M6-01 | LOD policy experiment: depth-limited traversal vs explicit LOD meshes; measure both on 3 seeds | M5-24 | Table of VRAM, frame time, pop-in for both | report | 2 d | [ ] |
| M6-02 | Write ADR-009 amendment with the measured LOD decision | M6-01 | One page, numbers inline | review | 0.5 d | [ ] |
| M6-03 | Implement the chosen LOD path in traversal and streaming | M6-02 | LOD0/1/2 rings render at the target distances | run | 2 d | [ ] |
| M6-04 | Explicit LOD mesh generation (only if chosen): aggregate 2× per level, cache by content hash | M6-02 | LOD1/2 counts match the aggregation rule; dedup works | `cargo test -p voxel-mesh lod_build` | 2 d | [ ] |
| M6-05 | Streaming prefetch by camera velocity and ring crossing | M6-03 | Loads ahead of the camera; no visible pop within 128 m | run | 1.5 d | [ ] |
| M6-06 | Eviction policy by distance with hysteresis | M6-05 | No thrash at the ring boundary over 1 000 frames | `cargo test -p voxel-viewer eviction` | 1 d | [ ] |
| M6-07 | Per-frame streaming budget and backlog accounting | M6-05 | Backlog drains; p99 frame ≤ 16.6 ms | run | 0.5 d | [ ] |
| M6-08 | Disk header v2: magic, version, `CHUNK_BITS`, level table, compression flag, CRC | M1-13 | Old files rejected with a clear error; new round-trip | `cargo test -p voxel-storage header_v2` | 1 d | [ ] |
| M6-09 | Band serialization with level lengths and leaf arena; zstd optional | M6-08 | Round-trip on 5 fixtures; sizes recorded | `cargo test -p voxel-storage band_serde` | 1.5 d | [ ] |
| M6-10 | Persist block light cells with the chunk | M6-09 | Reload equals the pre-save light on 5 fixtures | `cargo test -p voxel-storage light_persist` | 1 d | [ ] |
| M6-11 | Persist probes at the current LOD with the chunk | M6-09 | Reload equals pre-save bytes; LOD tag stored | `cargo test -p voxel-storage probe_persist` | 1 d | [ ] |
| M6-12 | Dirty-region journal: record edited block cells so reopen can skip full relight | M6-10 | Journal replay equals a full relight on 20 random edit sets | `cargo test -p voxel-storage journal_light` | 2 d | [ ] |
| M6-13 | Reopen path: load bands + light + probes + journal, no full recompute | M6-12 | Reopen ≤ 50 ms/chunk on the fixture | `cargo run -p voxel-bench -- reopen` | 1 d | [ ] |
| M6-14 | Journal-vs-recompute equality and corruption handling | M6-12 | Corrupt journal triggers full recompute, not a bad light field | `cargo test -p voxel-storage journal_corrupt` | 0.5 d | [ ] |
| M6-15 | Seam policy at chunk and LOD borders: stitch or skirt; pick one and document | M6-03 | No visible crack on the seam fixture | visual + test | 1.5 d | [ ] |
| M6-16 | Seam fixture: deliberate 1-level LOD difference across a border | M6-15 | Pixel diff under the threshold; screenshot | `cargo test -p voxel-viewer seam` | 0.5 d | [ ] |
| M6-17 | VRAM ring budget test at LOD0 128 m + LOD1 256 m + LOD2 512 m | M6-03 | Within 40% of VRAM at the reference machine | `cargo run -p voxel-bench -- gate memory` | 1 d | [ ] |
| M6-18 | Streaming hitch p99 ≤ 4 ms over a 2 000-frame flight | M6-07 | Measured; no > 16.6 ms frame | `cargo run -p voxel-bench -- hitch` | 1 d | [ ] |
| M6-19 | Reopen benchmark and recorded history | M6-13 | ≤ 50 ms/chunk p95; recorded | `scripts/bench.sh LABEL=m6-reopen` | 0.5 d | [ ] |
| M6-20 | M6 exit: tag `m6`, update the roadmap and the rings table | M6-19 | Tag; spec/roadmap updated; atlas bake hash equals the M3.5-16 freeze | `git tag -n m6` | 0.25 d | [ ] |

---

## 13. M7 — Hardening and budgets

**Goal:** the budgets are enforced, not aspirational. **Exit:** M7-14. **Budget:** 12 d raw, plus 30% contingency. **Depends:** M6.

| ID | Task | Depends | Done when | Verify | Est | Done |
| --- | --- | --- | --- | --- | --- | --- |
| M7-01 | CI matrix: fmt, clippy, both-edge builds, workspace tests, docs | M1-24 | All jobs required and green | CI | 1 d | [ ] |
| M7-02 | naga validation for every shader on every PR | M7-01 | A broken shader fails CI | CI | 0.5 d | [ ] |
| M7-03 | GPU test runner (self-hosted) with graceful skip; parity job on demand | M7-01 | GPU jobs run nightly; skip path tested | CI | 1 d | [ ] |
| M7-04 | Bench regression script: compare to the last tagged run; > 10% on a stable metric fails review | M7-01 | Script exits non-zero on a seeded regression | `scripts/bench-regress.sh` | 1 d | [ ] |
| M7-05 | Memory-budget enforcement: fail the build when the gate report exceeds the pre-registration | M7-04 | A seeded over-budget fixture fails | CI | 0.5 d | [ ] |
| M7-06 | Chunk inspector: node counts, run dedup, overlay entries, light/probe state per chunk | M4-15 | Usable from the viewer; screenshots | run | 1.5 d | [ ] |
| M7-07 | Layout/band viewer: level bands, padding, dedup ratio | M1-18 | Visual and numeric output | run | 1 d | [ ] |
| M7-08 | Cull visualiser: show kept/culled chunks and blocks, Hi-Z-like occupancy | M5-20 | Toggleable; documented | run | 0.5 d | [ ] |
| M7-09 | Light debug view: block cells, probes, AO | M2-17 | Toggleable; documented | run | 1 d | [ ] |
| M7-10 | Overnight soak job: 8 h edit + camera motion, invariant check every minute | M4-13 | Passes; artifacts archived | script | 1 d | [ ] |
| M7-11 | Operations guide: run, benchmark, inspect, recover a corrupt save | M6-14 | Doc covers each flow with commands | review | 1 d | [ ] |
| M7-12 | Clean-checkout smoke: clone, build, run viewer, record a bench run | M7-01 | Script passes on a fresh clone | `scripts/smoke.sh` | 1 d | [ ] |
| M7-13 | M7 exit: tag `v0.1`, update all docs and the definition of done | M7-12 | Tag; README quick start works; material manifest frozen | `git tag -n v0.1` | 0.5 d | [ ] |
| M7-14 | Post-mortem: what the gate predicted vs what shipped; list of reopened assumptions | M7-13 | One page; feeds the next roadmap | review | 0.5 d | [ ] |

---

## 14. Cross-cutting checklists

Use these in review when a task touches the relevant area.

### Layout (every M1 task)

- [ ] No literal chunk size, level count or block count outside `constants`.
- [ ] Every child read is `base + popcount(...)`; no pointer chase per child.
- [ ] Full runs are 64 B aligned; partial-run padding is counted.
- [ ] A node never leaves its band; compaction asserts no live references.
- [ ] `check()` runs in tests and in debug builds after every mutation.

### Memory (every M2–M6 task)

- [ ] No chunk-wide dense allocation; block-granular scratch only.
- [ ] Counted bytes are reported per component, not just the total.
- [ ] Worst case (cave, mixed material) is measured, not only terrain.
- [ ] p95, not the mean, is compared to the pre-registered budget.

### Synchronisation (every M4–M5 task)

- [ ] `versionIndex` read once per workgroup.
- [ ] Flip only after the fence; no mid-frame commit.
- [ ] Recycle respects `inFlightFrame` per root.
- [ ] No per-frame readback on the critical path.
- [ ] Lifecycle state transitions are asserted, not inferred.

### Correctness oracles (every M2–M5 task)

- [ ] The reference implementation from M1 still passes against the production path.
- [ ] Golden hashes are updated only with a written reason.
- [ ] Determinism across thread counts is re-run.

### Materials and textures (every task that adds a block key or a tile)

- [ ] The key is in the manifest with a 256² source, hash, alpha mode and slot; no runtime fallback face.
- [ ] Air is 0; the drawable count is ≤ 256 and the slot table is updated; no gap in the used range.
- [ ] Alpha matches `RenderType`: binary for `Cutout`, smooth for `Translucent`.
- [ ] The tile reads at a 1 m face and at mip 4; no new cross-tile bleed.
- [ ] Transition, detail and damage variants reuse parent tiles; zero new slots.
- [ ] `bake-atlas` hash and the screenshot are updated in the same PR.

---

## 15. Traceability

| Spec section | Tasks |
| --- | --- |
| §1 units / edge | M0-04, M1-19…M1-26 |
| §2 tiers | M4-01…M4-10, M6-08…M6-13 |
| §3 COW protocol | M1-10, M4-01…M4-08, M4-12 |
| §4 culling | M1-32…M1-34, M5-01…M5-17 |
| §5 state | M0-16, M4-07, M5-04, M6-05 |
| §6 mesher + atlas | M0-06, M1.5-06…M1.5-14, M2.5-30/31, M3.5-15, M2-01…M2-12 |
| §7 light + probes | M0-07, M1-29, M1-30, M2-13…M2-21, M6-10…M6-12 |
| §8 sync | M4-04…M4-11 |
| §9 gate | M0-02, M1-35…M1-43 |
| §10 layout | M0-05, M1-01…M1-18 |
| §11 traversal | M5-07…M5-17 |
| §12 ownership | M0-21, M4-11, M5-18, M7-03 |

---

## 16. Backlog (not scheduled)

- Far-field raymarching or impostors for silhouette detail.
- GPU-side remeshing from the append buffer.
- Bindless materials and texture-array fallback if 256 slots is exceeded.
- Damage crack rendering (vertex AO + procedural shader).
- Editor, networking, audio, physics.
- Console/mobile targets.
- A second `CHUNK_BITS` axis beyond one feature flag (e.g. per-world chunk size) — explicitly rejected unless a product need appears.

---

## 17. Budget summary

Raw days are the sum of the task estimates. Contingency is milestone-specific: it is low where the work is mechanical and high where it is research or GPU-dependent.

| Milestone | Raw days | Contingency | Planning days | Sprints | Exit |
| --- | ---: | ---: | ---: | ---: | --- |
| M0 | 7.5 | 0% | 7.5 | 1 | gate frozen |
| M1 | 33.5 | 25% | 41.9 | 4 | 64³/128³ decided |
| M1.5 | 14.5 | 20% | 17.4 | 1.5 | first pixel |
| M2 | 27.0 | 25% | 33.75 | 3 | content pipeline |
| M2.5 | 53.25 | 40% | 74.55 | 3 | 1024 km world with biomes, rivers, caves |
| M3 | 9 | 40% | 12.6 | 1.5 | 256 chunks at 60 Hz |
| M3.5 | 19.25 | 50% | 28.9 | 1.5 | 16–32 km view, water, atlas |
| M4 | 17.25 | 30% | 22.4 | 2 | sync soak clean |
| M5 | 18.25 | 50% | 27.4 | 3 | ≥ 3× draw reduction |
| M6 | 22.25 | 50% | 33.4 | 3 | 512 m within budget |
| M7 | 12 | 30% | 15.6 | 2 | enforced budgets |
| **Total** | **233.75** | — | **315.3** | **27** | |

Milestone rows include their art tasks: 4 d in M1.5, 8 d in M2.5 and 1.5 d in M3.5. There is no separate art track. 233.75 raw days is ~47 weeks; 315.3 planning days is ~63 weeks (~15 months). The **249 tasks** (245 previous + 4 art) are the complete delivery set; the last sprint is unallocated buffer and must not be filled with new scope.

The gate is at S4 because it must not inherit the later milestones' contingency. If it slips, the correct response is to cut reference scope (M1-30, M1-31 can be stubs for the first pass), not to weaken the gate.
