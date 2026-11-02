# M3-02 mesh buffer pool

**Status:** landed 2026-10-31. One paged arena with byte-range suballocation
replaces the per-chunk vertex and index buffers; the ring loads 256 chunks from
16 pages of 8 MiB and creates no buffer after warm-up.

## Acceptance

| Requirement | Measured |
|---|---|
| No per-frame allocation after warm-up | 256 meshes come from **256 arena allocations in 16 pages**; the pages are created **once** (0.04–0.05 s total). A whole 64-column flight adds at most one page. The frame thread never calls `create_buffer`. |
| Fragmentation reported | `ArenaStats` reports used/slack/holes/largest hole/fragmentation; the pool logs one line when the ring completes and `tests/pool.rs` reports every page size over a 64-column flight. |

The pool does not remove wgpu's per-write staging allocation — that is M3-03's
upload queue (see *Limits*).

## Decision: one arena, not per-chunk buffers

M3-01 measured one chunk upload at **6.2 ms mean / 20.9 ms max**, almost all of
it two `create_buffer` calls plus mapped writes. Per-chunk buffers cannot meet
"No per-frame allocation after warm-up": every install allocates and every
eviction frees, forever, while the camera moves. The arena pays that cost once,
during warm-up, and steady state becomes free-list bookkeeping.

Fixed-size per-chunk slots were rejected as well: a slot must fit the largest
mesh (128³ p95 **2,115 KiB**), which wastes ~40 % against the 470 KiB mean and
still caps the mesh. The arena only commits 6 % over the live bytes.

ADR-010 and the M3-00 plan require the pool to be **format-agnostic**, to page
any single binding at **≤128 MiB**, and to let **no draw cross a page**. A
`MeshFormat` descriptor (`vertex_stride`, `index_stride`, `0` for a record
stream) plus a byte-granular arena satisfies all three.

## Design

### Arena (`mesh_pool::Arena`)

- Pages are `page_bytes` long; a range never crosses a page.
- Each page has a bump pointer and a sorted free list of `(offset, len)`.
- **Allocate:** best fit across every free list, then the bump tail, then a new
  page. Best fit is what lets a ring rotation reuse the column it just evicted.
- **Free:** insert sorted, merge both neighbours, and return a block that
  reaches the bump pointer to the bump.
- Ranges and lengths are 4 B aligned, the `Queue::write_buffer` copy alignment;
  `set_vertex_buffer` also needs a 4 B offset.
- Accounting is exact (`used + free == capacity` is asserted in debug builds);
  stats split free bytes into page **slack** (unused tails, one per page) and
  **holes** (freed ranges inside pages). Fragmentation is measured over holes
  only: `1 - largest_hole / hole_bytes`.

### Pool (`mesh_pool::MeshPool`)

- One mesh is **one contiguous range**: vertices first, indices at the next 4 B
  boundary. Two separate ranges per chunk doubled the free-list fragments and
  doubled the allocation count; one range keeps a rotation's leftovers smaller.
- A `MeshAllocation` holds an `Arc` of its page plus the range, counts and
  format. The page index is stable because pages are never removed, and the
  `Arc` keeps a page alive even if the pool is dropped first.
- `free` consumes the allocation, so a stale handle cannot be drawn or freed
  twice.
- Pages are `VERTEX | INDEX | COPY_DST`. `MeshPool::with_usage` exists for the
  record path (`STORAGE | COPY_DST`) and for tests that read a range back.

### Streaming integration

- The uploader thread allocates a range, writes the mesh, and only then sends
  the finished resource to the frame thread. Allocation is the only section
  under the pool mutex; the write happens outside it.
- The frame thread frees an evicted chunk in `recenter`, **before** queueing
  the replacement column, so the uploader can reuse those ranges immediately.
- Results that arrive for a position the camera already left are freed in
  `pump` instead of being dropped. That was a leak in the M3-01 install path
  once ranges became owned.
- `desired` is rebuilt in place; a recenter does not reallocate the set.

### Aliasing safety

A range is only reused after `MeshPool::free`, which happens on the frame
thread before the pass that draws. `Queue::write_buffer` is ordered before the
next submission and wgpu-core transitions the whole buffer from its previous
read use, so the GPU never reads a range while it is being rewritten. The
frame that installs a mesh is also the frame that first draws it, and both the
write and the draw are ordered in the queue.

## Page size

8 MiB. The measured flight (`tests/pool.rs`, 16 real ring meshes, 64 columns)
over page sizes:

| Page | 64³ ring (30 MiB live) | 128³ ring (120 MiB live) |
|---:|---|---|
| 4 MiB | 8 pages, 32 MiB, 1.044×, 0.12 MiB holes | 31→32 pages, 124 MiB, 1.083×, 0.18 MiB holes |
| **8 MiB** | **4 pages, 32 MiB, 1.063×, 0.08 MiB holes** | **15→16 pages, 128 MiB, 1.095×, 0.67 MiB holes** |
| 16 MiB | 2 pages, 32 MiB, 1.057×, 0.36 MiB holes | 8 pages, 128 MiB, 1.081×, 0.86 MiB holes |
| 32 MiB | 1 page, 32 MiB, 1.045×, 0.02 MiB holes | 4 pages, 128 MiB, 1.080×, 1.28 MiB holes |

8 MiB commits the least VRAM for the **static** ring, which is the number
M3-11 records: 120 MiB of a 128 MiB budget at 128³. Under a flight every page
size except 16/32 MiB eventually adds one page at 128³, so growth is a property
of the size mix, not the constant. Page creation measured 0–17 ms on the RTX
4060, i.e. about a millisecond per page once the driver is warm. The size is
overridable with `VOXEL_POOL_PAGE_MIB` (4–128) for experiments.

`MAX_PAGE_BYTES` is 128 MiB, the WebGPU default
`max_storage_buffer_binding_size` that the instanced-record path binds, and
`MeshPool::new` asserts `page_bytes <= max_buffer_size`.

## Measured run

Release viewer, RTX 4060 / Radeon iGPU, 1920×1080, `--features chunk128`, budget
camera. `VOXEL_VIEWER_ATLAS` pointed at the local bake because the concurrent
texgen face-table work had regenerated `data/textures.ron` without its atlas
hash.

| GPU | Ring | Max pump | Hitches | Pool | Windows / holes | Pages created |
|---|---|---:|---:|---|---|---:|
| RTX 4060 | 256/256 | **0.09 ms** | **0** | 16 × 8 MiB = **128 MiB** for **120.3 MiB** | 7.7 MiB / 0.0 MiB | 0.05 s |
| Radeon iGPU | 256/256 | **0.09 ms** | **0** | 16 × 8 MiB = **128 MiB** for **120.3 MiB** | 7.7 MiB / 0.0 MiB | 0.04 s |
| RTX 4060, 64³ | 256/256 | **0.09 ms** | **0** | 4 × 8 MiB = **32 MiB** for **30.2 MiB** | 1.8 MiB / 0.0 MiB | 0.00 s |

The 128³ ring is 3,679–14,920 quads/chunk (mean 11,198, 44 B/quad, 120.3 MiB),
the same scene M3-01 measured.

### Where the upload time goes

Per-chunk stages from `RUST_LOG=voxel_viewer=trace`, 256 chunks, release:

| Stage | 128³ mean / p50 / p99 / max (ms) | 64³ mean / p50 / p99 / max (ms) |
|---|---|---|
| generation (worker) | 79.8 / 78.8 / 143 / 166 | 8.1 / 6.5 / 17.8 / 20.4 |
| uploader starved (waiting for content) | 2.0 / 0 / 50 / 68 | 0.4 / 0 / 6.2 / 11.0 |
| arena allocate (page creation included) | 0.18 / 0 / 0.10 / 43 | 0.004 / 0 / 0.07 / 0.33 |
| `Queue::write_buffer` | 17.4 / **1.7** / 307 / 364 | 3.6 / **0.4** / 177 / 206 |
| lighting buffers | 1.5 / 0.2 / 9.6 / 239 | 0.3 / 0.09 / 4.2 / 14.6 |
| upload total (alloc + write + lighting) | 19.1 / 1.9 / 308 / 365 | 3.9 / 0.5 / 178 / 208 |

Two things this table makes explicit:

1. **The arena is not the cost.** Allocation is ~0.2 ms at 128³, and page
   creation is a one-off during warm-up. The pool's contribution to an install
   is a free-list search plus a slice.
2. **The remaining write cost is wgpu's staging path.** `write_buffer` allocates
   a staging buffer per call and, about once per frame, waits for a submission:
   ~15 of the 256 writes take 110–360 ms while the p50 is 1.7 ms. That is the
   same driver behaviour M3-01 saw with per-chunk buffers, which is exactly why
   M3-03 replaces the per-mesh write with a bounded staging ring and a per-frame
   byte budget. The M3-01 figure (6.2/20.9 ms) was the uploader's wall clock per
   chunk, so it also counted staging allocation and waiting; it is not a
   like-for-like comparison with the stage split above.

## Fragmentation

Holes are the price of variable-size suballocation without compaction. A
rotation frees a 16-chunk column and loads a different one; best fit reuses the
closest-sized ranges, and the size difference stays behind. Measured on the
RTX 4060 probe at 128³: **0.5–0.7 MiB of new holes per chunk-edge rotation**
(largest hole 120–430 KiB). After ~8 rotations the free space can no longer
serve a mean mesh and the arena adds one 8 MiB page.

What is bounded: `tests/pool.rs` requires the page count not to grow past
warm + 2 over 64 columns, capacity/used < 1.5, and holes < 5 % of live bytes.
What is not: a very long flight slowly grows the committed capacity. Options if
that becomes a problem — fixed-size slots (+~40 % capacity but zero
fragmentation), or compaction, which needs the CPU mesh bytes kept alive (M4's
versioned resources / M5's residency work). Recorded here rather than papered
over.

The `write_buffer` alignment rule and the page-limit rules are collected in
[`wgpu-traps.md`](../../notes/wgpu-traps.md).

## Tests

```bash
cargo test -p voxel-viewer --test pool
cargo test -p voxel-viewer --test pool --features chunk128
cargo test -p voxel-viewer pool
cargo clippy -p voxel-viewer --all-targets -- -D warnings
```

`tests/pool.rs` covers: aligned ranges inside one page, an over-page request
rejected, free-space reuse before a page is added, adjacent coalescing, the
tail returning to the bump, fragmentation reporting, exact accounting, a double
free rejected, the format descriptor, a p95 mesh fitting one page, a record pool
paging without crossing a page, `DEFAULT_PAGE_BYTES` against the adapter's
`max_buffer_size` and storage-binding limit, an odd index count writing a 4 B
padded tail (GPU), and **a pooled draw matching a per-chunk draw byte for byte**
(GPU). The in-module `mesh_pool::tests` duplicate the allocator invariants so
`cargo test -p voxel-viewer --lib mesh_pool` runs without an adapter.

The GPU tests skip when no adapter is available and honour `VOXEL_REQUIRE_GPU=1`
like `tests/validation.rs`. `cargo test --workspace --features chunk128` (the
`chunk128` CI job) runs the same suite at the 128³ edge.

## Limits

- **Upload staging was still per mesh at close.** M3-03 replaced it with a
  byte-budgeted queue and a staging belt; the frame now copies at most 1 MiB per
  frame. See [`m3-03-upload-queue.md`](m3-03-upload-queue.md).
- **Lighting is still per chunk** (light, probes, origin buffer + bind group).
  The pool is mesh-only; M3-06's instrumentation and M3-11's memory report split
  the rows.
- **The arena only grows.** Pages are never returned to the driver. The ring is
  a fixed 256 chunks, so the committed capacity is bounded in practice.
- **A mesh must fit one page.** 8 MiB covers the 2,115 KiB 128³ p95 by 3.8×; a
  mesh above the page size panics at allocation rather than crossing a page.
- **Free-list work is O(pages × holes).** At 16 pages and a handful of holes it
  is microseconds; it is not an index structure.
- **The iGPU frame time is unchanged** (25.5 ms in M3-01); the pool affects
  upload and memory, not the 256-draw raster cost. M3-10 owns 60 Hz.

## Next

M3-03 landed the upload queue and per-frame byte budget on top of these ranges;
M3-04 is the CPU frustum cull.
