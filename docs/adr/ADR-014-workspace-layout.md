# ADR-014: Workspace Layout — Libraries in `crates/`, Apps in `apps/`

**Status:** Accepted — 2026-11-03
**Decision owner:** engine
**Supersedes:** none — records the layout the README and `docs/site.toml` assume
**Related:** ADR-013 state ownership (the CPU ECS this layout promotes);
`xtask/src/layers.rs`; `docs/notes/wsl-and-low-memory.md`

## Context

The workspace grew until the directory tree stopped describing the dependency
graph. `crates/engine/` mixed a leaf generator (`worldgen`), a content crate
(`block`), the forward render library (`render`) and an application
(`viewer`, 18k lines with a `main.rs`). `crates/bench` was a second
application. `crates/ECS` was a vendored `my_ecs` snapshot with its own
lockfile, excluded from the workspace and from sonar, referenced by nothing.

Nothing enforced the shape. A crate could import any other crate, and a new
crate could land anywhere, so the only description of the architecture was
prose in the README.

## Decision

**One rule for placement, one table for dependencies, one command that checks
both.**

1. **`crates/engine/` holds libraries only.** `voxel-viewer` and `voxel-bench`
   move to `apps/`; both keep their lib targets because the bench drives viewer
   internals. Package names do not change, so `cargo run -p voxel-viewer`,
   `-p voxel-bench`, `-p voxel-texgen` and the launcher's spawn commands keep
   working verbatim.
2. **Directory name is the package name without `voxel-`.** `voxel-storage`
   lives in `crates/engine/storage`. `xtask` is the only exception, being the
   package and the directory.
3. **The layering table is source of truth.** `xtask/src/layers.rs` lists the
   allowed production dependency of every crate; `cargo xtask check-layers`
   fails when an edge is not in the table, a crate is missing from it, or a
   directory name drifts. Dev-dependencies are exempt: a test may build a
   fixture from any crate without that becoming a production edge.
4. **The CPU ECS is a workspace member.** `crates/ecs` / `voxel-ecs` keeps its
   frozen 1.0.0 version and contract, drops the nested lockfile and nested CI,
   and joins the workspace lints, sonar analysis and docs taxonomy. It is not
   wired into the viewer; that is M4 work under ADR-013.
5. **Build-only code leaves the runtime foundation.** The mingw-w64/rustup
   probe moves from `voxel-core` to `crates/build-support`, so `xtask` and the
   launcher share it without core carrying a `std::process` tool check.

Tool binaries stay inside their crates for now (`texgen-mcp` in
`voxel-texgraph`, the worldgen CLI in `voxel-worldgen`): both are documented
entry points (`opencode.jsonc`, the texgen README, the worldgen docs), and
moving them would trade a documented command for a cosmetic boundary.

## Consequences

- Adding a crate is a two-line change: a `members` entry and a `layers.rs`
  row. Forgetting the row fails CI rather than silently widening the graph.
- CI and `scripts/health.sh` run `check-layers`, so the layout cannot drift
  back without an explicit, reviewable edit to the table.
- Package names stayed stable, so this is a pure move for users; only paths
  inside the repository changed.
- A crate-local `clippy.toml` is not read by `cargo clippy --workspace`:
  Clippy resolves the config from the working directory. The ECS therefore has
  its own CI step that runs from `crates/ecs`. Any future crate-local Clippy
  config needs the same treatment.

## Alternatives rejected

- **Flat `crates/` with no `engine/` grouping.** Most predictable, but churns
  every published path and deletes the only hint that these crates form an
  engine.
- **Re-layering directories as `foundation/`, `content/`, `world/`, `render/`.**
  Encodes layer order in the tree, but the taxonomy must be maintained forever
  and the compiler does not check it. An explicit table that CI enforces is
  cheaper and stricter.
- **Deleting `voxel-render` as duplicate of the viewer's renderer.** Rejected:
  the M4/M5 tasks route GPU traversal and indirect draw through
  `voxel-render`. The two render paths are a transition, not an accident.
