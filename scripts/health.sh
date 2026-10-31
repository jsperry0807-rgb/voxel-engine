#!/usr/bin/env bash
# Full project health check for the voxel engine.
#
# The script runs the in-repo gates first, then the optional external analysis
# tools, and finishes with a one-screen PASS/FAIL/WARN/SKIP summary. Every
# command's raw output is kept under the cargo target dir's health/ (or HEALTH_LOG_DIR).
#
# Usage:
#   scripts/health.sh                       # everything
#   HEALTH_QUIET=1 scripts/health.sh        # summary only
#   HEALTH_EXTERNAL=0 scripts/health.sh     # in-repo checks only
#   HEALTH_BENCH=0 HEALTH_MUTANTS=0 ...     # skip selected long checks
#
# Environment:
#   HEALTH_LOG_DIR       output directory (default: <target>/health)
#   HEALTH_QUIET         "1" prints only the final summary
#   HEALTH_INREPO        "0" skips the in-repo checks (external tools only)
#   HEALTH_STRICT        "1" makes WARN count as a failed result
#   HEALTH_STRICT_GATES  "1" makes a gate report verdict of FAIL/INCOMPLETE fail
#   HEALTH_GATES         "0" skips the two gate reports
#   HEALTH_SCALEUP       "0" skips the two scale-up profiles
#   HEALTH_BENCH         "0" skips LABEL=local scripts/bench.sh
#   HEALTH_VIEWER        "0" skips the viewer smoke run
#   HEALTH_VIEWER_SECS   seconds the viewer must stay alive (default: 8)
#   HEALTH_VIEWER_LOG    RUST_LOG for the smoke run (default: wgpu_core=warn)
#   HEALTH_EXTERNAL      "0" skips coverage, mutants, deny, audit, udeps,
#                         bloat, flamegraph/samply and tokei
#   HEALTH_COVERAGE      "0" skips cargo llvm-cov
#   HEALTH_MUTANTS       "0" skips cargo mutants
#   HEALTH_MUTANTS_ARGS  extra cargo-mutants args (default: --workspace); use
#                         a shard, e.g. "--workspace --shard 1/100"
#   HEALTH_PROFILE       "0" skips flamegraph/samply
#   HEALTH_DENY_CONFIG   optional cargo-deny config path; without it cargo
#                         deny falls back to its built-in default policy
#
# The bench step is the literal `LABEL=local scripts/bench.sh`, so it archives
# the run under benchmarks/ as that script normally does (RECORD=0 keeps it in
# target/bench). Gate failures are reported as WARN because CI marks the M1
# gate advisory; set HEALTH_STRICT_GATES=1 to make them FAIL here.
#
# Exit status: 0 when no FAIL was recorded. WARN and SKIP are advisory unless
# HEALTH_STRICT=1. External tools that are not installed are SKIP, not FAIL.
set -uo pipefail

if ! cd "$(dirname "${BASH_SOURCE[0]}")/.."; then
  printf 'health: cannot enter the repository root\n' >&2
  exit 2
fi
ROOT="$PWD"

# Cargo's target directory, honouring CARGO_TARGET_DIR. Without this a build
# with a shared target dir succeeds and the viewer/samply checks then look in
# the wrong place.
target_dir() {
  if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    if [[ "$CARGO_TARGET_DIR" = /* ]]; then
      printf '%s\n' "$CARGO_TARGET_DIR"
    else
      printf '%s\n' "$ROOT/$CARGO_TARGET_DIR"
    fi
    return
  fi
  local meta
  meta=$(mktemp "${TMPDIR:-/tmp}/voxel-health-metadata.XXXXXX") || meta=""
  if [[ -n "$meta" ]] && cargo metadata --no-deps --format-version 1 >"$meta" 2>/dev/null; then
    local dir
    dir=$(json_query "$meta" '.target_directory')
    rm -f "$meta"
    if [[ -n "$dir" ]]; then
      printf '%s\n' "$dir"
      return
    fi
  else
    [[ -n "$meta" ]] && rm -f "$meta"
  fi
  printf '%s\n' "$ROOT/target"
}

START_SECONDS=$SECONDS

# ---- presentation ---------------------------------------------------------

if [[ -t 1 && -z "${NO_COLOR:-}" && "${TERM:-}" != "dumb" ]]; then
  C_PASS=$'\033[32m'
  C_FAIL=$'\033[31m'
  C_WARN=$'\033[33m'
  C_SKIP=$'\033[36m'
  C_INFO=$'\033[2m'
  C_BOLD=$'\033[1m'
  C_RESET=$'\033[0m'
else
  C_PASS=''
  C_FAIL=''
  C_WARN=''
  C_SKIP=''
  C_INFO=''
  C_BOLD=''
  C_RESET=''
fi

declare -a S_GROUP=()
declare -a S_NAME=()
declare -a S_STATUS=()
declare -a S_DETAIL=()
declare -a S_LOG=()
LAST_INDEX=-1
LAST_LOG=""

declare -a ATTENTION=()
declare -a GATE_ATTENTION=()
declare -a OPEN_EXIT=()
FRAME_GAP=0
GPU_GAP=0
VIEWER_SKIP_REASON=""

TASK_TOTAL=0
TASK_DONE=0
TASK_OPEN=0
TASK_BAD=0

have() { command -v "$1" >/dev/null 2>&1; }

slug() {
  printf '%s' "$*" \
    | tr '[:upper:]' '[:lower:]' \
    | tr ' ' '-' \
    | tr -cd 'a-z0-9._-'
}

human_secs() {
  local secs="${1:-0}"
  if (( secs >= 60 )); then
    printf '%dm%02ds' $((secs / 60)) $((secs % 60))
  else
    printf '%ds' "$secs"
  fi
}

fit() {
  local text="${1:-}"
  local max="${2:-115}"
  if (( ${#text} > max )); then
    printf '%s…' "${text:0:max-1}"
  else
    printf '%s' "$text"
  fi
}

begin_check() {
  [[ "${HEALTH_QUIET:-0}" == "1" ]] || printf '  %-34s ' "$1"
}

add_result() {
  local group="$1" name="$2" status="$3" detail="${4:-}" log="${5:-}"
  S_GROUP+=("$group")
  S_NAME+=("$name")
  S_STATUS+=("$status")
  S_DETAIL+=("$detail")
  S_LOG+=("$log")
  LAST_INDEX=$(( ${#S_NAME[@]} - 1 ))
  LAST_LOG="$log"
}

end_check() {
  local group="$1" name="$2" status="$3" detail="${4:-}" log="${5:-}" secs="${6:-0}"
  local color="$C_INFO"
  case "$status" in
    PASS) color="$C_PASS" ;;
    FAIL) color="$C_FAIL" ;;
    WARN) color="$C_WARN" ;;
    SKIP) color="$C_SKIP" ;;
    INFO) color="$C_INFO" ;;
  esac

  detail="${detail//$'\n'/; }"
  detail="${detail//$'\r'/}"
  if [[ "$status" == "FAIL" && -n "$log" ]]; then
    detail="${detail:+$detail; }log ${log#"$ROOT"/}"
  fi
  local timing=""
  (( secs > 0 )) && timing=" ($(human_secs "$secs"))"

  if [[ "${HEALTH_QUIET:-0}" != "1" ]]; then
    printf '%s%-7s%s %s%s\n' "$color" "$status" "$C_RESET" "$(fit "$detail")" "$timing"
  fi
  add_result "$group" "$name" "$status" "$detail" "$log"
}

set_last_status() {
  (( LAST_INDEX >= 0 )) && S_STATUS[LAST_INDEX]="$1"
}

set_last_detail() {
  (( LAST_INDEX >= 0 )) && S_DETAIL[LAST_INDEX]="$1"
}

run_check() {
  local group="$1" name="$2"
  shift 2
  local log="$LOG_DIR/$(slug "$name").log"
  local t0=$SECONDS
  begin_check "$name"
  "$@" >"$log" 2>&1
  local rc=$?
  local secs=$((SECONDS - t0))
  if (( rc == 0 )); then
    end_check "$group" "$name" PASS "" "$log" "$secs"
  else
    end_check "$group" "$name" FAIL "exit $rc" "$log" "$secs"
  fi
  return "$rc"
}

skip_check() {
  local group="$1" name="$2" reason="$3"
  begin_check "$name"
  end_check "$group" "$name" SKIP "$reason" "" 0
}

# ---- small JSON helpers (jq, then python3, then sed for summary fields) ----

json_query() {
  local file="$1" expr="$2"
  if have jq; then
    jq -r "$expr // empty" "$file" 2>/dev/null
    return
  fi
  if have python3; then
    python3 - "$file" "$expr" <<'PY' 2>/dev/null
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
for part in sys.argv[2].lstrip(".").split("."):
    if not part:
        continue
    value = value[part]
print(value)
PY
    return
  fi

  # Last-resort fallback for the three summary fields; rows are omitted.
  case "$expr" in
    .summary.status)
      sed -n '/"summary"[[:space:]]*:/,/^[[:space:]]*}/p' "$file" \
        | sed -n 's/.*"status"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1
      ;;
    .summary.pass|.summary.fail|.summary.not_measured)
      local key="${expr##*.}"
      sed -n '/"summary"[[:space:]]*:/,/^[[:space:]]*}/p' "$file" \
        | sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p" \
        | head -n 1
      ;;
  esac
}

# Resolved after `json_query` exists; `target_dir` falls back to $ROOT/target
# when cargo metadata or a JSON tool is unavailable.
TARGET_DIR="$(target_dir)"
LOG_DIR="${HEALTH_LOG_DIR:-$TARGET_DIR/health}"
mkdir -p "$LOG_DIR"

gate_rows() {
  local file="$1"
  if have jq; then
    jq -r '.rows[] | [.id,.verdict,.measured] | @tsv' "$file" 2>/dev/null
  elif have python3; then
    python3 - "$file" <<'PY' 2>/dev/null
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)
for row in report.get("rows", []):
    print("\t".join(str(row.get(key, "")) for key in ("id", "verdict", "measured")))
PY
  fi
}

# ---- in-repo checks -------------------------------------------------------

check_tasks() {
  local name="task-list counts"
  local log="$LOG_DIR/task-list.log"
  begin_check "$name"

  if [[ ! -f docs/task-list.md ]]; then
    end_check "in-repo" "$name" FAIL "docs/task-list.md not found" "" 0
    return
  fi

  awk -F'|' '
    /^\| *M[0-9]+(\.[0-9]+)?-[0-9]+ *\|/ {
      id = $2
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", id)
      task = $3
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", task)
      done = $8
      gsub(/[[:space:]]/, "", done)
      if (id in seen) next
      seen[id] = 1
      total++
      split(id, parts, "-")
      milestone = parts[1]
      ms_total[milestone]++
      if (done == "[x]") {
        done_count++
        ms_done[milestone]++
      } else if (done == "[]") {
        open_count++
        ms_open[milestone]++
        if (milestone == "M0" || milestone == "M1" || milestone == "M1.5") {
          printf "OPEN\t%s\t%s\n", id, task
        }
      } else {
        bad++
      }
    }
    END {
      printf "TOTAL\t%d\t%d\t%d\t%d\n", total, done_count, open_count, bad
      for (milestone in ms_total) {
        printf "MS\t%s\t%d\t%d\t%d\n", milestone, ms_done[milestone] + 0, ms_total[milestone], ms_open[milestone] + 0
      }
    }
  ' docs/task-list.md >"$log"

  TASK_TOTAL=0
  TASK_DONE=0
  TASK_OPEN=0
  TASK_BAD=0
  OPEN_EXIT=()

  local kind a b c d
  while IFS=$'\t' read -r kind a b c d; do
    case "$kind" in
      TOTAL)
        TASK_TOTAL="$a"
        TASK_DONE="$b"
        TASK_OPEN="$c"
        TASK_BAD="$d"
        ;;
      OPEN)
        OPEN_EXIT+=("$a — $b")
        ;;
    esac
  done <"$log"

  local milestone15
  milestone15=$(awk -F'\t' '$1 == "MS" && $2 == "M1.5" { print $3 "/" $4 }' "$log")

  if (( TASK_TOTAL > 0 && TASK_BAD == 0 && TASK_DONE + TASK_OPEN == TASK_TOTAL )); then
    local detail="$TASK_DONE/$TASK_TOTAL done, $TASK_OPEN open"
    [[ -n "$milestone15" ]] && detail="$detail; M1.5 $milestone15"
    end_check "in-repo" "$name" PASS "$detail" "$log" 0
  else
    end_check "in-repo" "$name" FAIL "could not reconcile the task rows" "$log" 0
  fi
}

check_frame_profiling() {
  local name="frame-time profiling"
  local log="$LOG_DIR/frame-time-profiling.log"
  begin_check "$name"
  {
    echo "Searching the bench suite for frame-time, 1% low, hitch or FPS evidence."
    grep -RniE 'frame.?time|1% ?low|frametime|hitch|\bfps\b' \
      crates/bench/src crates/bench/benches --include='*.rs' 2>/dev/null || true
  } >"$log" 2>&1

  if grep -RqiE 'frame.?time|1% ?low|frametime|hitch|\bfps\b' \
      crates/bench/src crates/bench/benches --include='*.rs' 2>/dev/null; then
    end_check "in-repo" "$name" PASS "bench suite has a frame-time/hitch lens" "$log" 0
  else
    FRAME_GAP=1
    end_check "in-repo" "$name" WARN \
      "no frame-time/1%-low/hitch lens; viewer logs FPS at debug only" "$log" 0
  fi
}

check_gpu_validation() {
  local name="GPU validation (M1.5-16)"
  local log="$LOG_DIR/gpu-validation.log"
  begin_check "$name"
  {
    echo "Looking for a zero-validation viewer job in CI."
    grep -nE 'wgpu_core=warn|M1\.5-16|voxel-viewer' .github/workflows/ci.yml 2>/dev/null || true
  } >"$log" 2>&1

  if grep -qE 'wgpu_core=warn' .github/workflows/ci.yml 2>/dev/null \
    && grep -qE 'voxel-viewer|--test validation' .github/workflows/ci.yml 2>/dev/null; then
    end_check "in-repo" "$name" PASS "CI has a RUST_LOG=wgpu_core=warn viewer job" "$log" 0
  else
    GPU_GAP=1
    end_check "in-repo" "$name" WARN \
      "no zero-validation viewer job in CI; M1.5-16 remains open" "$log" 0
  fi
}

check_viewer_validation() {
  local name="GPU validation test"
  local test_file="crates/engine/viewer/tests/validation.rs"
  if [[ ! -f "$test_file" ]]; then
    skip_check "in-repo" "$name" "crates/engine/viewer/tests/validation.rs is not present"
    return
  fi

  local t0=$SECONDS
  begin_check "$name"
  local log="$LOG_DIR/gpu-validation-test.log"
  (
    cd "$ROOT" || exit 1
    env RUST_LOG="${HEALTH_VIEWER_LOG:-wgpu_core=warn}" VOXEL_REQUIRE_GPU=1 \
      cargo test -p voxel-viewer --test validation
  ) >"$log" 2>&1
  local rc=$?
  local secs=$((SECONDS - t0))

  if (( rc == 0 )); then
    end_check "in-repo" "$name" PASS "managed validation error scope stayed clean" "$log" "$secs"
  elif grep -Eqi 'no wgpu adapter|failed to find an appropriate adapter|no adapter found|VOXEL_REQUIRE_GPU is set but no wgpu adapter' "$log"; then
    end_check "in-repo" "$name" SKIP "no wgpu adapter available" "$log" "$secs"
  else
    end_check "in-repo" "$name" FAIL "exit $rc" "$log" "$secs"
  fi
}

check_gate_profile() {
  local profile="$1"
  local name="gate report ($profile)"
  local dir="$LOG_DIR/gate-$profile"
  local log="$LOG_DIR/gate-$profile.log"
  local t0=$SECONDS
  begin_check "$name"

  cargo run --release -p voxel-bench -- gate report --profile "$profile" --out "$dir" \
    >"$log" 2>&1
  local rc=$?
  local secs=$((SECONDS - t0))

  if (( rc != 0 )); then
    end_check "in-repo" "$name" FAIL "exit $rc" "$log" "$secs"
    return
  fi

  local json="$dir/gate.json"
  if [[ ! -f "$json" ]]; then
    end_check "in-repo" "$name" FAIL "gate.json missing" "$log" "$secs"
    return
  fi

  local status pass fail not_measured
  status=$(json_query "$json" '.summary.status')
  pass=$(json_query "$json" '.summary.pass')
  fail=$(json_query "$json" '.summary.fail')
  not_measured=$(json_query "$json" '.summary.not_measured')

  local check_status=PASS
  case "$status" in
    pass) check_status=PASS ;;
    fail|incomplete) check_status=WARN ;;
    *) check_status=WARN ;;
  esac
  if [[ "${HEALTH_STRICT_GATES:-0}" == "1" && "$status" != "pass" ]]; then
    check_status=FAIL
  fi

  end_check "in-repo" "$name" "$check_status" \
    "gate ${status:-unknown}: ${pass:-?} pass / ${fail:-?} fail / ${not_measured:-?} not measured" \
    "$json" "$secs"

  if [[ "$status" != "pass" ]]; then
    local failed_rows missing_rows
    failed_rows=$(gate_rows "$json" \
      | awk -F'\t' '$2 == "fail" { printf "%s%s", sep, $1; sep = ", " }')
    missing_rows=$(gate_rows "$json" \
      | awk -F'\t' '$2 == "not_measured" { printf "%s%s", sep, $1; sep = ", " }')
    local gate_line="gate ${profile}: ${status:-unknown} (${pass:-?}P/${fail:-?}F/${not_measured:-?}NM)"
    [[ -n "$failed_rows" ]] && gate_line="$gate_line; fail: $failed_rows"
    [[ -n "$missing_rows" ]] && gate_line="$gate_line; not measured: $missing_rows"
    GATE_ATTENTION+=("$gate_line")
  fi
}

check_scaleup_profile() {
  local profile="$1"
  local name="scale-up ($profile)"
  local log="$LOG_DIR/scaleup-$profile.md"
  local json="$LOG_DIR/scaleup-$profile.json"
  local t0=$SECONDS
  begin_check "$name"

  cargo run --release -p voxel-bench -- gate scaleup --profile "$profile" --out "$json" \
    >"$log" 2>&1
  local rc=$?
  local secs=$((SECONDS - t0))

  if (( rc != 0 )); then
    end_check "in-repo" "$name" FAIL "exit $rc" "$log" "$secs"
    return
  fi

  local memory probe
  memory=$(awk -F'|' '
    /^\| Memory projected / {
      for (i = 2; i <= NF; i++) gsub(/^[[:space:]]+|[[:space:]]+$/, "", $i)
      line = $3 " / " $4
    }
    END { print line }
  ' "$log")
  memory=$(printf '%s' "$memory" | sed -E 's/ \([^)]*\)//g')
  probe=$(awk -F'|' '
    /^\| Probe update / {
      for (i = 2; i <= NF; i++) gsub(/^[[:space:]]+|[[:space:]]+$/, "", $i)
      line = $3 " / " $4
    }
    END { print line }
  ' "$log")
  probe=$(printf '%s' "$probe" | sed -E 's/ \([^)]*\)//g')

  local check_status=PASS
  grep -q 'MISS' "$log" && check_status=WARN

  local detail=""
  [[ -n "$memory" ]] && detail="memory $memory"
  [[ -n "$probe" ]] && detail="${detail:+$detail; }probe $probe"
  [[ -z "$detail" ]] && detail="see $log"
  end_check "in-repo" "$name" "$check_status" "$detail" "$log" "$secs"
}

check_viewer_smoke() {
  local name="viewer smoke"
  if [[ "${HEALTH_VIEWER:-1}" == "0" ]]; then
    skip_check "in-repo" "$name" "disabled by HEALTH_VIEWER=0"
    VIEWER_SKIP_REASON="disabled by HEALTH_VIEWER=0"
    return
  fi
  if ! have timeout; then
    skip_check "in-repo" "$name" "timeout is not installed"
    VIEWER_SKIP_REASON="timeout is not installed"
    return
  fi

  local t0=$SECONDS
  begin_check "$name"
  local build_log="$LOG_DIR/viewer-build.log"
  if ! cargo build -p voxel-viewer >"$build_log" 2>&1; then
    end_check "in-repo" "$name" FAIL "viewer build failed" "$build_log" $((SECONDS - t0))
    return
  fi

  local triple="${CARGO_BUILD_TARGET:-}"
  local bin="$TARGET_DIR/${triple:+$triple/}debug/voxel-viewer"
  if [[ ! -x "$bin" ]]; then
    end_check "in-repo" "$name" FAIL "$bin is missing after the build" "$build_log" $((SECONDS - t0))
    return
  fi

  local -a wrapper=()
  if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    if have xvfb-run; then
      wrapper=(xvfb-run -a)
    else
      local reason="no DISPLAY/WAYLAND_DISPLAY and xvfb-run is not installed"
      end_check "in-repo" "$name" SKIP "$reason" "" $((SECONDS - t0))
      VIEWER_SKIP_REASON="$reason"
      return
    fi
  fi

  local secs="${HEALTH_VIEWER_SECS:-8}"
  local rust_log="${HEALTH_VIEWER_LOG:-wgpu_core=warn}"
  local log="$LOG_DIR/viewer-smoke.log"
  (
    cd "$ROOT" || exit 1
    "${wrapper[@]}" env RUST_LOG="$rust_log" timeout --signal=TERM "${secs}s" "$bin"
  ) >"$log" 2>&1
  local rc=$?
  local elapsed=$((SECONDS - t0))

  if grep -Eqi 'no suitable adapter|failed to find an appropriate adapter|no adapter found|cannot open display|unable to open display|Xvfb failed' "$log"; then
    local reason="no usable display/adapter on this host (GPU validation cannot run here)"
    end_check "in-repo" "$name" SKIP "$reason" "$log" "$elapsed"
    VIEWER_SKIP_REASON="$reason"
    return
  fi

  if grep -Eqi 'Validation Error|invalid usage|wgpu error|panicked at|thread .* panicked' "$log"; then
    end_check "in-repo" "$name" FAIL "validation error or panic in the viewer log" "$log" "$elapsed"
    return
  fi

  if (( rc == 124 || rc == 137 )); then
    end_check "in-repo" "$name" PASS \
      "stayed alive ${secs}s with RUST_LOG=$rust_log (no validation errors)" "$log" "$elapsed"
  elif (( rc == 0 )); then
    end_check "in-repo" "$name" PASS "exited cleanly" "$log" "$elapsed"
  else
    end_check "in-repo" "$name" FAIL "exit $rc" "$log" "$elapsed"
  fi
}

run_in_repo_checks() {
  run_check "in-repo" "cargo fmt" cargo fmt --all --check
  run_check "in-repo" "clippy" cargo clippy --workspace --all-targets -- -D warnings
  run_check "in-repo" "tests (workspace)" cargo test --workspace
  run_check "in-repo" "tests (chunk128)" cargo test --workspace --features chunk128
  run_check "in-repo" "texgen check" cargo run -p voxel-texgen -- check
  run_check "in-repo" "shader tests" cargo test -p voxel-render --test shaders
  check_viewer_validation
  run_check "in-repo" "chunk constants" ./scripts/check-constants.sh

  if [[ "${HEALTH_GATES:-1}" == "0" ]]; then
    skip_check "in-repo" "gate report (default)" "disabled by HEALTH_GATES=0"
    skip_check "in-repo" "gate report (minimum)" "disabled by HEALTH_GATES=0"
  else
    check_gate_profile default
    check_gate_profile minimum
  fi

  if [[ "${HEALTH_SCALEUP:-1}" == "0" ]]; then
    skip_check "in-repo" "scale-up (default)" "disabled by HEALTH_SCALEUP=0"
    skip_check "in-repo" "scale-up (minimum)" "disabled by HEALTH_SCALEUP=0"
  else
    check_scaleup_profile default
    check_scaleup_profile minimum
  fi

  if [[ "${HEALTH_BENCH:-1}" == "0" ]]; then
    skip_check "in-repo" "bench (LABEL=local)" "disabled by HEALTH_BENCH=0"
  else
    run_check "in-repo" "bench (LABEL=local)" env LABEL=local ./scripts/bench.sh
  fi

  check_viewer_smoke
  check_tasks
  check_gpu_validation
  check_frame_profiling
}

# ---- external analysis tools ---------------------------------------------

check_coverage() {
  local name="coverage (llvm-cov)"
  if [[ "${HEALTH_COVERAGE:-1}" == "0" ]]; then
    skip_check "external" "$name" "disabled by HEALTH_COVERAGE=0"
    return
  fi
  if ! have cargo-llvm-cov; then
    skip_check "external" "$name" "cargo-llvm-cov is not installed"
    return
  fi

  run_check "external" "$name" cargo llvm-cov --workspace --summary-only
  if [[ "${S_STATUS[LAST_INDEX]}" == "PASS" ]]; then
    local line total
    line=$(awk '/^TOTAL/ { print; exit }' "$LAST_LOG")
    total=$(awk '/^TOTAL/ { print $10; exit }' "$LAST_LOG")
    if [[ -n "$line" ]]; then
      set_last_detail "lines ${total:-?}; see ${LAST_LOG#"$ROOT"/}"
    fi
  fi
}

check_mutants() {
  if [[ "${HEALTH_MUTANTS:-1}" == "0" ]]; then
    skip_check "external" "mutants" "disabled by HEALTH_MUTANTS=0"
    return
  fi
  if ! have cargo-mutants; then
    skip_check "external" "mutants" "cargo-mutants is not installed"
    return
  fi

  local -a command=(cargo mutants)
  local args_label="workspace"
  if [[ -n "${HEALTH_MUTANTS_ARGS:-}" ]]; then
    # Intentional word splitting: this is a small, documented argument string.
    read -r -a extra <<<"$HEALTH_MUTANTS_ARGS"
    command+=("${extra[@]}")
    args_label="$HEALTH_MUTANTS_ARGS"
  else
    command+=(--workspace)
  fi
  # A shard must not share `mutants.out` or the log with its siblings: cargo
  # clears the output directory at start, so parallel shards delete each
  # other's results and the last one is reported as a full pass.
  local slugged
  slugged=$(printf '%s' "$args_label" | tr -c 'A-Za-z0-9' '-' | sed 's/--*/-/g; s/^-//; s/-$//')
  local out="${HEALTH_MUTANTS_OUT:-$TARGET_DIR/mutants/$slugged}"
  command+=(--output "$out")
  local name="mutants ${slugged}"
  run_check "external" "$name" "${command[@]}"
  if [[ "${S_STATUS[LAST_INDEX]}" != "PASS" ]]; then
    local summary
    summary=$(grep -Ei 'mutants tested|caught|missed' "$LAST_LOG" | tail -n 1)
    [[ -n "$summary" ]] && set_last_detail "$summary"
  fi
}

check_deny() {
  local name="deny"
  if ! have cargo-deny; then
    skip_check "external" "$name" "cargo-deny is not installed"
    return
  fi

  local -a command=(cargo deny)
  if [[ -n "${HEALTH_DENY_CONFIG:-}" ]]; then
    command+=(--config "$HEALTH_DENY_CONFIG")
  fi
  command+=(check)
  run_check "external" "$name" "${command[@]}"
}

check_audit() {
  local name="audit"
  if ! have cargo-audit; then
    skip_check "external" "$name" "cargo-audit is not installed"
    return
  fi
  run_check "external" "$name" cargo audit
  if [[ "${S_STATUS[LAST_INDEX]}" != "PASS" ]]; then
    local summary
    summary=$(grep -Ei 'vulnerabilit|warning' "$LAST_LOG" | tail -n 1)
    [[ -n "$summary" ]] && set_last_detail "$summary"
  fi
}

check_udeps() {
  local name="udeps"
  if ! have cargo-udeps; then
    skip_check "external" "$name" "cargo-udeps is not installed"
    return
  fi
  if ! have rustup; then
    skip_check "external" "$name" "rustup is not installed (cargo udeps needs nightly)"
    return
  fi
  if ! rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
    skip_check "external" "$name" "the nightly toolchain is not installed"
    return
  fi
  if ! cargo +nightly udeps --version >/dev/null 2>&1; then
    skip_check "external" "$name" "cargo-udeps is not available on nightly"
    return
  fi
  run_check "external" "$name" cargo +nightly udeps --workspace --all-targets
}

check_bloat() {
  local name="bloat"
  if ! have cargo-bloat; then
    skip_check "external" "$name" "cargo-bloat is not installed"
    return
  fi
  run_check "external" "$name" \
    cargo bloat --release -p voxel-viewer --bin voxel-viewer -n 20
  if [[ "${S_STATUS[LAST_INDEX]}" == "PASS" ]]; then
    local total
    total=$(grep -E '^[[:space:]]*Total' "$LAST_LOG" | tail -n 1 | sed -E 's/^[[:space:]]+//; s/[[:space:]]+/ /g')
    [[ -n "$total" ]] && set_last_detail "$total"
  fi
}

check_profile() {
  local name="flamegraph/samply"
  if [[ "${HEALTH_PROFILE:-1}" == "0" ]]; then
    skip_check "external" "$name" "disabled by HEALTH_PROFILE=0"
    return
  fi

  if have cargo-flamegraph && have perf && perf stat -e task-clock true >/dev/null 2>&1; then
    run_check "external" "$name" \
      cargo flamegraph --release -p voxel-bench --output "$LOG_DIR/flamegraph.svg" -- \
      cpu --out "$LOG_DIR/flame-bench"
    if [[ "${S_STATUS[LAST_INDEX]}" == "FAIL" ]] \
      && grep -Eqi 'permission denied|perf_event_paranoid|not permitted|access to performance' "$LAST_LOG"; then
      set_last_status SKIP
      set_last_detail "perf is not permitted on this host; see ${LAST_LOG#"$ROOT"/}"
    elif [[ "${S_STATUS[LAST_INDEX]}" == "PASS" ]]; then
      set_last_detail "flamegraph ${LOG_DIR#"$ROOT"/}/flamegraph.svg"
    fi
    return
  fi

  if have samply; then
    local build_log="$LOG_DIR/samply-build.log"
    local t0=$SECONDS
    begin_check "$name"
    if ! cargo build --release -p voxel-bench >"$build_log" 2>&1; then
      end_check "external" "$name" FAIL "release voxel-bench build failed" "$build_log" $((SECONDS - t0))
      return
    fi

    local log="$LOG_DIR/samply.log"
    samply record --save-only --output "$LOG_DIR/samply.json" -- \
      "$TARGET_DIR/release/voxel-bench" cpu --out "$LOG_DIR/samply-bench" >"$log" 2>&1
    local rc=$?
    local secs=$((SECONDS - t0))
    if (( rc == 0 )); then
      end_check "external" "$name" PASS "samply ${LOG_DIR#"$ROOT"/}/samply.json" "$log" "$secs"
    elif grep -Eqi 'permission denied|perf_event_paranoid|not permitted|access to performance|failed to start' "$log"; then
      end_check "external" "$name" SKIP "profiling is not permitted on this host" "$log" "$secs"
    else
      end_check "external" "$name" FAIL "exit $rc" "$log" "$secs"
    fi
    return
  fi

  if have cargo-flamegraph; then
    skip_check "external" "$name" "cargo-flamegraph is installed but perf is unavailable"
  else
    skip_check "external" "$name" "neither cargo-flamegraph nor samply is installed"
  fi
}

check_tokei() {
  local name="tokei"
  if ! have tokei; then
    skip_check "external" "$name" "tokei is not installed"
    return
  fi
  run_check "external" "$name" tokei .
  if [[ "${S_STATUS[LAST_INDEX]}" == "PASS" ]]; then
    local total
    total=$(grep -E '^[[:space:]]*Total' "$LAST_LOG" | tail -n 1 | sed -E 's/^[[:space:]]+//; s/[[:space:]]+/ /g')
    [[ -n "$total" ]] && set_last_detail "$total"
  fi
}

run_external_checks() {
  if [[ "${HEALTH_EXTERNAL:-1}" == "0" ]]; then
    skip_check "external" "external tools" "disabled by HEALTH_EXTERNAL=0"
    return
  fi
  check_coverage
  check_mutants
  check_deny
  check_audit
  check_udeps
  check_bloat
  check_profile
  check_tokei
}

# ---- summary --------------------------------------------------------------

print_group() {
  local group="$1" heading="$2" found=0 index color
  for index in "${!S_NAME[@]}"; do
    [[ "${S_GROUP[index]}" == "$group" ]] || continue
    if (( found == 0 )); then
      printf '\n%s%s%s\n' "$C_BOLD" "$heading" "$C_RESET"
      found=1
    fi
    color="$C_INFO"
    case "${S_STATUS[index]}" in
      PASS) color="$C_PASS" ;;
      FAIL) color="$C_FAIL" ;;
      WARN) color="$C_WARN" ;;
      SKIP) color="$C_SKIP" ;;
    esac
    printf '  %s%-5s%s  %-34s %s\n' \
      "$color" "${S_STATUS[index]}" "$C_RESET" \
      "$(fit "${S_NAME[index]}" 34)" "$(fit "${S_DETAIL[index]}" 108)"
  done
}

print_summary() {
  local index status pass=0 fail=0 warn=0 skip=0 info=0
  for index in "${!S_STATUS[@]}"; do
    status="${S_STATUS[index]}"
    case "$status" in
      PASS) pass=$((pass + 1)) ;;
      FAIL) fail=$((fail + 1)) ;;
      WARN) warn=$((warn + 1)) ;;
      SKIP) skip=$((skip + 1)) ;;
      INFO) info=$((info + 1)) ;;
    esac
  done

  # Open exit artifacts come from the task list (M0, M1 and M1.5 only; the
  # current milestone's backlog is counted but not enumerated here).
  local item
  for item in "${OPEN_EXIT[@]}"; do
    ATTENTION+=("OPEN $item")
  done
  for item in "${GATE_ATTENTION[@]}"; do
    ATTENTION+=("$item")
  done
  if (( FRAME_GAP == 1 )); then
    ATTENTION+=("GAP frame-time profiling: no average/1%-low/hitch lens in the bench suite; the viewer only logs FPS at debug level")
  fi
  if (( GPU_GAP == 1 )); then
    ATTENTION+=("GAP M1.5-16: CI has no zero-validation viewer job (RUST_LOG=wgpu_core=warn)")
  fi
  if [[ -n "$VIEWER_SKIP_REASON" && "$VIEWER_SKIP_REASON" != "disabled by HEALTH_VIEWER=0" ]]; then
    ATTENTION+=("GAP viewer smoke: $VIEWER_SKIP_REASON")
  fi

  printf '\n%sHEALTH SUMMARY%s (%s)\n' \
    "$C_BOLD" "$C_RESET" "$(human_secs $((SECONDS - START_SECONDS)))"
  print_group "in-repo" "IN-REPO"
  print_group "external" "EXTERNAL"

  if (( ${#ATTENTION[@]} > 0 )); then
    printf '\n%sTARGETS / ATTENTION%s\n' "$C_BOLD" "$C_RESET"
    for item in "${ATTENTION[@]}"; do
      printf '  - %s\n' "$(fit "$item" 118)"
    done
  fi

  local result="PASS" exit_code=0
  if (( fail > 0 )) || { [[ "${HEALTH_STRICT:-0}" == "1" ]] && (( warn > 0 )); }; then
    result="FAIL"
    exit_code=1
  fi

  printf '\n%sRESULT: %s%s — %d passed, %d failed, %d warning(s), %d skipped' \
    "$C_BOLD" "$result" "$C_RESET" "$pass" "$fail" "$warn" "$skip"
  (( info > 0 )) && printf ', %d info' "$info"
  printf '\nlogs: %s\n' "$LOG_DIR"

  return "$exit_code"
}

# ---- main -----------------------------------------------------------------

main() {
  if [[ "${HEALTH_INREPO:-1}" != "0" ]]; then
    run_in_repo_checks
  fi
  run_external_checks
  print_summary
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main
  exit $?
fi
