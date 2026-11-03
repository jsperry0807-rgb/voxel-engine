# WSL2 and low-memory hosts

The reference machine is not a laptop. A 4 GiB WSL2 guest runs the same suite,
but it needs capped parallelism: the defaults assume CI-sized memory and can
push the guest into swap hard enough to look like a crash.

Measured on Ubuntu on WSL2, 4 GiB RAM, 2 GiB swap, 4 CPUs, Quadro P1000 4 GB:

| Command | Wall time | Peak RSS |
|---|---|---|
| whole workspace, 2 build jobs, 2 test threads | ~9 min | 1.3 GiB |
| workspace without `voxel-bench` | ~3 min | 1.3 GiB |
| `voxel-bench` gate suite alone | ~6 min | 0.9 GiB |
| GPU round-trips | pass | — |

All tests pass on that host, including the GPU suites.

## Use the capped runner

```sh
./scripts/test-local.sh              # whole workspace, safe parallelism
./scripts/test-local.sh --fast       # skip the benchmark/gate suite
./scripts/test-local.sh --no-gpu     # skip GPU round-trips
./scripts/test-local.sh --bench      # only the benchmark/gate suite
./scripts/test-local.sh --clean      # drop target/debug/incremental first
```

The script picks 2 build jobs and 2 test threads at 8 GiB or less and otherwise
uses every core. Override with `TEST_JOBS` / `TEST_THREADS`. Any other cargo
command can use the same cap:

```sh
CARGO_BUILD_JOBS=2 cargo build --workspace
```

## Why the default can hang the guest

`cargo test --workspace` builds with one `rustc` per core. At four cores the
winit/wgpu dependency graph peaks well above the 3.8 GiB the guest reports, and
the 2 GiB swap is not enough to absorb it. The symptom is a frozen terminal or
a VM reset, not a Rust error. Cap the jobs first; only then look for a code
problem.

## GPU tests do not skip on WSL

WSL2 exposes the host GPU through `/dev/dxg`, so `wgpu` finds an adapter and the
GPU tests run instead of skipping. On this machine they pass through the
Dozen/D3D12 path. If a driver update makes them hang or reset the GPU, opt out:

```sh
VOXEL_SKIP_GPU=1 cargo test --workspace
```

`VOXEL_SKIP_GPU` is honoured by the viewer and render GPU tests and reports a
skip exactly like a host with no adapter. It is incompatible with
`VOXEL_REQUIRE_GPU=1`, which CI sets to force a real adapter; do not set both.

## Internal compiler error after a killed build

If a build is interrupted, `rustc` can fail later with an internal compiler
error while saving its incremental graph (`save_dep_graph`, empty query stack).
The incremental cache is corrupt, not the source. Drop only the cache:

```sh
rm -rf target/debug/incremental
```

`./scripts/test-local.sh --clean` does this. Builds with
`CARGO_INCREMENTAL=0` (as CI does) are immune but slower to repeat.

## What not to run on 4 GiB

`scripts/health.sh` with external tools enabled, `cargo mutants`,
`cargo llvm-cov`, `cargo bench --workspace`, and any `--features chunk128`
build are all much larger than the test suite. Run them on a bigger machine, or
one at a time with `HEALTH_BENCH=0 HEALTH_MUTANTS=0 HEALTH_COVERAGE=0`.
