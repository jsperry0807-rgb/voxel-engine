# Benchmark archive

This folder is the durable history of engine benchmark runs. `target/bench`
holds whatever was measured last and is ignored by git; a run that is worth
remembering is copied here by `voxel-bench record`.

## Layout

```text
benchmarks/
├── README.md          # this file: method, rules, caveats
├── HISTORY.md         # generated index of every run, newest first, with deltas
├── history.json       # machine-readable run summaries and stable metrics
└── runs/
    └── 20261025T144501Z-baseline/
        ├── report.json     # every memory, CPU and latency case
        ├── report.md       # human-readable version
        ├── run.json        # summary row for this run
        ├── criterion.txt   # raw Criterion output
        ├── iai.txt         # raw iai-callgrind instruction/cache/branch counts
        └── perf.txt        # raw perf-stat counters
```

The raw files are optional, depending on which tools existed on the machine.

## Recording a run

`scripts/bench.sh` archives the run by default:

```sh
scripts/bench.sh                 # auto-labeled auto-<short-commit>
LABEL=baseline scripts/bench.sh  # name it yourself
RECORD=0 scripts/bench.sh        # report only, keep it out of the archive
```

or record an already-produced `target/bench` run:

```sh
cargo run --release -p voxel-bench -- record \
  --from target/bench \
  --label baseline \
  --note "hot/cold material split, GPU upload layer" \
  --attach target/bench/criterion.txt \
  --attach target/bench/iai.txt \
  --attach target/bench/perf.txt
```

`record` writes a timestamped directory under `runs/`, appends a summary to
`history.json`, and regenerates `HISTORY.md`. The run records:

- every report case (full JSON and Markdown),
- peak process RSS, CPU time and utilization,
- stable headline metrics used for the trend table,
- the git commit and whether the worktree was dirty (`*` in `HISTORY.md`),
- the CPU model, OS and architecture.

## Naming runs

Use a label that says what changed, not just when it ran:

- `baseline`
- `before-hot-cold-split`
- `hot-cold-split`
- `gpu-upload-layer`
- `gpu-overlay`
- `sve-on` / `sve-off`

The timestamp makes the folder unique; the label makes the history readable.
Re-recording the same label is allowed and gets a numeric suffix.

## What goes in the trend

`HISTORY.md` tracks the metrics that actually change decisions:

| Metric | Lens |
| --- | --- |
| peak RSS, CPU time | process (only when the case set matches) |
| registry 65k, terrain chunk, 256-chunk GPU world, material GPU tables | memory |
| GPU overlay for 1 block and 64 blocks | memory |
| registry freeze, hot lookup, resolved lookup, palette resolve | CPU |
| DAG build/get, GPU lookup, GPU overlay lookup, 100-edit COW, overlay set/flush | CPU |
| hot lookup, DAG get, GPU edit, GPU overlay set, `World::set` p99 | latency |

Peak RSS and CPU time are whole-run totals: adding a case raises them without
any code regression. Each run stores a suite fingerprint, and `HISTORY.md`
omits those two deltas when the fingerprint changes. Re-record a baseline after
adding cases if you need the process comparison.

The full case list is in each run's `report.md`.

## Rules for a useful history

1. **Same host, same power profile.** Frequency scaling and turbo make
   Criterion numbers drift between hosts. Memory and iai counts are more
   portable; use iai for regression gates.
2. **One change per recorded run.** A run that bundles five changes cannot
   tell you which one cost 8%.
3. **Record the commit.** `benchmarks/HISTORY.md` marks dirty worktrees with
   `*`; a history entry without a clean commit is a local experiment.
4. **Do not commit every experiment.** Keep throwaway runs in `target/bench`;
   archive the ones that establish a baseline or prove a change.
5. **Read the whole table.** A change that improves ns/op but doubles peak RSS
   is not a win for a render-distance-bound engine.
6. **Keep the case set fixed between compared runs.** Adding a case is a suite
   change, not an optimization; the history drops the whole-run deltas when the
   suite fingerprint differs.

## Caveats

- Heap numbers come from the counting allocator and exclude stack and mapped
  pages; RSS is the process truth and moves in 4 KiB pages.
- The CPU lens is single-threaded, so CPU utilization stays near 100%.
- Criterion and perf are noisy; deltas under a few percent are not meaningful.
- iai-callgrind is deterministic, but its cache model is a simulation, not the
  host's real cache.
- GPU cases skip when no wgpu adapter is available; the run is still valid for
  the CPU and memory lenses.
