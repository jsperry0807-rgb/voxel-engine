## What changed

<!-- One logical change. Say what it does, not just which files moved. -->

Task: <!-- M2.5-22, or "none" -->

## Verification

<!-- Paste the commands you ran, with their result. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `./scripts/health.sh`, or the rows in CONTRIBUTING.md that match this change

## Conventions

- [ ] No frozen fixture was edited to make a test pass (mesh goldens, worldgen
      goldens, macro-world hash).
- [ ] Generated texture output was not hand-edited; `voxel-texgen -- check` is
      clean.
- [ ] A changed binding number is updated in the spec and the pre-registration
      too, not only in the task list.
- [ ] Benchmark numbers, if any, come from `scripts/bench.sh`.
- [ ] I agree to the contribution terms in `CONTRIBUTING.md`.
