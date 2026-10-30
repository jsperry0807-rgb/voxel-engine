# M2-02 — Apron Cost

**Status:** Complete — re-measured 2026-10-29 after fixing the benchmark method
**Task:** M2-02, apron cost benchmark vs a dense 130³ copy
**Host:** Intel Core i7-10700, 4 vCPU under WSL2. **Not the reference machine**, so the absolute times are a development check, not a gate number; the byte ratio and the direction of the time result are what M2-02 asks for.

## Command

```sh
cargo bench -p voxel-storage --bench apron
cargo bench -p voxel-storage --features chunk128 --bench apron
```

The benchmark loads a target chunk and its six face neighbours, then measures
two ways of getting the padded neighbourhood:

- **apron**: `World::apron_into`, the mesher's path, into a reused `Apron`;
- **dense**: resolve each of the 27 chunks once via `World::chunk`, then copy
  every padded voxel out of that chunk.

The earlier version called `World::get` once per padded voxel and reallocated
the apron every iteration, so it measured hash lookups and allocation rather
than the copy the apron avoids. Both buffers are now allocated once, and the
timed dense path does not repeat a chunk lookup per voxel.

## Results

| Edge | Face planes | Dense padded copy | Bytes | Time |
|---|---:|---:|---:|---:|
| 64³ | 49,152 B / 445 µs | 574,992 B / 4.90 ms | **8.5%** | **9.1%** |
| 128³ | 196,608 B / 1.57 ms | 4,394,000 B / 27.4 ms | **4.5%** | **5.7%** |

Criterion ranges from the same run:

| Edge | `face_planes` | `dense_padded` |
|---|---|---|
| 64³ | 448 µs – 460 µs | 4.67 – 4.69 ms |
| 128³ | 1.57 – 1.60 ms | 29.1 – 30.3 ms |

## Verdict

The face-plane path wins on both axes and clears the M2-02 limit of ≤ 25% of
the dense path's bytes and time with a wide margin. Keep the apron API.

The 128³ numbers are the relevant production edge: 4.5% of the dense bytes and
5.7% of its time on this host. The time ratio is higher than the first
(6.6% / 4.3%) measurement because the dense side no longer pays a per-voxel
chunk lookup; the byte ratio is geometric and barely moved. Re-run on the
reference machine if these become a gate citation; the bench is deterministic
in what it measures, but wall-clock timings are host-specific.
