#!/usr/bin/env bash
# Run the test suite with parallelism sized for a small or WSL2 machine.
#
# CI runs `cargo test --workspace` with unlimited parallelism. On a 4 GiB WSL2
# guest that means four `rustc` jobs plus the gate suite, which can push the VM
# into swap and freeze it. This script caps build and test parallelism to what
# a low-memory machine can actually hold, and lets you skip the heaviest parts.
#
# Measured on a 4 GiB / 4 CPU WSL2 guest with a 4 GiB Quadro P1000:
#
#   whole workspace, 2 jobs, 2 test threads   ~9 min, ~1.3 GiB peak RSS
#   workspace without voxel-bench             ~3 min, ~1.3 GiB peak RSS
#   voxel-bench gate suite alone              ~6 min, ~0.9 GiB peak RSS
#   GPU round-trips through WSL/Dozen         pass
#
# Usage:
#   scripts/test-local.sh              # whole workspace, safe parallelism
#   scripts/test-local.sh --fast       # skip the benchmark/gate suite
#   scripts/test-local.sh --no-gpu     # skip GPU round-trips (VOXEL_SKIP_GPU=1)
#   scripts/test-local.sh --bench      # only the benchmark/gate suite
#   scripts/test-local.sh --clean      # drop the incremental cache first
#   scripts/test-local.sh -- --nocapture
#
# Environment:
#   TEST_JOBS      rustc jobs (default: 2 at <= 8 GiB RAM, else nproc)
#   TEST_THREADS   libtest threads (default: 2 at <= 8 GiB RAM, else nproc)
#
# If a previous build was killed, rustc can fail with an internal compiler
# error while saving its incremental graph. `--clean` removes only
# `target/debug/incremental`, which is a cache and never source.
set -euo pipefail

if ! cd "$(dirname "${BASH_SOURCE[0]}")/.."; then
  printf 'test-local: cannot enter the repository root\n' >&2
  exit 2
fi

fast=0
bench_only=0
no_gpu=0
clean=0
libtest=()

while (($#)); do
  case $1 in
    -h | --help)
      sed -n '2,30p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    --fast) fast=1 ;;
    --bench) bench_only=1 ;;
    --no-gpu) no_gpu=1 ;;
    --clean) clean=1 ;;
    --)
      shift
      libtest=("$@")
      break
      ;;
    *)
      printf 'test-local: unknown argument `%s` (try --help)\n' "$1" >&2
      exit 2
      ;;
  esac
  shift
done

if ((fast && bench_only)); then
  printf 'test-local: --fast and --bench are mutually exclusive\n' >&2
  exit 2
fi

mem_kib=0
if [[ -r /proc/meminfo ]]; then
  mem_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
fi
mem_gib=$((mem_kib / 1024 / 1024))

default_jobs=$(nproc)
default_threads=$(nproc)
if ((mem_gib > 0 && mem_gib <= 8)); then
  default_jobs=2
  default_threads=2
fi

jobs=${TEST_JOBS:-$default_jobs}
threads=${TEST_THREADS:-$default_threads}

if ((clean)); then
  printf 'test-local: removing target/debug/incremental\n'
  rm -rf target/debug/incremental
fi

args=(test)
if ((bench_only)); then
  args+=(-p voxel-bench)
else
  args+=(--workspace)
  if ((fast)); then
    args+=(--exclude voxel-bench)
  fi
fi

if ((no_gpu)); then
  export VOXEL_SKIP_GPU=1
fi

export CARGO_BUILD_JOBS="$jobs"
export RUST_TEST_THREADS="$threads"

printf 'test-local: %s\n' "$(uname -sr)"
printf 'test-local: %s GiB RAM, %s CPUs -> %s build jobs, %s test threads\n' \
  "${mem_gib:-?}" "$(nproc)" "$jobs" "$threads"
if ((no_gpu)); then
  printf 'test-local: GPU round-trips skipped (VOXEL_SKIP_GPU=1)\n'
fi
printf 'test-local: cargo %s\n' "${args[*]}"

if ((${#libtest[@]})); then
  cargo "${args[@]}" -- "${libtest[@]}"
else
  cargo "${args[@]}"
fi
