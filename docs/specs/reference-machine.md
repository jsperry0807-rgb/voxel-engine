# Reference Machine and Profiles

**Status:** Frozen 2026-10-26 — the machine every gate number is measured on
**Applies to:** M1 gate, all benchmarks recorded in `benchmarks/HISTORY.md`

---

## 1. The machine

| Part | Value |
|---|---|
| CPU | AMD Ryzen 5 7600, 6 cores / 12 threads |
| GPU | NVIDIA GeForce RTX 4060, **8,188 MiB VRAM** |
| RAM | 32 GB |
| OS | Linux (primary) |
| Display | 1920 × 1080 |
| Storage | SSD, ≥ 40 GB free for worlds and builds |

The reference machine is also the development machine. Every number quoted by a
gate names it. If a run happens on different hardware, it is not a gate run.

## 2. Platforms

- **Linux is the primary development and test platform now.**
- **Windows is required before public release.** No Windows-specific code in the
  engine crates; wgpu and winit carry the portability. CI cross-builds the
  workspace for `x86_64-pc-windows-gnu` from M2 onward (the job installs
  mingw-w64); native windowing, pointer capture and GPU behaviour still have to
  be tried on Windows.
- macOS is not a target.

## 3. Gated profiles

The gate measures **Minimum** and **Default**. High is a future profile and is not
gated on this machine.

| Profile | Gated | Resolution | Far clip | Near voxel radius | Vertical shell | LOD rings | Detail |
|---|---|---:|---:|---:|---:|---|---|
| **Minimum** | yes | 1080p | 8 km | 64 m | ±16 m | 64 / 128 / 256 m | reduced |
| **Default** | yes | 1080p | 16 km | 128 m | ±16 m | 128 / 256 / 512 m | full |
| High | no | 1440p / 4K | 32 km | 256 m | ±32 m | 256 / 512 / 1024 m | full+ |

"Near voxel radius" is a horizontal disc around the camera; beyond it the LOD rings
and then the macro heightfield take over. The vertical shell follows the surface;
the cave cache covers descents.

## 4. What "60 fps" means

- **Average 60 fps**, **1% low 60 fps** — every frame capped at 16 ms — at 1080p native.
- Frame time budget is 16 ms; every budgeted stage is capped at 16 ms and no frame
  may exceed it.
- Measured over a scripted 2 km flight and a 60 s cave walk, not a static camera.

The frame-time lens reads a viewer trace rather than a synthetic loop:

```sh
VOXEL_FRAME_PROFILE=target/frames.csv cargo run --release -p voxel-viewer
cargo run --release -p voxel-bench -- frame --input target/frames.csv --warmup 120
```

The report prints the average, 1% low, p50/p95/p99, longest frame and hitch
count. It passes only when the average and 1% low are both on target and no
frame exceeds 16 ms; a 60 fps average with one 200 ms hitch fails.

## 5. Non-negotiables

- Native 1080p + TAA. No FSR/DLSS/XeSS dependency at launch.
- No real-time shadows. Sun direction, baked AO and probes carry the lighting.
- Windows build green before public release.
