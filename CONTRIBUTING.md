# Contributing

This repository is a Rust + `wgpu` voxel engine built around a measured
milestone plan: every task has a *Done when* condition and a *Verify* command,
and the documents in [`docs/specs/`](docs/specs) decide what "correct" means
before the code does. Contributions are welcome as long as they follow that
plan rather than working around it.

Participation in this project is covered by the
[Code of Conduct](CODE_OF_CONDUCT.md), and every contribution is accepted
under the [contribution terms](#licensing) below.

## Before you start

- **Bug reports and questions** belong in
  [issues](https://github.com/jsperry0807-rgb/voxel-engine/issues). Include the
  seed, the chunk edge (`64³` or `128³`), the feature flags, and the exact
  command you ran. If a benchmark is involved, attach the recorded run.
- **Security issues are not issues.** Report them privately through the Security
  tab, as described in [the security policy](SECURITY.md); never in a public
  issue, discussion or pull request.
- **Open an issue before large changes.** Anything that touches a binding
  number, a frozen fixture or a specification needs agreement first, because
  the change is a plan change, not just a code change.
- **The normative order is spec, then roadmap, then task list.** When they
  disagree, the spec wins. Quote the task ID (`M2.5-22`, `M3-02`) in your
  branch, commits and pull request.

## Set up

Rust stable is all that is required; CI tracks stable and there is no pinned
toolchain file. A GPU is optional — tests that need one skip themselves when no
adapter is present.

```sh
cargo run -p voxel-viewer      # the streaming viewer
cargo test --workspace         # tests
./scripts/health.sh            # in-repo gates plus optional external tools
```

`./scripts/health.sh` runs the same checks as CI and adds coverage, mutation
testing, `cargo deny`, `cargo audit`, `cargo udeps` and profiling when those
tools are installed. `HEALTH_EXTERNAL=0 ./scripts/health.sh` keeps it to the
in-repo checks; `HEALTH_QUIET=1` prints only the summary.

Windows builds are cross-checked from Linux with `cargo build-win`; run
`cargo setup-win` once to install the `mingw-w64` preflight it needs.

## What CI runs

CI is area-aware — `scripts/changed-areas.sh` classifies the diff, so a
docs-only change does not start the Rust jobs. The full set:

| Job | Command |
|---|---|
| fmt | `cargo fmt --all --check` |
| constants | `./scripts/check-constants.sh` (chunk geometry may not be a literal outside `constants.rs`) |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` and the same for `-p voxel-mesh --features raytest` |
| test | `cargo test --workspace` and `cargo test -p voxel-mesh --features raytest` |
| chunk128 | `cargo test --workspace --features chunk128` |
| goldens | mesh and worldgen golden tests, the pinned macro-world hash, checksummed fixtures |
| textures | `cargo run -p voxel-texgen -- check` and the manifest / generated-material tests |
| shaders | `cargo test -p voxel-render --test shaders` (naga validation, no GPU) |
| viewer validation | offscreen chunk draw under Lavapipe with `VOXEL_REQUIRE_GPU=1` |
| windows cross-build | `cargo build-win` |

Run the rows that match your change locally before opening a pull request.

## Pull requests

- Keep one logical change per pull request, and say which commands you ran.
  "Verified with `cargo test --workspace` and `./scripts/health.sh`" is the
  expected level of detail.
- `fmt` and `clippy -D warnings` are merge blockers. Fix the lint rather than
  adding an allow, unless the allow explains itself in a comment.
- **Never edit a frozen fixture to make a test pass.** The mesh goldens, the
  worldgen goldens and the macro-world hash are contracts; changing one is a
  specification change and needs the matching task and report update.
- **Never hand-edit generated texture output.** Edit the node graph in
  `crates/engine/block/textures/graph/*.ron` (through
  `cargo run -p voxel-texgen`), then regenerate with
  `cargo run -p voxel-texgen -- update-all`; `cargo run -p voxel-texgen -- check`
  must pass with a zero-byte diff. The committed source PNGs, `textures.ron`,
  `materials.gen.ron`, the atlas and `atlas.hash.ron` come from that pipeline
  only.
- **Benchmark numbers in a document must come from `scripts/bench.sh`** with a
  recorded run under `benchmarks/`. Ad-hoc timings are not evidence.
- Changing a binding number means changing the spec and the pre-registration
  too, never only the task list or the code.

Commit messages follow the history's pattern: `type(scope): summary`, for
example `feat(textures): 16/32/64/128/256 texel density at runtime`,
`docs: restructure documentation`, `build(deps): bump ...`.

## Documentation

Every Markdown file in the repository is published to the documentation site.
A document lives next to the code it describes, and the
[site taxonomy](docs/site.toml) decides where it appears in the navigation;
an unlisted document still shows up under "Other", so nothing disappears
silently. Add a `pages` entry when a new document should be part of the reading
order.

Build the site locally:

```sh
pip install mkdocs-material
python3 scripts/build-docs-site.py
```

## Licensing

This project is source-available under the
[PolyForm Noncommercial License 1.0.0](LICENSE), reproduced verbatim. In plain
terms:

- **Noncommercial use is free.** Read it, run it, change it, and share your
  changes for any noncommercial purpose: personal projects, study, hobby work,
  research, and noncommercial organisations such as schools, charities and
  public bodies.
- **Publishing is allowed while it stays noncommercial.** If you redistribute
  the Software or a fork of it, pass on the license text and the
  `Required Notice:` line, and keep the use noncommercial.
- **Commercial use needs a license from the copyright holder** — a paid game, a
  product, a paid service, or work you are paid to do for a company. Ask first;
  the terms are not granted by default.
- **Vendored third-party code keeps its own license.** `crates/ECS` is
  `MIT OR Apache-2.0` according to its own `Cargo.toml`.

By opening a pull request you agree to these contribution terms:

- You keep copyright in your contribution.
- You grant Mr. Sperry a perpetual, worldwide, non-exclusive, royalty-free,
  irrevocable license to use, reproduce, modify, adapt, publish, distribute,
  sublicense and relicense your contribution, in source or binary form, under
  any license, including a proprietary one.
- You confirm the contribution is your own work, or that you have the right to
  submit it and to grant those rights. Say so in the pull request if any part
  came from elsewhere — copied code under incompatible terms cannot be merged.
- Contributing grants you no rights in the project beyond those in `LICENSE`,
  and no ownership or control over the direction of the project.

If you cannot agree to those terms, open an issue describing the change
instead of a pull request; the change can still be made by the maintainer.

## Reporting conduct issues

Read the [Code of Conduct](CODE_OF_CONDUCT.md) first. Reports go privately
through this repository's **Security** tab
([Report a vulnerability](https://github.com/jsperry0807-rgb/voxel-engine/security/advisories/new)),
never through a public issue.
