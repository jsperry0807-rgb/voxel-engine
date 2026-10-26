#!/usr/bin/env bash
# Guard chunk geometry against hard-coded 6-bit literals (M0-23, M1-25).
#
# The chunk edge, the level count and the blocks-per-chunk count come from
# crates/core/src/constants.rs. Code outside that module must derive them with
# CHUNK_BITS / CHUNK_DIM / ROOT_LEVEL / BLOCKS_PER_CHUNK so the pre-registered
# 128^3 edge (CHUNK_BITS = 7, feature `chunk128`) stays a one-place change.
#
# Usage:
#   scripts/check-constants.sh            # warn only (M0 default); exits 0
#   scripts/check-constants.sh --strict   # exits 1 if anything is found
#   STRICT=1 scripts/check-constants.sh   # same as --strict
#
# M0-23 lands this guard in warn mode; M1-25 flips the default to strict and
# clears the allow-list.
#
# What is checked (Rust sources under crates/ only):
#   >> 6, >>6                 hard-coded 6-bit shift
#   & 63, &0x3f, & 0x3F       hard-coded 6-bit mask
#   ROOT_LEVEL = 6            chunk-geometry constant redefined with a literal
#   BLOCKS_PER_CHUNK = 64     (the same check covers CHUNK_BITS, CHUNK_DIM,
#                              CHUNK_VOLUME, BLOCK_LEVEL, BLOCKS_PER_EDGE and
#                              LEVEL_SIZE)
#
# What is not checked:
#   - crates/core/src/constants.rs, the single source of truth.
#   - Markdown documentation. Comments inside Rust files are scanned; this is a
#     text grep, not a parser.
#   - WGSL: M1-23 asserts the shader mirror against the Rust constants.
#   - unrelated uses of 64 (cache lines, byte counts, ...). The guard keys on
#     shifts, masks and the geometry constant names rather than the bare
#     number for exactly that reason.
#
# Allow-list:
#   ALLOW_FILES below lists temporary source-file exceptions. Add a file only
#   with a one-line reason and keep it as narrow as possible; every entry is
#   removed in M1-25. It is empty during M0, so the current offenders are all
#   reported. The constants module itself is always skipped and is not part of
#   this list.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ ! -d crates ]]; then
  echo "check-constants: crates/ not found; run this from the repository" >&2
  exit 2
fi

strict="${STRICT:-0}"
for arg in "$@"; do
  case "$arg" in
    --strict|--error) strict=1 ;;
    --warn) strict=0 ;;
    -h|--help)
      echo "usage: $0 [--warn|--strict]"
      echo "see the header of this script for the checked patterns and allow-list"
      exit 0
      ;;
    *)
      echo "check-constants: unknown argument: $arg" >&2
      echo "usage: $0 [--warn|--strict]" >&2
      exit 2
      ;;
  esac
done

CONSTANTS="crates/core/src/constants.rs"

# Temporary allow-list: space-separated exact paths. Empty during M0; M1-25
# clears it. If an entry is unavoidable before then, put the reason above it.
# Example:
#   ALLOW_FILES="crates/engine/storage/src/lib.rs"  # region cell uses << 6
ALLOW_FILES=""

# 1. A right shift by 6, but not 60/64/...
# 2. A mask of 63 (decimal or hex), but not 630 or 0x3f0...
# 3. A geometry constant assigned a bare numeric literal, with or without a
#    type. Requiring the semicolon keeps derived values such as
#    `CHUNK_DIM = 1 << CHUNK_BITS;` out of the report.
pattern='(>>[[:space:]]*6([^0-9]|$)|&[[:space:]]*(63|0[xX]3[fF])([^0-9a-fA-F]|$)|(CHUNK_BITS|CHUNK_DIM|CHUNK_VOLUME|ROOT_LEVEL|BLOCK_LEVEL|BLOCKS_PER_CHUNK|BLOCKS_PER_EDGE|LEVEL_SIZE)[[:space:]]*(:[^=]*)?=[[:space:]]*[0-9]+((u|i)(8|16|32|64|size))?[[:space:]]*;)'

raw="$(grep -REn --include='*.rs' -- "$pattern" crates 2>/dev/null || true)"

matches=""
count=0
while IFS= read -r line; do
  [[ -z "$line" ]] && continue
  file="${line%%:*}"
  allowed=0
  if [[ "$file" == "$CONSTANTS" ]]; then
    allowed=1
  else
    for entry in $ALLOW_FILES; do
      if [[ "$file" == "$entry" ]]; then
        allowed=1
        break
      fi
    done
  fi
  if (( allowed == 0 )); then
    matches+="$line"$'\n'
    count=$((count + 1))
  fi
done <<< "$raw"

if (( count == 0 )); then
  echo "check-constants: OK - no 6-bit literals outside the constants module."
  exit 0
fi

echo "check-constants: found $count offending line(s) outside the constants module:"
printf '%s' "$matches"
echo
if [[ "$strict" == "1" ]]; then
  echo "check-constants: strict mode - failing (M1-25)." >&2
  exit 1
fi
echo "check-constants: warn-only in M0; this becomes an error in M1-25."
exit 0
