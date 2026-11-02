# M3-12 visual regression screenshot

**Status:** landed 2026-11-01. The reference is
[`images/m3-12-reference.png`](../../images/m3-12-reference.png) — the M3-08 stress
scene at the pinned pose, 1920×1080 — and `voxel-bench image-diff` checks a new
capture against it with a documented threshold. Two captures from the same build
are **byte-identical**; a capture from the M3-08 build differs in **one pixel**.

## Acceptance

| Requirement | How |
|---|---|
| Image in `docs/images/` | [`m3-12-reference.png`](../../images/m3-12-reference.png), 1920×1080 RGBA8 (1.4 MiB) |
| Diff threshold documented | table below, enforced by `voxel-bench image-diff` (exit 1 outside it) |

## Reproducing the reference

```sh
cargo build --release -p voxel-viewer --features chunk128
VOXEL_SCREENSHOT=docs/images/m3-12-reference.png \
VOXEL_SCREENSHOT_EYE=14,6,20 \
VOXEL_SCREENSHOT_TARGET=2,1,2 \
  ./target/release/voxel-viewer
```

Default scene seed (`0x5EED1234`), default upload budget, 1920×1080 drawable, the
128³ gate build. The pose is the one M3-08 committed, so the image, the 60 Hz
numbers ([`m3-10-60hz.md`](m3-10-60hz.md)) and the scene description all refer to
the same frame.

## Checking a capture

```sh
./target/release/voxel-bench image-diff \
  --reference docs/images/m3-12-reference.png \
  --candidate target/capture.png
```

```
0 of 2073600 pixels differ beyond 2 (0.0000%), max delta 0, mean abs 0.0000;
threshold: fraction <= 0.0001 (207 px), max delta <= 32, mean abs <= 0.50 -> PASS
```

`--json` prints the same fields; the command exits 1 outside the threshold.

## The threshold

| Limit | Value | Why |
|---|---|---|
| Per-channel tolerance | **2 / 255** | below this is capture noise; a 1–2 level shift is invisible and comes from the compositor |
| Differing fraction | **≤ 0.01%** (207 of 2,073,600 px) | one stray pixel or a cursor artifact must not fail the build; a dropped chunk, wrong material or broken corner is thousands of pixels |
| Max channel delta | **≤ 32 / 255** | a single pixel may flip on a raster tie; a 32-level cap keeps the allowance from hiding a real change |
| Mean absolute delta | **≤ 0.5 / 255** | catches a subtle global shift (lighting, exposure) that stays under the per-pixel limits |

All four must pass. The renderer is deterministic on the reference machine, so
the threshold is sized for the *capture path*, not for the rasteriser:
`CommandEncoder`-ordered geometry, a fixed pose, a fixed seed and no timestamp
queries inside the draw.

### Measured variance

| Pair | Differing pixels | Max delta | Mean abs |
|---|---:|---:|---:|
| Two captures from the current build | **0** | 0 | 0 |
| Current build vs the M3-08 milestone capture | **1** | 18 | 0.0000043 |

The single pixel is inside the allowance (1 ≤ 207, 18 ≤ 32). It is what a
cross-build comparison realistically costs; the same-build determinism is exact.

## Failure guidance

A failure means the frame changed. In order:

1. **Look at both images.** `differing_fraction` tells you the scale: >1% is a
   scene or pipeline change, a handful of pixels is a tie or an artifact.
2. **Check the inputs**: the seed (`VOXEL_SCENE_SEED`), the chunk feature
   (`chunk128`), the atlas, the pose and the drawable size. A capture at the
   wrong size is reported as a size mismatch, not as differences.
3. If the change is intended (a scene edit, a lighting fix), regenerate the
   reference **in the same commit as the change**, and say so in the message.
   A reference refresh without a code change is a smell.

## Tests

```bash
cargo test -p voxel-bench --lib image_diff
cargo test -p voxel-bench --test image_diff
```

- the comparison pins identical images at zero delta, a single pixel inside the
  allowance, a 1%-of-frame change failing, a 2-level delta being ignored, a size
  mismatch being an error rather than a difference, and the threshold being
  configurable;
- the artifact test loads the committed reference, checks it is 1920×1080 RGBA8,
  and compares it against itself.

## Limits

- **One frame, one pose, one seed.** M3.5-12 grows this into a four-image set
  (seam, water, far field, night).
- **Not a perceptual metric.** A global 1-level tint fails the mean check by
  design; a deliberate look change means regenerating the reference.
- **WSL/Dozen-specific variance.** The measured 0/1-pixel numbers are for the
  reference machine; another driver may need a wider tolerance, which the flags
  make explicit rather than silent.
- The comparison is not wired into CI; CI has no adapter. It is a review step,
  as the milestone's `review` verification says.
