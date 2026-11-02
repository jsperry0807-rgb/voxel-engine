#!/usr/bin/env bash
# Classify the files this CI run touches into build areas so jobs that cannot
# be affected can be skipped.
#
# Writes `name=true|false` lines to $GITHUB_OUTPUT (`/dev/stdout` when unset):
#
#   all        range unknown, or a dependency/toolchain/workflow change
#   code       any change that can affect a build (everything but `docs`)
#   rust       compiled Rust sources
#   textures   texture generator inputs and outputs
#   mesh       mesh crate, its fixtures and goldens
#   worldgen   noise/macro generator and its goldens
#   shaders    WGSL sources and the render crate
#   viewer     viewer crate, launcher and the offscreen validation
#   bench      gate, profiling lens and benchmark fixtures
#   docs       prose only
#
# Environment:
#   BASE_SHA, HEAD_SHA  commit range to classify
#   FORCE_ALL=1         treat everything as changed (manual dispatch)
#
# Any failure to work out the range is treated as "everything changed": a
# skipped job must never be why a regression lands.

set -euo pipefail

rust=false
textures=false
mesh=false
worldgen=false
shaders=false
viewer=false
bench=false
docs=false
all=false

files=""

if [[ ${FORCE_ALL:-0} == 1 ]]; then
  all=true
else
  base=${BASE_SHA:-}
  head=${HEAD_SHA:-}
  # A missing or zero base means a new branch, a manual dispatch or a force
  # push; there is no trustworthy range to diff against.
  if [[ -z $base || -z $head || $base =~ ^0+$ ]]; then
    all=true
  elif ! files=$(git diff --name-only --no-renames "$base" "$head" 2>/dev/null); then
    all=true
  fi
fi

while IFS= read -r path; do
  [[ -z $path ]] && continue

  # Dependency, toolchain and workflow changes can move any job's outcome.
  case $path in
    Cargo.toml | Cargo.lock | rust-toolchain | rust-toolchain.toml | .cargo/* | .github/* | scripts/changed-areas.sh)
      all=true
      continue
      ;;
  esac

  # The helper scripts are shelled out to by the Rust jobs.
  if [[ $path == scripts/*.sh ]]; then
    rust=true
  fi

  if [[ $path == *.rs || $path == */Cargo.toml ]]; then
    rust=true
  fi
  if [[ $path == textures/* || $path == crates/engine/block/* || $path == crates/engine/texgraph/* ]]; then
    textures=true
  fi
  if [[ $path == crates/engine/mesh/* ]]; then
    mesh=true
  fi
  if [[ $path == crates/engine/worldgen/* ]]; then
    worldgen=true
  fi
  if [[ $path == *.wgsl || $path == crates/engine/render/* ]]; then
    shaders=true
  fi
  if [[ $path == crates/engine/viewer/* || $path == apps/launcher/* ]]; then
    viewer=true
  fi
  if [[ $path == crates/bench/* || $path == crates/profiling/* ]]; then
    bench=true
  fi
  case $path in
    docs/* | *.md | mkdocs.yml | site.toml | scripts/build-docs-site.py)
      docs=true
      ;;
  esac
done <<<"$files"

if [[ $all == true ]]; then
  rust=true
  textures=true
  mesh=true
  worldgen=true
  shaders=true
  viewer=true
  bench=true
  docs=true
fi

code=false
if [[ $rust == true || $textures == true || $mesh == true || $worldgen == true ||
  $shaders == true || $viewer == true || $bench == true ]]; then
  code=true
fi

{
  echo "all=$all"
  echo "code=$code"
  echo "rust=$rust"
  echo "textures=$textures"
  echo "mesh=$mesh"
  echo "worldgen=$worldgen"
  echo "shaders=$shaders"
  echo "viewer=$viewer"
  echo "bench=$bench"
  echo "docs=$docs"
} | if [[ -n ${GITHUB_OUTPUT:-} ]]; then
  tee -a "$GITHUB_OUTPUT"
else
  cat
fi
