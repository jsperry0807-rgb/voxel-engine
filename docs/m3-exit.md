# M3 exit: the streaming slice

**Date:** 2026-11-01
**Milestone:** M3 (streaming slice), tasks M3-00…M3-13
**Tag:** `m3`
**Frozen config:** [`benchmarks/m3-scene.json`](../benchmarks/m3-scene.json) — enforced by `tests/scene_config.rs` in both crates
**Gate archive:** [`benchmarks/runs/20261101T175113Z-m3-gate-128`](../benchmarks/runs/20261101T175113Z-m3-gate-128) (minimum profile, plus all three scale-up profiles)
**Reference image:** [`images/m3-12-reference.png`](images/m3-12-reference.png)

## 1. Verdict

**The M3 slice is measured, with two open conditions.**

It does what the milestone asked: 256 generated chunks stream at 128³ without a
frame-thread hitch, the CPU cull is the measured M1-32 reference, the scene is
seeded and reproducible, and the numbers exist for both reference GPUs. Memory
is 3.4% of the VRAM target.

The two conditions are **not** regressions in this slice's code; they are the
boundary of what it proves:

1. **The Radeon iGPU misses the 60 Hz 1% low at 128³** — 56.6–61.2 fps against a
   60 fps bar, while its average (70–75 fps) clears it. The 4060 clears both by
   1.7–3×.
2. **"128³ green" holds at the gated minimum view only.** The storage projection
   is 2.98 GiB at the minimum profile (805 chunks), but 11.93 GiB at default and
   95.4 GiB at high. The M3 *render* scene is 107.5 MiB because it uploads no
   DAG or overlay; that does not change the storage model's projection.

Both belong to later milestones: the iGPU tail to M3.5/M5 (LOD, indirect, or a
cheaper present path), and the view-distance question to M5/M7.

Carried unchanged from M1/M2: gate row 11 (a real populated save) is still not
measured, and the 64³ path is abandoned.

## 2. What M3 delivered

| Task | Result |
|---|---|
| M3-00 mesh compression | 44 B/quad packed 8 B vertex; 128³ row 6 **2,115 KiB PASS**; 64³ abandoned |
| M3-01 streaming shell | 256 chunks, **max pump 0.09 ms, 0 hitches** over 4 ms |
| M3-02 mesh pool | one paged arena, **13 × 8 MiB = 104.0 MiB for 100.5 MiB** of meshes, 0 holes, no per-frame buffer creation |
| M3-03 upload queue | budgeted staging copies; backlog capped at 32 MiB; **0 overruns** in the ring fill |
| M3-04 CPU cull | equals the M1-32 reference on 100 poses; **93 of 256** chunks drawn at the gate pose |
| M3-05 draw loop | culled frame **byte-identical** to a draw-everything frame; no duplicate visits |
| M3-06 instrumentation | per-frame CPU/GPU/geometry counters, in-pass timestamp queries |
| M3-07 frame breakdown | 1080p: **5.5 ms (4060) / 13.7 ms (iGPU)**; cull 0.05 ms; content off-thread |
| M3-08 stress scene | seeded scene, **5 terrain variants + 2 canopy densities**, 100.5 MiB ring, 5 s load |
| M3-09 hitch test | upload **p99 2.5–2.9 ms (4060) / 1.6–3.1 ms (iGPU)**, worst frame 6.2–6.4 / 5.7–6.1 ms; budget halved to 512 KiB |
| M3-10 60 Hz | **192–202 fps avg / 104–113 fps 1% low (4060)**; iGPU 70–75 avg / **56.6–61.2 1% low** |
| M3-11 memory | **107.5 MiB VRAM + 16.8 MiB host**; 3.4% of the 3.20 GiB target, 29.8× headroom |
| M3-12 visual regression | reference image + `image-diff` threshold; same-build captures byte-identical |
| M3-13 exit | this record, the frozen config and the `m3` tag |

## 3. The frozen scene

`benchmarks/m3-scene.json` is the single source for the frozen values, and
`crates/engine/viewer/tests/scene_config.rs` fails if the code and the file
disagree. Both chunk edges are named: the gate is 128³, the default build stays
at 64³.

| Knob | Value |
|---|---|
| Scene seed | `0x5EED1234` (1592594996) |
| Chunk edge | **128** (gate, `--features chunk128`); 64 (default build) |
| Ring | 16 × 16 = **256 chunks** (`RING_HALF = 8`) |
| Pump budget | 4 ms |
| Upload budget | **512 KiB** per frame |
| Staging chunk | 2 MiB |
| Upload backlog cap | 32 MiB |
| Gate resolution | 1920 × 1080 |
| Gate pose | eye `(14, 6, 20)`, target `(2, 1, 2)` |
| Frame-time window | 1200 frames, 12 settle frames |
| Reference image | `docs/images/m3-12-reference.png` |
| Image threshold | ≤0.01% beyond 2/255, max 32/255, mean ≤0.5/255 |
| VRAM margin | 1.5× inside the target |

## 4. Reproducing the exit

```sh
# Gate report plus all three 128³ projections, archived
GATE=1 FEATURES=chunk128 \
  GATE_MANIFEST=benchmarks/fixtures/m2-128/manifest.json \
  GATE_PROFILE=minimum LABEL=m3-gate-128 scripts/bench.sh

# 60 Hz at the frozen pose
cargo build --release -p voxel-viewer --features chunk128
VOXEL_FRAMETIME=target/frametime.json \
VOXEL_FRAMETIME_EYE=14,6,20 VOXEL_FRAMETIME_TARGET=2,1,2 \
  ./target/release/voxel-viewer

# The reference capture and its check
VOXEL_SCREENSHOT=target/capture.png \
VOXEL_SCREENSHOT_EYE=14,6,20 VOXEL_SCREENSHOT_TARGET=2,1,2 \
  ./target/release/voxel-viewer
cargo run --release -p voxel-bench -- image-diff \
  --reference docs/images/m3-12-reference.png --candidate target/capture.png

# Scene memory, upload hitch
cargo run --release -p voxel-bench --features chunk128 -- scene-memory
cargo run --release -p voxel-bench --features chunk128 -- hitch
```

## 5. What is not in M3

- **No far field, water, fog or map** — M3.5.
- **No versioned GPU roots or COW sync** — M4; that is also when the DAG and
  overlay enter VRAM, which is when the storage projection starts applying to the
  renderer.
- **No LOD or indirect draws** — M5; the levers for the iGPU tail and for the
  default/high view distances.
- **No worldgen** — M2.5 (not run); the scene is analytic and seeded, not
  generated by the worldgen design.
- **64³ is abandoned** as a product edge (M3-00); it remains the default build
  for the fast tests.

## 6. Tag

```sh
git tag -n m3
```

The tag carries this record's headline: the slice is measured; the iGPU 1% low at
128³ and the default/high view-distance projections are open.
