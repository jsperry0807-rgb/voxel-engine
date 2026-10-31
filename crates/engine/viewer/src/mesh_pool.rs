//! M3-02 mesh buffer pool: paged arena with byte-range suballocation.
//!
//! The streaming ring loads and evicts whole chunks while the camera moves. With
//! one vertex and one index buffer per chunk, every install asks the driver for
//! two new buffers and every eviction frees them again; the M3-01 trace measured
//! that stage at 6.2 ms mean / 20.9 ms max. The pool replaces those per-chunk
//! allocations with ranges inside a small set of large pages:
//!
//! - [`Arena`] is the pure allocator. It owns page capacities, a bump pointer
//!   and a per-page free list, allocates aligned byte ranges that never cross a
//!   page, coalesces freed neighbours and reports live/free/fragmented bytes.
//!   It performs no GPU work, so its behaviour is testable without an adapter.
//! - [`MeshPool`] binds that arena to `wgpu` buffers. A page is one buffer of
//!   [`DEFAULT_PAGE_BYTES`]; a mesh is a [`MeshAllocation`] holding an `Arc` of
//!   its vertex page, an `Arc` of its index page (often the same page) and the
//!   counts needed to draw. A page is never moved or freed while it holds a
//!   live mesh, so a page index is stable.
//!
//! The format lives in [`MeshFormat`], not in the arena: the allocator deals in
//! bytes and the caller supplies the vertex/index stride. That keeps the same
//! pool usable for the M1.5 naive 16 B path and for a future instanced-record
//! (no index stream) path, as ADR-010 and the M3-00 plan require.
//!
//! # Page size
//!
//! A draw may only bind a range from one page, so a page must be at least as
//! large as the largest single mesh. It must also stay under the adapter's
//! `max_buffer_size` and under the 128 MiB default
//! `max_storage_buffer_binding_size`, because the instanced-record path binds
//! one page as a read-only storage buffer. 8 MiB satisfies both with room to
//! spare: the 64³ ring needs four pages, and even the 128³ projection
//! (805 × 2,115 KiB ≈ 1.66 GiB) pages out at ~208 buffers rather than one
//! impossible 1.66 GiB allocation.
//!
//! # Allocation policy
//!
//! Best fit inside the existing free lists, then the bump tail, then a new
//! page. Best fit keeps a ring rotation (evict one 16-chunk column, load the
//! next) reusing the freed column instead of growing the arena; coalescing
//! keeps a long flight from turning the arena into confetti. Growth is still
//! possible when a new column is uniformly larger than the freed one — that is
//! exactly what [`PoolStats::fragmentation`] and
//! [`PoolStats::page_allocations`] report.
//!
//! # Aliasing safety
//!
//! A range is only reused after [`MeshPool::free`] returns it, and the frame
//! thread frees an evicted chunk in the same pump before the pass that draws.
//! `Queue::write_buffer` is ordered before the next submission, and wgpu-core
//! inserts the buffer-usage barrier against the previous submission's draw, so
//! the GPU never reads a range while it is being rewritten.

use std::sync::Arc;

use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

use voxel_mesh::PackedVertex;

/// Copy alignment `Queue::write_buffer` requires, and the range alignment the
/// arena guarantees. All shipped formats use it; `set_vertex_buffer` also
/// requires a 4-byte offset.
const COPY_ALIGNMENT: u64 = 4;

/// Default page size: 8 MiB.
///
/// The 64³ 256-chunk ring is ≈30 MiB of meshes (four pages) and the 128³ ring
/// is ≈120 MiB (fifteen pages plus one on a flight); the 805-chunk 128³
/// projection pages into ~208 buffers. The measured runs picked this size
/// because it commits the least VRAM for the static ring that M3-11 records:
/// 120 MiB of a 128 MiB budget at 128³ and 32 MiB at 64³, where 16/32 MiB
/// pages commit a full 128 MiB. The measured 64-column flight in
/// `tests/pool.rs` grows one page at 128³ for every page size except 16/32 MiB,
/// so growth is a property of the size mix, not of this constant.
pub const DEFAULT_PAGE_BYTES: u64 = 8 * 1024 * 1024;
/// Smallest page the pool accepts. A page must hold the largest mesh; a tiny
/// page would make every allocation fail rather than just fragment.
pub const MIN_PAGE_BYTES: u64 = 4 * 1024 * 1024;
/// Largest page the pool accepts: the WebGPU default
/// `max_storage_buffer_binding_size`, which the instanced-record path binds.
pub const MAX_PAGE_BYTES: u64 = 128 * 1024 * 1024;

/// The vertex/index layout a pool serves.
///
/// The arena allocates bytes; the format turns record counts into byte lengths
/// and therefore is the only place that knows the mesh encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshFormat {
    vertex_stride: u32,
    index_stride: u32,
}

impl MeshFormat {
    /// The production 8 B packed vertex plus a `u16` index stream (M3-00).
    pub const PACKED_8B: Self = Self {
        vertex_stride: PackedVertex::SIZE as u32,
        index_stride: 2,
    };
    /// The M1.5 naive 16 B vertex plus a `u16` index stream.
    pub const NAIVE_16B: Self = Self {
        vertex_stride: 16,
        index_stride: 2,
    };
    /// A bare record stream with no index buffer (storage-pull draws).
    pub const RECORD_8B: Self = Self {
        vertex_stride: 8,
        index_stride: 0,
    };

    /// Builds a format, panicking on a stride the pool cannot serve.
    ///
    /// `index_stride` is `0` for a format without an index stream, otherwise
    /// `2` or `4`.
    pub fn new(vertex_stride: u32, index_stride: u32) -> Self {
        assert!(vertex_stride > 0, "a vertex stride is at least one byte");
        assert!(
            index_stride == 0 || index_stride == 2 || index_stride == 4,
            "index stride must be 0, 2 or 4, got {index_stride}"
        );
        Self {
            vertex_stride,
            index_stride,
        }
    }

    /// Bytes per vertex record.
    pub const fn vertex_stride(self) -> u32 {
        self.vertex_stride
    }

    /// Bytes per index, or `0` when the format has no index stream.
    pub const fn index_stride(self) -> u32 {
        self.index_stride
    }

    /// Bytes needed for `vertices` records.
    pub const fn vertex_bytes(self, vertices: u32) -> u64 {
        vertices as u64 * self.vertex_stride as u64
    }

    /// Bytes needed for `indices` records, or `0` without an index stream.
    pub const fn index_bytes(self, indices: u32) -> u64 {
        indices as u64 * self.index_stride as u64
    }

    /// Bytes per quad: four vertices and six indices.
    pub const fn quad_bytes(self) -> u64 {
        4 * self.vertex_stride as u64 + 6 * self.index_stride as u64
    }
}

const _: () = assert!(MeshFormat::PACKED_8B.vertex_stride == PackedVertex::SIZE as u32);
const _: () = assert!(MeshFormat::PACKED_8B.quad_bytes() == 44);

/// A byte range inside one arena page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// Page index; stable for the pool's lifetime.
    pub page: u32,
    /// Byte offset inside the page; a multiple of [`COPY_ALIGNMENT`].
    pub offset: u64,
    /// Byte length, rounded up to [`COPY_ALIGNMENT`].
    pub len: u64,
}

impl Range {
    /// One past the last byte of the range.
    pub const fn end(self) -> u64 {
        self.offset + self.len
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block {
    offset: u64,
    len: u64,
}

impl Block {
    const fn end(self) -> u64 {
        self.offset + self.len
    }
}

/// One allocator page: a bump pointer plus sorted, coalesced free blocks.
#[derive(Debug, Default)]
struct Page {
    bump: u64,
    free: Vec<Block>,
}

impl Page {
    fn used(&self) -> u64 {
        self.bump - self.free.iter().map(|block| block.len).sum::<u64>()
    }

    /// The smallest free block that fits `len` at `align`, as `(start, index)`.
    fn best_fit(&self, len: u64, align: u64) -> Option<(u64, usize)> {
        let mut best: Option<(u64, usize, u64)> = None;
        for (index, block) in self.free.iter().enumerate() {
            let start = align_up(block.offset, align);
            if start + len > block.end() {
                continue;
            }
            let waste = block.len - len;
            if best.is_none_or(|(best_waste, _, _)| waste < best_waste) {
                best = Some((waste, index, start));
            }
        }
        best.map(|(_, index, start)| (start, index))
    }
}

/// The pure allocator behind [`MeshPool`].
///
/// Ranges never cross a page, are aligned to [`COPY_ALIGNMENT`] and are reused
/// after [`Self::free`], which coalesces both neighbours and returns a block
/// that reaches the bump pointer to the bump.
#[derive(Debug)]
pub struct Arena {
    page_bytes: u64,
    pages: Vec<Page>,
    allocations: u64,
    frees: u64,
}

impl Arena {
    /// Creates an empty arena whose pages are `page_bytes` long.
    ///
    /// # Panics
    ///
    /// Panics if `page_bytes` is outside [`MIN_PAGE_BYTES`]..=[`MAX_PAGE_BYTES`]
    /// or is not a multiple of [`COPY_ALIGNMENT`].
    pub fn new(page_bytes: u64) -> Self {
        assert!(
            (MIN_PAGE_BYTES..=MAX_PAGE_BYTES).contains(&page_bytes),
            "a pool page is {MIN_PAGE_BYTES}..={MAX_PAGE_BYTES} B, got {page_bytes}"
        );
        assert!(
            page_bytes.is_multiple_of(COPY_ALIGNMENT),
            "a pool page is a multiple of {COPY_ALIGNMENT} B"
        );
        Self {
            page_bytes,
            pages: Vec::new(),
            allocations: 0,
            frees: 0,
        }
    }

    /// Bytes per page.
    pub const fn page_bytes(&self) -> u64 {
        self.page_bytes
    }

    /// Pages created so far. Pages are never removed.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Reserves `len` bytes at `align` and returns their range.
    ///
    /// The returned length is rounded up to [`COPY_ALIGNMENT`]. A request
    /// larger than one page panics: a range may not cross a page.
    ///
    /// # Panics
    ///
    /// Panics on a zero-length or over-page request, or an alignment that is
    /// not 1, 2 or 4.
    pub fn allocate(&mut self, len: u64, align: u64) -> Range {
        assert!(len > 0, "a mesh allocation is at least one byte");
        assert!(
            align == 1 || align == 2 || align == COPY_ALIGNMENT,
            "range alignment must be 1, 2 or {COPY_ALIGNMENT}, got {align}"
        );
        let len = align_up(len, COPY_ALIGNMENT);
        assert!(
            len <= self.page_bytes,
            "a {len} B range does not fit a {} B page",
            self.page_bytes
        );

        // Best fit across every page's free list.
        let mut choice: Option<(u64, usize, usize, u64)> = None;
        for (page_index, page) in self.pages.iter().enumerate() {
            let Some((start, block_index)) = page.best_fit(len, align) else {
                continue;
            };
            let waste = page.free[block_index].len - len;
            if choice.is_none_or(|(best_waste, _, _, _)| waste < best_waste) {
                choice = Some((waste, page_index, block_index, start));
            }
        }
        if let Some((_, page_index, block_index, start)) = choice {
            let page = &mut self.pages[page_index];
            let block = page.free.remove(block_index);
            debug_assert_eq!(start, block.offset, "a page offset stays aligned");
            let tail = start + len;
            if tail < block.end() {
                page.free.insert(
                    block_index,
                    Block {
                        offset: tail,
                        len: block.end() - tail,
                    },
                );
            }
            self.allocations += 1;
            return Range {
                page: page_index as u32,
                offset: start,
                len,
            };
        }

        // Then the bump tail of the page currently growing.
        for (page_index, page) in self.pages.iter_mut().enumerate() {
            let start = align_up(page.bump, align);
            if start + len <= self.page_bytes {
                page.bump = start + len;
                self.allocations += 1;
                return Range {
                    page: page_index as u32,
                    offset: start,
                    len,
                };
            }
        }

        // Then a new page.
        let page_index = self.pages.len();
        self.pages.push(Page::default());
        self.pages[page_index].bump = len;
        self.allocations += 1;
        Range {
            page: page_index as u32,
            offset: 0,
            len,
        }
    }

    /// Returns `range` to its page, merging contiguous neighbours.
    ///
    /// # Panics
    ///
    /// Panics on a page index outside the arena or a range past the page's bump
    /// pointer (a double free or a range from another arena).
    pub fn free(&mut self, range: Range) {
        let page_bytes = self.page_bytes;
        let page = self
            .pages
            .get_mut(range.page as usize)
            .unwrap_or_else(|| panic!("page {} does not exist", range.page));
        assert!(
            range.end() <= page.bump && range.end() <= page_bytes,
            "range {range:?} is not below the page bump"
        );

        let (mut start, mut end) = (range.offset, range.end());
        let mut index = page.free.partition_point(|block| block.offset < start);

        // Merge every block that overlaps or touches the released range.
        while index < page.free.len() && page.free[index].offset <= end {
            let block = page.free.remove(index);
            start = start.min(block.offset);
            end = end.max(block.end());
        }
        // Merge the block immediately before it, if contiguous.
        if index > 0 {
            let previous = page.free[index - 1];
            if previous.end() >= start {
                start = previous.offset;
                end = end.max(previous.end());
                page.free.remove(index - 1);
                index -= 1;
            }
        }
        page.free.insert(
            index,
            Block {
                offset: start,
                len: end - start,
            },
        );

        // Give a block that reaches the bump pointer back to the bump.
        if let Some(block) = page.free.pop_if(|block| block.end() == page.bump) {
            page.bump = block.offset;
        }
        self.frees += 1;
    }

    /// Live, free and fragmented bytes across every page.
    ///
    /// Free bytes are split into a page's unused tail (`slack_bytes`, one per
    /// page, usable by the bump pointer) and holes left by freed ranges
    /// (`hole_bytes`). Fragmentation is measured over the holes only: page
    /// slack is a page-granularity cost, not trapped space.
    pub fn stats(&self) -> ArenaStats {
        let mut used = 0u64;
        let mut slack = 0u64;
        let mut holes = 0u64;
        let mut largest_hole = 0u64;
        for page in &self.pages {
            used += page.used();
            let tail = self.page_bytes - page.bump;
            slack += tail;
            for block in &page.free {
                holes += block.len;
                largest_hole = largest_hole.max(block.len);
            }
        }
        let capacity = self.page_bytes * self.pages.len() as u64;
        let free = slack + holes;
        debug_assert_eq!(used + free, capacity, "arena accounting is exact");
        ArenaStats {
            page_bytes: self.page_bytes,
            pages: self.pages.len(),
            capacity_bytes: capacity,
            used_bytes: used,
            free_bytes: free,
            slack_bytes: slack,
            hole_bytes: holes,
            largest_hole_bytes: largest_hole,
            largest_free_bytes: largest_hole.max(
                self.pages
                    .iter()
                    .map(|page| self.page_bytes - page.bump)
                    .max()
                    .unwrap_or(0),
            ),
            fragmentation: if holes == 0 {
                0.0
            } else {
                1.0 - largest_hole as f64 / holes as f64
            },
            allocations: self.allocations,
            frees: self.frees,
        }
    }
}

/// A snapshot of one arena; see [`Arena::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArenaStats {
    /// Bytes per page.
    pub page_bytes: u64,
    /// Pages created.
    pub pages: usize,
    /// `pages * page_bytes`: the VRAM the pool has committed.
    pub capacity_bytes: u64,
    /// Bytes inside live allocations.
    pub used_bytes: u64,
    /// Bytes not inside a live allocation: `slack_bytes + hole_bytes`.
    pub free_bytes: u64,
    /// Unused page tails. One allocation can use a tail through the bump
    /// pointer; this is page granularity, not trapped space.
    pub slack_bytes: u64,
    /// Free blocks created by frees inside pages.
    pub hole_bytes: u64,
    /// The largest such hole.
    pub largest_hole_bytes: u64,
    /// The largest single range an allocation could use: a hole or a page tail.
    pub largest_free_bytes: u64,
    /// External fragmentation of the holes, `1 - largest_hole / hole_bytes`.
    /// `0` means every freed byte is contiguous with every other freed byte;
    /// `k` equal holes read as `1 - 1/k`.
    pub fragmentation: f64,
    /// Successful allocations.
    pub allocations: u64,
    /// Successful frees.
    pub frees: u64,
}

/// The vertex and index page ranges backing one mesh.
#[derive(Clone)]
pub struct MeshSlice {
    /// The page buffer.
    pub buffer: Arc<Buffer>,
    /// Byte offset of the range inside `buffer`.
    pub offset: u64,
    /// Byte length of the range (padded to [`COPY_ALIGNMENT`]).
    pub len: u64,
}

impl MeshSlice {
    /// The range as a bindable `wgpu` slice.
    pub fn slice(&self) -> wgpu::BufferSlice<'_> {
        self.buffer.slice(self.offset..self.offset + self.len)
    }
}

/// One mesh reserved inside a [`MeshPool`].
///
/// Dropping the value does not return the range; call [`MeshPool::free`]. The
/// `Arc` handles keep the page buffers alive even if the pool is dropped first.
#[derive(Clone)]
pub struct MeshAllocation {
    /// Vertex records.
    pub vertices: MeshSlice,
    /// Index records, or `None` for a format without an index stream.
    ///
    /// The index stream shares the mesh's single contiguous range and starts
    /// at the first 4 B boundary after the vertices, so one mesh is one
    /// reusable block: two separate ranges per chunk double the number of
    /// free-list fragments the ring leaves behind on every rotation.
    pub indices: Option<MeshSlice>,
    vertex_count: u32,
    index_count: u32,
    format: MeshFormat,
    reservation: Range,
}

impl MeshAllocation {
    /// Vertex records in the allocation.
    pub const fn vertex_count(&self) -> u32 {
        self.vertex_count
    }

    /// Index records in the allocation.
    pub const fn index_count(&self) -> u32 {
        self.index_count
    }

    /// The format the counts are interpreted with.
    pub const fn format(&self) -> MeshFormat {
        self.format
    }

    /// Bytes of vertex data.
    pub const fn vertex_bytes(&self) -> u64 {
        self.format.vertex_bytes(self.vertex_count)
    }

    /// Bytes of index data.
    pub const fn index_bytes(&self) -> u64 {
        self.format.index_bytes(self.index_count)
    }

    /// Vertex plus index bytes; 44/quad for [`MeshFormat::PACKED_8B`].
    pub const fn byte_len(&self) -> u64 {
        self.vertex_bytes() + self.index_bytes()
    }

    /// Writes the mesh into its reserved ranges.
    ///
    /// `vertices` and `indices` are raw record bytes; the index slice is empty
    /// for a format without an index stream. The write is padded to
    /// [`COPY_ALIGNMENT`] because `Queue::write_buffer` requires a four-byte
    /// size, and wgpu orders it before the next submission.
    ///
    /// # Panics
    ///
    /// Panics if the byte lengths do not match the allocation.
    pub fn write(&self, queue: &Queue, vertices: &[u8], indices: &[u8]) {
        assert_eq!(
            vertices.len() as u64,
            self.vertex_bytes(),
            "vertex bytes do not match the allocation"
        );
        assert_eq!(
            indices.len() as u64,
            self.index_bytes(),
            "index bytes do not match the allocation"
        );
        write_padded(queue, &self.vertices, vertices);
        if let Some(slice) = &self.indices {
            write_padded(queue, slice, indices);
        }
    }
}

/// Writes `bytes` into `slice`, padding the tail to [`COPY_ALIGNMENT`].
///
/// The range itself is already padded, so the tail write stays in bounds.
fn write_padded(queue: &Queue, slice: &MeshSlice, bytes: &[u8]) {
    let aligned = bytes.len() & !(COPY_ALIGNMENT as usize - 1);
    if aligned > 0 {
        queue.write_buffer(&slice.buffer, slice.offset, &bytes[..aligned]);
    }
    if aligned < bytes.len() {
        let mut tail = [0u8; COPY_ALIGNMENT as usize];
        let rest = &bytes[aligned..];
        tail[..rest.len()].copy_from_slice(rest);
        queue.write_buffer(&slice.buffer, slice.offset + aligned as u64, &tail);
    }
}

/// A paged arena bound to `wgpu` vertex/index buffers.
pub struct MeshPool {
    device: Device,
    format: MeshFormat,
    arena: Arena,
    pages: Vec<Arc<Buffer>>,
    usage: BufferUsages,
    page_creation: std::time::Duration,
}

impl MeshPool {
    /// Creates an empty pool for `format` with `page_bytes` pages.
    ///
    /// Pages are `VERTEX | INDEX | COPY_DST`, the usage the packed and naive
    /// vertex-stream draws need.
    ///
    /// # Panics
    ///
    /// Panics if `page_bytes` is outside [`MIN_PAGE_BYTES`]..=[`MAX_PAGE_BYTES`]
    /// or exceeds the adapter's `max_buffer_size`.
    pub fn new(device: Device, format: MeshFormat, page_bytes: u64) -> Self {
        Self::with_usage(
            device,
            format,
            page_bytes,
            BufferUsages::VERTEX | BufferUsages::INDEX | BufferUsages::COPY_DST,
        )
    }

    /// Creates a pool whose pages carry `usage`.
    ///
    /// The record path binds a page as a read-only storage buffer, so it passes
    /// `STORAGE | COPY_DST`; tests add `COPY_SRC` to read a range back. `usage`
    /// must include [`BufferUsages::COPY_DST`] because a mesh is written through
    /// the queue.
    ///
    /// # Panics
    ///
    /// Same as [`Self::new`], plus a missing [`BufferUsages::COPY_DST`].
    pub fn with_usage(
        device: Device,
        format: MeshFormat,
        page_bytes: u64,
        usage: BufferUsages,
    ) -> Self {
        let arena = Arena::new(page_bytes);
        let max_buffer = device.limits().max_buffer_size;
        assert!(
            page_bytes <= max_buffer,
            "a {page_bytes} B page exceeds the adapter's {max_buffer} B max_buffer_size"
        );
        assert!(
            usage.contains(BufferUsages::COPY_DST),
            "a pool page must accept queue writes"
        );
        Self {
            device,
            format,
            arena,
            pages: Vec::new(),
            usage,
            page_creation: std::time::Duration::ZERO,
        }
    }

    /// The format the pool allocates for.
    pub const fn format(&self) -> MeshFormat {
        self.format
    }

    /// Bytes per page.
    pub const fn page_bytes(&self) -> u64 {
        self.arena.page_bytes()
    }

    /// Pages created.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Reserves one mesh and returns its allocation.
    ///
    /// `vertex_count` must be non-zero. A format with an index stream requires
    /// `index_count > 0`; a format without one requires `index_count == 0`.
    ///
    /// # Panics
    ///
    /// Panics on an empty vertex stream, an index count the format forbids, or
    /// a mesh larger than one page (a draw may not cross pages).
    pub fn allocate(&mut self, vertex_count: u32, index_count: u32) -> MeshAllocation {
        assert!(
            vertex_count > 0,
            "an empty mesh allocates nothing; the caller keeps None"
        );
        if self.format.index_stride == 0 {
            assert_eq!(
                index_count, 0,
                "a format without an index stream cannot take indices"
            );
        } else {
            assert!(index_count > 0, "an indexed mesh needs at least one index");
        }

        // One contiguous range per mesh: vertices, then the index stream at
        // the next 4 B boundary. A draw still never crosses a page.
        let vertex_bytes = self.format.vertex_bytes(vertex_count);
        let index_bytes = self.format.index_bytes(index_count);
        let reservation = self
            .arena
            .allocate(vertex_bytes + index_bytes, COPY_ALIGNMENT);
        self.ensure_pages();

        let buffer = Arc::clone(&self.pages[reservation.page as usize]);
        let vertices = MeshSlice {
            buffer: Arc::clone(&buffer),
            offset: reservation.offset,
            len: vertex_bytes,
        };
        let indices = (index_bytes > 0).then(|| MeshSlice {
            buffer,
            offset: reservation.offset + vertex_bytes,
            len: index_bytes,
        });
        MeshAllocation {
            vertices,
            indices,
            vertex_count,
            index_count,
            format: self.format,
            reservation,
        }
    }

    /// Returns a mesh to the pool. The allocation is consumed; a stale handle
    /// cannot be drawn or freed twice.
    pub fn free(&mut self, allocation: MeshAllocation) {
        self.arena.free(allocation.reservation);
    }

    /// Live, free and fragmented bytes across every page.
    pub fn stats(&self) -> ArenaStats {
        self.arena.stats()
    }

    /// Total time spent in page creation, which happens off the frame thread
    /// on the streaming uploader.
    pub fn page_creation_time(&self) -> std::time::Duration {
        self.page_creation
    }

    /// Creates buffers for any page the arena has added.
    fn ensure_pages(&mut self) {
        while self.pages.len() < self.arena.pages() {
            let index = self.pages.len();
            let started = std::time::Instant::now();
            let buffer = self.device.create_buffer(&BufferDescriptor {
                label: Some(&format!("mesh pool page {index}")),
                size: self.arena.page_bytes(),
                usage: self.usage,
                mapped_at_creation: false,
            });
            let elapsed = started.elapsed();
            self.page_creation += elapsed;
            log::debug!(
                "pool: created page {index} ({:.0} MiB) in {:.2} ms",
                self.arena.page_bytes() as f64 / (1024.0 * 1024.0),
                elapsed.as_secs_f64() * 1000.0
            );
            self.pages.push(Arc::new(buffer));
        }
    }
}

/// Rounds `value` up to `align`, which must be a power of two.
fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arena() -> Arena {
        Arena::new(MIN_PAGE_BYTES)
    }

    fn overlap(a: Range, b: Range) -> bool {
        a.page == b.page && a.offset < b.end() && b.offset < a.end()
    }

    #[test]
    fn allocation_is_aligned_and_inside_one_page() {
        let mut arena = arena();
        let mut ranges = Vec::new();
        for _ in 0..64 {
            let range = arena.allocate(1_000, COPY_ALIGNMENT);
            assert_eq!(range.offset % COPY_ALIGNMENT, 0);
            assert_eq!(range.len % COPY_ALIGNMENT, 0);
            assert!(range.end() <= arena.page_bytes());
            for other in &ranges {
                assert!(!overlap(range, *other), "{range:?} overlaps {other:?}");
            }
            ranges.push(range);
        }
        assert_eq!(arena.pages(), 1);
    }

    #[test]
    fn an_over_page_request_is_rejected() {
        let mut arena = arena();
        let oversized = arena.page_bytes() + COPY_ALIGNMENT;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            arena.allocate(oversized, COPY_ALIGNMENT)
        }));
        assert!(result.is_err(), "a range may not cross a page");
    }

    #[test]
    fn free_space_is_reused_before_a_page_is_added() {
        let mut arena = arena();
        let first = arena.allocate(8_192, COPY_ALIGNMENT);
        let second = arena.allocate(8_192, COPY_ALIGNMENT);
        assert_ne!(second.offset, first.offset);
        arena.free(first);
        let reused = arena.allocate(8_192, COPY_ALIGNMENT);
        assert_eq!(reused, first, "best fit reuses the freed block");
        assert_eq!(arena.pages(), 1);
    }

    #[test]
    fn adjacent_frees_coalesce() {
        let mut arena = arena();
        let first = arena.allocate(4_096, COPY_ALIGNMENT);
        let second = arena.allocate(4_096, COPY_ALIGNMENT);
        arena.free(first);
        arena.free(second);
        let stats = arena.stats();
        assert_eq!(stats.used_bytes, 0);
        assert_eq!(stats.free_bytes, stats.capacity_bytes);
        assert_eq!(stats.largest_free_bytes, stats.capacity_bytes);
        assert_eq!(stats.fragmentation, 0.0);

        // The coalesced block must satisfy one allocation the pieces could not.
        let merged = arena.allocate(8_192, COPY_ALIGNMENT);
        assert_eq!(merged.page, first.page);
        assert_eq!(merged.offset, first.offset);
    }

    #[test]
    fn a_tail_free_returns_the_bump_pointer() {
        let mut arena = arena();
        let first = arena.allocate(4_096, COPY_ALIGNMENT);
        let second = arena.allocate(8_192, COPY_ALIGNMENT);
        arena.free(second);
        let third = arena.allocate(8_192, COPY_ALIGNMENT);
        assert_eq!(third.offset, second.offset, "the tail is reused in place");
        assert_eq!(third.page, second.page);
        arena.free(first);

        // `third` still pins the bump, so the released head is a hole and the
        // tail above `third` is slack.
        let partial = arena.stats();
        assert_eq!(partial.free_bytes, partial.capacity_bytes - third.len);
        assert_eq!(partial.hole_bytes, 4_096);
        assert_eq!(partial.slack_bytes, partial.capacity_bytes - third.end());
        assert_eq!(partial.fragmentation, 0.0, "one hole is not fragmented");

        arena.free(third);
        let stats = arena.stats();
        assert_eq!(stats.free_bytes, stats.capacity_bytes);
        assert_eq!(stats.fragmentation, 0.0);
    }

    #[test]
    fn fragmentation_is_reported() {
        let mut arena = arena();
        let blocks: Vec<Range> = (0..4)
            .map(|_| arena.allocate(4_096, COPY_ALIGNMENT))
            .collect();
        arena.free(blocks[0]);
        arena.free(blocks[2]);
        let stats = arena.stats();
        assert_eq!(stats.used_bytes, 8_192);
        assert!(stats.free_bytes > 8_192);
        assert!(stats.fragmentation > 0.0, "{stats:?}");
        assert!(stats.fragmentation < 1.0, "{stats:?}");
    }

    #[test]
    fn double_free_is_rejected() {
        let mut arena = arena();
        let range = arena.allocate(1_024, COPY_ALIGNMENT);
        arena.free(range);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| arena.free(range)));
        assert!(result.is_err(), "a second free must panic");
    }

    #[test]
    fn stats_account_for_every_byte() {
        let mut arena = arena();
        let ranges: Vec<Range> = (0..10)
            .map(|index| arena.allocate(1_000 + index * 97, COPY_ALIGNMENT))
            .collect();
        for range in ranges.iter().step_by(2) {
            arena.free(*range);
        }
        let stats = arena.stats();
        assert_eq!(stats.used_bytes + stats.free_bytes, stats.capacity_bytes);
        assert_eq!(stats.allocations, 10);
        assert_eq!(stats.frees, 5);
    }

    #[test]
    fn formats_describe_the_shipped_streams() {
        assert_eq!(MeshFormat::PACKED_8B.vertex_stride(), 8);
        assert_eq!(MeshFormat::PACKED_8B.index_stride(), 2);
        assert_eq!(MeshFormat::PACKED_8B.quad_bytes(), 44);
        assert_eq!(MeshFormat::NAIVE_16B.quad_bytes(), 76);
        assert_eq!(MeshFormat::RECORD_8B.quad_bytes(), 32);
        assert_eq!(MeshFormat::RECORD_8B.index_bytes(12), 0);
        assert_eq!(MeshFormat::PACKED_8B.vertex_bytes(4), 32);
        assert_eq!(MeshFormat::PACKED_8B.index_bytes(6), 12);
    }
}
