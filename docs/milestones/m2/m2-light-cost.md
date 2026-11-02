# M2-17 — Full vs Incremental Light Cost

**Status:** Complete — 2026-10-29. Target met at 5.7% p95 (128³), down from the
31.6% miss recorded earlier.
**Task:** M2-17, light border fixture and full-vs-incremental benchmark
**Host:** AMD Ryzen 5 7600, 4 vCPU visible (reference-machine class). Absolute
times are a development check; the ratio and the changed-cell counts are what the
task asks for.

The optimization path and its measured charts are in
[`m2-light-20pct.md`](m2-light-20pct.md).

## Command

```sh
cargo bench -p voxel-mesh --bench light
```

The bench measures both production light edges — 4³ cells at the 64³ chunk edge
and 8³ cells at 128³ — regardless of which edge the crate is compiled at, because
the light code is edge-agnostic.

## Fixture

A roofed chunk with a sky shaft at `(0, 0, 0)` and a torch in the one-cell apron
at `(-1, edge/2, edge/2)`, so both channels enter across a chunk border. An
interior pillar keeps the propagation from being a bare box. The measured edit
adds a level-15 emitter at the chunk centre.

The benchmark asserts, once per edge:

- the incremental state equals a fresh full pass before the edit;
- the incremental state equals a fresh full pass after the edit;
- `full_relights() == 0`, so the measured edit really took the incremental path.

## Method

- **Full:** `full_pass_at(edge, [0, 0, 0], &world)`.
- **Incremental:** snapshot a built `LightState`, restore it with a
  capacity-preserving `clone_from` outside the timed section, then time
  `apply_edit` for the single emitter addition. State construction is not part
  of an edit.
- p50 and p95 over 10,000 timed samples; criterion also reports the mean.

The M2-17 threshold is incremental p95 ≤ 20% of full.

## Results

| Edge | Full p50 / p95 | Incremental p50 / p95 | Ratio p50 / p95 | Changed cells |
|---|---:|---:|---:|---:|
| 64³ (4³ cells) | 4.88 / 6.39 µs | 0.261 / 0.280 µs | 5.3% / **4.4%** | 46 / 216 (21%) |
| 128³ (8³ cells) | 28.81 / 36.12 µs | 2.00 / 2.06 µs | 7.0% / **5.7%** | 414 / 1,000 (41%) |

Criterion means from the same run:

| Edge | `full_pass` | `incremental_edit` | Mean ratio |
|---|---:|---:|---:|
| 64³ | 4.905–4.931 µs | 256.8–260.8 ns | 5.25% |
| 128³ | 28.804–28.912 µs | 2.030–2.044 µs | 7.06% |

The 128³ edge clears the 20% limit with a 3.5× margin. The single-edit sweep in
[`m2-light-20pct.md`](m2-light-20pct.md) puts every tested edit under 10% p95 at
128³, including the border-side edit that changes 570 of 1,000 padded cells.

## What changed

Before the optimization work `apply_edits` always reseeded and re-propagated the
whole dirty region. At 4³/8³ cells the 15-cell light radius covers the chunk, so
a single edit measured **139% (64³) / 128% (128³)** of a full pass — the
incremental path was slower than rebuilding.

The changes, in order:

1. **Increase-only fast path** — a batch with no removal propagates increases
   from the changed cells instead of clearing and refilling the region. Removals
   keep M2-16's escalation rule.
2. **Channel-selective propagation** — only the channel the batch touched is
   propagated; a pure gain seeds the edited cell, not its neighbours.
3. **Scratch reuse** — the 16 bucket vectors and seed lists live on
   `LightState`, and a custom `clone_from` keeps their capacity.
4. **Neighbour mask** — one byte per padded cell with six direction bits and
   six constant index offsets; the inner loop needs no coordinate division or
   bounds checks. The 1 KB mask replaced a 24 KB index-table prototype at the
   same speed.

`tests/light_increase.rs` pins emitter additions, opacity decreases, sky gains
and apron edits to a full pass with `full_relights() == 0`; the 1,000-set random
test and the escalation test continue to pass.

## Flamegraphs

The 2026-10-29 pprof captures are committed:

- [`profile/m2-17-20pct.html`](profile/m2-17-20pct.html) — the browser report
  with the charts, the four changes and the final flamegraph.
- [`profile/m2-17-light-incremental.svg`](profile/m2-17-light-incremental.svg) —
  the final incremental edit.
- [`profile/m2-17-light-incremental-before-table.svg`](profile/m2-17-light-incremental-before-table.svg)
  — after scratch reuse, before the neighbour mask.
- [`profile/m2-17-light-full.svg`](profile/m2-17-light-full.svg) — one full pass.

The final incremental graph has no allocator hotspot; the remaining samples are
the propagation loop. The pre-mask graph shows the coordinate-heavy loop.

## Verdict

**M2-17's 20% limit is met.** The production 128³ edge measures 5.7% p95
(7.06% by criterion mean), and the worst edit in the six-position sweep is 7.5%
p95. The locality issue from the first measurement is gone: the optimized
incremental path handles a 570-cell change without approaching the threshold.

Removals still escalate to a full relight at production edges (M2-16); that path
is not part of the 20% ratio.

## Reproduce

```sh
cargo bench -p voxel-mesh --bench light
cargo test -p voxel-mesh --test light_increase --test light_incremental
```
