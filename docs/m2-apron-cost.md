# M2-02 — Apron Cost

**Status:** Complete — 2026-10-29
**Task:** M2-02, apron cost benchmark vs a dense 130³ copy
**Host:** Intel Core i7-10700, 4 vCPU under WSL2. **Not the reference machine**, so the absolute times are a development check, not a gate number; the byte ratio and the direction of the time result are what M2-02 asks for.

## Command

```sh
cargo bench -p voxel-storage --bench apron
cargo bench -p voxel-storage --features chunk128 --bench apron
```

The benchmark loads a target chunk and its six face neighbours, measures
`World::apron`, then measures a direct `(CHUNK_DIM + 2)³` read. The dense path
does one `World::get` per padded voxel, exactly the materialisation the apron
avoids.

## Results

| Edge | Face planes | Dense padded copy | Bytes | Time |
|---|---:|---:|---:|---:|
| 64³ | 49,152 B / 694 µs | 574,992 B / 10.52 ms | **8.5%** | **6.6%** |
| 128³ | 196,608 B / 2.70 ms | 4,394,000 B / 63.01 ms | **4.5%** | **4.3%** |

Criterion ranges from the same run:

| Edge | `face_planes` | `dense_padded` |
|---|---|---|
| 64³ | 805 µs – 1.23 ms | 10.16 – 13.31 ms |
| 128³ | 2.64 – 3.20 ms | 67.2 – 76.3 ms |

## Verdict

The face-plane path wins on both axes and clears the M2-02 limit of ≤ 25% of
the dense path's bytes and time with a wide margin. Keep the apron API.

The 128³ numbers are the relevant production edge: 4.5% of the dense bytes and
4.3% of its time on this host. Re-run on the reference machine if these become
a gate citation; the bench is deterministic in what it measures, but wall-clock
timings are host-specific.
