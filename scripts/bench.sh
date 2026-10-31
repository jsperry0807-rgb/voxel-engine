#!/usr/bin/env bash
# Run every benchmark lens that this machine can support.
#
#   scripts/bench.sh                     # all lenses, report + archive in benchmarks/
#   LABEL=my-change scripts/bench.sh     # name the archived run
#   RECORD=0 scripts/bench.sh            # local report only, do not archive
#   VOXEL_GPU_BENCH=1 scripts/bench.sh   # also wgpu upload benchmarks
#   OUT=target/my-bench scripts/bench.sh # custom output directory
#
# Missing tools (Valgrind, perf) are reported and skipped; they never fail the
# run. Criterion and the software suite are required.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
OUT="${OUT:-target/bench}"
mkdir -p "$OUT"

# The perf lens runs the binary directly, so resolve Cargo's target directory
# instead of assuming ./target.
TARGET_DIR="${CARGO_TARGET_DIR:-}"
if [[ -z "$TARGET_DIR" ]]; then
  TARGET_DIR=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
    | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
fi
TARGET_DIR="${TARGET_DIR:-target}"
[[ "$TARGET_DIR" = /* ]] || TARGET_DIR="$PWD/$TARGET_DIR"

echo "== software lenses: memory, CPU, latency =="
cargo run --release -p voxel-bench -- all --out "$OUT"

if [[ "${GATE:-0}" == "1" ]]; then
  echo "== gate: twelve-row report and 128³ projection =="
  mkdir -p "$OUT/gate"
  cargo run --release -p voxel-bench -- gate report --out "$OUT/gate" 2>&1 \
    | tee "$OUT/gate-report.txt"
  cargo run --release -p voxel-bench -- gate scaleup --json --out "$OUT/gate/scaleup.json" 2>&1 \
    | tee "$OUT/gate-scaleup.txt"
  cargo run --release -p voxel-bench -- gate scaleup >"$OUT/gate/scaleup.md" \
    2>>"$OUT/gate-scaleup.txt"
fi

echo "== Criterion: wall clock and throughput =="
cargo bench --workspace 2>&1 | tee "$OUT/criterion.txt"

if command -v valgrind >/dev/null 2>&1; then
  echo "== iai-callgrind: instructions, cache and branch counts =="
  cargo bench -p voxel-bench --features iai --bench iai 2>&1 | tee "$OUT/iai.txt"
else
  echo "valgrind not found: skipping iai-callgrind lens"
fi

cargo build --release -p voxel-bench

if command -v perf >/dev/null 2>&1; then
  echo "== perf stat: whole CPU-lens process =="
  if perf stat \
      -e task-clock,cycles,instructions,cache-references,cache-misses,branch-instructions,branch-misses,page-faults \
      "$TARGET_DIR/release/voxel-bench" cpu --out "$OUT/perf-run" \
      2> "$OUT/perf.txt"; then
    echo "perf counters -> $OUT/perf.txt"
  else
    echo "perf is not permitted here; see $OUT/perf.txt (try: sysctl kernel.perf_event_paranoid)"
  fi
else
  echo "perf not found: skipping hardware-counter lens"
fi

if [[ "${VOXEL_GPU_BENCH:-}" == "1" ]]; then
  echo "== wgpu upload =="
  VOXEL_GPU_BENCH=1 cargo bench -p voxel-bench --bench render 2>&1 | tee "$OUT/gpu.txt"
fi

if [[ "${RECORD:-1}" == "1" ]]; then
  echo "== archive run =="
  if [[ -z "${LABEL:-}" ]]; then
    commit="$(git rev-parse --short HEAD 2>/dev/null || true)"
    LABEL="auto-${commit:-local}"
  fi
  record_args=(--from "$OUT" --label "$LABEL")
  [[ -n "${NOTE:-}" ]] && record_args+=(--note "$NOTE")
  for raw in criterion.txt iai.txt perf.txt gpu.txt gate-report.txt gate-scaleup.txt; do
    [[ -f "$OUT/$raw" ]] && record_args+=(--attach "$OUT/$raw")
  done
  for raw in gate/gate.md gate/gate.json gate/scaleup.md gate/scaleup.json; do
    [[ -f "$OUT/$raw" ]] && record_args+=(--attach "$OUT/$raw")
  done
  cargo run --release -p voxel-bench -- record "${record_args[@]}"
  echo "history: benchmarks/HISTORY.md"
else
  echo "RECORD=0: kept the run in $OUT only (not archived)"
fi

echo
echo "report:  $OUT/report.md"
echo "json:    $OUT/report.json"
echo "raw:     $OUT/criterion.txt, $OUT/iai.txt, $OUT/perf.txt"
