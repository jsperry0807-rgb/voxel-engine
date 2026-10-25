#!/usr/bin/env bash
# Run every benchmark lens that this machine can support.
#
#   scripts/bench.sh                     # software lenses + Criterion + iai + perf
#   RECORD=1 LABEL=baseline scripts/bench.sh   # also archive under benchmarks/
#   VOXEL_GPU_BENCH=1 scripts/bench.sh   # also wgpu upload benchmarks
#   OUT=target/my-bench scripts/bench.sh # custom output directory
#
# Missing tools (Valgrind, perf) are reported and skipped; they never fail the
# run. Criterion and the software suite are required.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
OUT="${OUT:-target/bench}"
mkdir -p "$OUT"

echo "== software lenses: memory, CPU, latency =="
cargo run --release -p voxel-bench -- all --out "$OUT"

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
      ./target/release/voxel-bench cpu --out "$OUT/perf-run" \
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

if [[ "${RECORD:-0}" == "1" ]]; then
  echo "== archive run =="
  record_args=(--from "$OUT" --label "${LABEL:-baseline}")
  [[ -n "${NOTE:-}" ]] && record_args+=(--note "$NOTE")
  for raw in criterion.txt iai.txt perf.txt gpu.txt; do
    [[ -f "$OUT/$raw" ]] && record_args+=(--attach "$OUT/$raw")
  done
  cargo run --release -p voxel-bench -- record "${record_args[@]}"
else
  echo "not archived: set RECORD=1 LABEL=<name> to add this run to benchmarks/"
fi

echo
echo "report:  $OUT/report.md"
echo "json:    $OUT/report.json"
echo "raw:     $OUT/criterion.txt, $OUT/iai.txt, $OUT/perf.txt"
