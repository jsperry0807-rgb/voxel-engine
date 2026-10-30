#!/usr/bin/env bash
# Run every benchmark lens that this machine can support.
#
#   scripts/bench.sh                     # all lenses, report + archive in benchmarks/
#   LABEL=my-change scripts/bench.sh     # name the archived run
#   RECORD=0 scripts/bench.sh            # local report only, do not archive
#   VOXEL_GPU_BENCH=1 scripts/bench.sh   # also wgpu upload benchmarks
#   OUT=target/my-bench scripts/bench.sh # custom output directory
#   FRAMES=target/frames.csv scripts/bench.sh
#                                        # attach a viewer frame trace to the report
#
# Gate knobs. The gate refuses to measure across chunk edges, so a 128³ build
# needs a fixture frozen at 7-bit chunks; the default (m1-representative) is 64³.
#
#   GATE=1 FEATURES=chunk128 \
#     GATE_MANIFEST=benchmarks/fixtures/m2-128/manifest.json \
#     GATE_PROFILE=minimum LABEL=m3-gate-128 scripts/bench.sh
#
# `GATE_PROFILE` selects the twelve-row verdict's profile (default, minimum or
# high); the projection is analytic, so all three profiles are recorded. That
# keeps a run from claiming one view in `gate.md` and a different one in the
# scale-up. `FEATURES` applies to the voxel-bench invocations (software lens and
# gate); Criterion, iai and perf always run at the default edge.
#
# Missing tools (Valgrind, perf) are reported and skipped; they never fail the
# run. Criterion and the software suite are required.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
OUT="${OUT:-target/bench}"
FRAME_WARMUP="${FRAME_WARMUP:-0}"
mkdir -p "$OUT"

FEATURES_ARGS=()
if [[ -n "${FEATURES:-}" ]]; then
  FEATURES_ARGS=(--features "$FEATURES")
fi
BENCH=(cargo run --release -p voxel-bench "${FEATURES_ARGS[@]+"${FEATURES_ARGS[@]}"}")

GATE_MANIFEST_ARGS=()
if [[ -n "${GATE_MANIFEST:-}" ]]; then
  GATE_MANIFEST_ARGS=(--manifest "$GATE_MANIFEST")
fi
GATE_PROFILE_ARGS=()
if [[ -n "${GATE_PROFILE:-}" ]]; then
  GATE_PROFILE_ARGS=(--profile "$GATE_PROFILE")
fi

if [[ "${GATE:-0}" == "1" && -z "${GATE_MANIFEST:-}" && "${FEATURES:-}" == *chunk128* ]]; then
  echo "error: a chunk128 build needs GATE_MANIFEST frozen at 7-bit chunks" >&2
  echo "       e.g. GATE_MANIFEST=benchmarks/fixtures/m2-128/manifest.json" >&2
  exit 2
fi

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
frame_args=()
if [[ -n "${FRAMES:-}" ]]; then
  echo "   frame trace: $FRAMES (warm-up $FRAME_WARMUP frames)"
  frame_args+=(--frames "$FRAMES" --frame-warmup "$FRAME_WARMUP")
fi
"${BENCH[@]}" -- all --out "$OUT" "${frame_args[@]}"

if [[ "${GATE:-0}" == "1" ]]; then
  echo "== gate: twelve-row report and per-profile 128³ projection =="
  mkdir -p "$OUT/gate"
  "${BENCH[@]}" -- gate report \
    "${GATE_MANIFEST_ARGS[@]+"${GATE_MANIFEST_ARGS[@]}"}" \
    "${GATE_PROFILE_ARGS[@]+"${GATE_PROFILE_ARGS[@]}"}" \
    --out "$OUT/gate" 2>&1 | tee "$OUT/gate-report.txt"
  # The projection is analytic, so every profile is cheap. Record all three:
  # the per-chunk term is shared, the loaded chunk count is not.
  for profile in minimum default high; do
    "${BENCH[@]}" -- gate scaleup \
      "${GATE_MANIFEST_ARGS[@]+"${GATE_MANIFEST_ARGS[@]}"}" \
      --profile "$profile" --json --out "$OUT/gate/scaleup-$profile.json" 2>&1 \
      | tee -a "$OUT/gate-scaleup.txt"
    "${BENCH[@]}" -- gate scaleup \
      "${GATE_MANIFEST_ARGS[@]+"${GATE_MANIFEST_ARGS[@]}"}" \
      --profile "$profile" >"$OUT/gate/scaleup-$profile.md" \
      2>>"$OUT/gate-scaleup.txt"
  done
fi

echo "== Criterion: wall clock and throughput =="
cargo bench --workspace 2>&1 | tee "$OUT/criterion.txt"

if command -v valgrind >/dev/null 2>&1; then
  echo "== iai-callgrind: instructions, cache and branch counts =="
  cargo bench -p voxel-bench --features iai --bench iai 2>&1 | tee "$OUT/iai.txt"
else
  echo "valgrind not found: skipping iai-callgrind lens"
fi

cargo build --release -p voxel-bench "${FEATURES_ARGS[@]+"${FEATURES_ARGS[@]}"}"

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
  for raw in gate/gate.md gate/gate.json; do
    [[ -f "$OUT/$raw" ]] && record_args+=(--attach "$OUT/$raw")
  done
  for raw in "$OUT"/gate/scaleup-*.md "$OUT"/gate/scaleup-*.json; do
    [[ -f "$raw" ]] && record_args+=(--attach "$raw")
  done
  [[ -n "${FRAMES:-}" && -f "$FRAMES" ]] && record_args+=(--attach "$FRAMES")
  cargo run --release -p voxel-bench -- record "${record_args[@]}"
  echo "history: benchmarks/HISTORY.md"
else
  echo "RECORD=0: kept the run in $OUT only (not archived)"
fi

echo
echo "report:  $OUT/report.md"
echo "json:    $OUT/report.json"
echo "raw:     $OUT/criterion.txt, $OUT/iai.txt, $OUT/perf.txt"
if [[ -n "${FRAMES:-}" ]]; then
  echo "frame:   $FRAMES (section in $OUT/report.md)"
fi
