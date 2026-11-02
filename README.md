# Voxel Engine

A Rust + `wgpu` voxel engine built around **1 m blocks / 16³ meshlets**, a
**128³** chunk edge (the M1 gate's *Minimum* profile), and streaming terrain.
The current slice renders a 256-chunk ring at 1080p, and the world generator
produces a finite 1,024 km square with 18 biomes, rivers, lakes, caves, ores,
landmarks, structures and flora.

## Status

| Milestone | State |
|---|---|
| M0, M1, M1.5, M2 | Complete |
| M3 | Complete — `m3` tagged; two conditions carried forward |
| M2.5 | In progress — landed through `M2.5-21`; surface detail (`M2.5-22`) is next |

The measured position, open conditions and per-milestone evidence are in
[`docs/status.md`](docs/status.md).

## Quick start

```sh
cargo run -p voxel-viewer      # the streaming viewer
cargo test --workspace         # tests (GPU cases skip without an adapter)
./scripts/health.sh            # in-repo gates plus optional external tools
```

Controls and scene options are in
[`crates/engine/viewer/README.md`](crates/engine/viewer/README.md). Texture and
material work uses the standalone editor in [`apps/texgen/README.md`](apps/texgen/README.md).

## Documentation

Every document lives next to the code it describes and is published as one
site by [`scripts/build-docs-site.py`](scripts/build-docs-site.py), curated by
[`docs/site.toml`](docs/site.toml).

| Start here | |
|---|---|
| [`docs/status.md`](docs/status.md) | Where the project is now |
| [`docs/README.md`](docs/README.md) | The documentation index |
| [`docs/voxel-engine-architecture-spec.md`](docs/voxel-engine-architecture-spec.md) | What the engine is (normative) |
| [`docs/voxel-engine-roadmap.md`](docs/voxel-engine-roadmap.md) | Why the work is ordered this way |
| [`docs/task-list.md`](docs/task-list.md) | Every task, its dependency and verification |
| [`docs/adr/`](docs/adr) | Architecture decision records |

Build the site locally:

```sh
pip install mkdocs-material
python3 scripts/build-docs-site.py
```

## Repository layout

| Path | Contents |
|---|---|
| `crates/engine/` | block, mesh, render, storage, viewer, texgraph, worldgen |
| `crates/` | core, bench, profiling, app support, vendored ECS |
| `apps/` | `launcher` and `texgen` desktop applications |
| `benchmarks/` | recorded runs, history, fixtures and lens definitions |
| `docs/` | specifications, ADRs, milestone reports and engineering notes |
| `scripts/` | health, bench, constants and documentation-site tooling |

## Conventions

- Task IDs (`M1-07`, `M3-02`) are stable; quote them in commits.
- A task is done only when its *Done when* condition is met **and** its
  *Verify* command passes on the reference machine.
- Gating benchmarks are recorded through `scripts/bench.sh`, never quoted from
  an ad-hoc run.
- Any change to a binding number is made in the spec and the pre-registration,
  never only in the task list.

Agent instructions live in [`AGENTS.md`](AGENTS.md).
