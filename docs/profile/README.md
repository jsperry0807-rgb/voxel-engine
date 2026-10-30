# Profiles and M2-17 report

- [`m2-17-20pct.html`](m2-17-20pct.html) — the browser report: result, charts,
  the four optimization phases and the final flamegraph.
- [`m2-17-changed-cells.svg`](m2-17-changed-cells.svg) — share of a full pass
  against the number of padded cells an edit changes.
- [`m2-17-phases.svg`](m2-17-phases.svg) — p95 cost and ratio across the phases.
- [`m2-17-light-incremental.svg`](m2-17-light-incremental.svg) — final
  incremental pprof flamegraph.
- [`m2-17-light-incremental-before-table.svg`](m2-17-light-incremental-before-table.svg)
  — after scratch reuse, before the neighbour mask.
- [`m2-17-light-full.svg`](m2-17-light-full.svg) — full-pass flamegraph.
- [`m2-17-matrix-before.csv`](m2-17-matrix-before.csv),
  [`m2-17-matrix-after.csv`](m2-17-matrix-after.csv),
  [`m2-17-phases.csv`](m2-17-phases.csv) — raw measurements.

The SVGs are standalone and interactive (hover, click-to-zoom, search). Open the
HTML in a browser:

```sh
explorer.exe "$(wslpath -w docs/profile/m2-17-20pct.html)"
# or
python3 -m http.server 8100 --directory docs/profile
# http://localhost:8100/m2-17-20pct.html
```

Regenerate the charts:

```sh
python3 scripts/m2_17_charts.py
```

Regenerate the flamegraphs:

```sh
LIGHT_SKIP_P95=1 LIGHT_PROFILE=1 \
  cargo bench -p voxel-mesh --bench light -- \
  "light/edge_8/incremental_edit" --profile-time 10
LIGHT_SKIP_P95=1 LIGHT_PROFILE=1 \
  cargo bench -p voxel-mesh --bench light -- \
  "light/edge_8/full_pass" --profile-time 10
```

pprof writes the SVGs under
`target/criterion/light_edge_8/<bench>/profile/flamegraph.svg`.
