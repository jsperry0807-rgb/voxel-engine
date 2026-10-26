//! Sibling-run allocation and interning over a level band (M1-03, M1-04).
//!
//! A parent's present children form one **run**: `popcount(mask)` contiguous
//! nodes in ascending child order (ADR-011). This module hands out those runs
//! inside a [`Bands`] level band, records the alignment padding each one
//! costs, and hash-conses identical runs.
//!
//! - A **full run** (`mask == 0xFF`) is eight nodes, exactly one 64 B cache
//!   line. Its first node is aligned to a 64 B boundary, so it never straddles
//!   two lines; the allocator inserts at most
//!   [`MAX_RUN_PADDING_BYTES`] (56 B) of padding first.
//! - A **partial run** is packed directly after the previous run with no
//!   padding; nodes are 8 B and every run therefore starts 8 B aligned.
//! - `mask == 0` allocates nothing and is not recorded, so an empty parent
//!   costs no slot (M1-11).
//!
//! An allocator tracks exactly one band. [`RunAllocator::allocate`] only plans
//! the run (it returns the base and advances the cursor); [`RunAllocator::allocate_into`]
//! also materializes the padding slots and the child words in the band. Use
//! one or the other for a given allocation, never both.
//!
//! # Interning
//!
//! [`RunInterner`] owns a [`Bands`] arena and one allocator per level. Its
//! [`RunInterner::intern`] keys a run on `(level, mask, ordered child words)`:
//! the level names the band that stores the children, the mask says which
//! child indices are present, and the child words are compared verbatim. The
//! hash is a lookup accelerator only; a hit is accepted only after the stored
//! run has been compared field-for-field, so two different runs that share a
//! hash never merge. A repeated key returns its existing base, while a new key
//! appends through the allocator and returns the new base.
//!
//! Zero-mask runs allocate nothing (M1-11) and intern to the unused sentinel
//! base `0`. [`RunInterner::deallocate`] retires a run from the index so the
//! key can be interned again; the append-only band keeps the bytes until
//! compaction (M1-12) and the dead bytes stay queryable.
//!
//! # Example
//!
//! ```
//! use voxel_storage::bands::Bands;
//! use voxel_storage::node_word::Node;
//! use voxel_storage::run::{RunAllocator, RunInterner, FULL_MASK};
//!
//! let mut bands = Bands::new();
//! let mut runs = RunAllocator::new();
//!
//! // Three present children: packed at slot zero, no padding.
//! let partial = runs.allocate_into(&mut bands, 2, 0b0000_0111, &[Node::new(0, 2, 0); 3]);
//! assert_eq!(partial.base(), 0);
//! assert_eq!(partial.padding_bytes(), 0);
//!
//! // A full run must start on a 64 B line: five slots of padding are inserted.
//! let full = runs.allocate_into(&mut bands, 2, FULL_MASK, &[Node::new(0, 2, 0); 8]);
//! assert_eq!(full.base(), 8);
//! assert_eq!(full.padding_bytes(), 40);
//! assert!(full.is_cache_line_aligned());
//! assert_eq!(runs.padding_bytes(), 40);
//!
//! // The same content interns to one base; a different mask gets a new run.
//! let mut pool = RunInterner::new();
//! let children = [Node::new(1, 2, 0), Node::new(1, 2, 1)];
//! let first = pool.intern(2, 0b0000_0011, &children);
//! assert_eq!(pool.intern(2, 0b0000_0011, &children), first);
//! assert_ne!(pool.intern(2, 0b0000_1100, &children), first);
//! assert_eq!(pool.live_runs(), 2);
//! ```

use std::hash::Hasher;

use voxel_core::constants::ROOT_LEVEL;

use crate::bands::{BAND_COUNT, Bands};
use crate::hash::{Fx, FxMap};
use crate::node_word::{NODE_INDEX_MAX, Node};

/// Size of one internal node slot in bytes.
pub const NODE_BYTES: u64 = 8;

/// Number of nodes in a full sibling run: one child per bit of the mask.
pub const FULL_RUN_NODES: u64 = 8;

/// Byte alignment of a full sibling run: one 64 B cache line.
pub const FULL_RUN_ALIGN_BYTES: u64 = crate::bands::BAND_ALIGN_BYTES as u64;

/// Largest padding a single full run can insert: seven slots.
pub const MAX_RUN_PADDING_BYTES: u64 = (FULL_RUN_NODES - 1) * NODE_BYTES;

/// The child-presence mask of a full run.
pub const FULL_MASK: u8 = 0xFF;

// The layout constants are a contract with `Node` and the band arena.
const _: () = {
    assert!(NODE_BYTES == core::mem::size_of::<Node>() as u64);
    assert!(FULL_RUN_NODES * NODE_BYTES == FULL_RUN_ALIGN_BYTES);
    assert!(MAX_RUN_PADDING_BYTES == FULL_RUN_ALIGN_BYTES - NODE_BYTES);
    assert!(FULL_RUN_ALIGN_BYTES.is_multiple_of(NODE_BYTES));
};

/// One allocated run: where it starts, which mask it belongs to, and what it
/// cost in padding.
///
/// `base` is a band-local node-slot index, the value stored in the parent's
/// `u48` base field. `padding_bytes` is the gap the allocator inserted before
/// `base` to align a full run; it is zero for partial runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunAllocation {
    base: u64,
    mask: u8,
    nodes: u64,
    padding_bytes: u64,
}

impl RunAllocation {
    /// Index of the run's first node in its band.
    pub const fn base(self) -> u64 {
        self.base
    }

    /// The parent's child-presence mask.
    pub const fn mask(self) -> u8 {
        self.mask
    }

    /// Number of node slots in the run, `popcount(mask)`.
    pub const fn nodes(self) -> u64 {
        self.nodes
    }

    /// Bytes of alignment padding inserted immediately before the run.
    pub const fn padding_bytes(self) -> u64 {
        self.padding_bytes
    }

    /// Padding measured in node slots.
    pub const fn padding_slots(self) -> u64 {
        self.padding_bytes / NODE_BYTES
    }

    /// One past the run's last node slot.
    pub const fn end(self) -> u64 {
        self.base + self.nodes
    }

    /// True when the run has all eight children present.
    pub const fn is_full(self) -> bool {
        self.mask == FULL_MASK
    }

    /// Byte offset of the run from the start of its band.
    pub const fn byte_offset(self) -> u64 {
        self.base * NODE_BYTES
    }

    /// True when the run starts on a 64 B cache-line boundary.
    ///
    /// Bands start on a 64 B boundary (see [`crate::bands::BAND_ALIGN_BYTES`]),
    /// so this is a property of `base` alone. Full runs always satisfy it.
    pub const fn is_cache_line_aligned(self) -> bool {
        self.byte_offset().is_multiple_of(FULL_RUN_ALIGN_BYTES)
    }
}

/// Hands out sibling runs in one level band and records their padding.
///
/// The allocator is a cursor over a band's slot space. [`Self::allocate_into`]
/// keeps that cursor in step with the band by asserting the band length before
/// every allocation; a single allocator must not be shared between bands.
///
/// Padding is queryable both per run ([`Self::padding_bytes_of`],
/// [`Self::runs`]) and in aggregate ([`Self::padding_bytes`]), which is what
/// the layout cost report and the compaction trigger consume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunAllocator {
    next_slot: u64,
    padding_bytes: u64,
    runs: Vec<RunAllocation>,
}

impl RunAllocator {
    /// Creates an allocator for an empty band.
    pub fn new() -> Self {
        Self::default()
    }

    /// Plans a run for `mask` and advances the cursor.
    ///
    /// This does not touch a band: the caller is responsible for materializing
    /// the padding slots and the `popcount(mask)` child nodes. Prefer
    /// [`Self::allocate_into`] when a [`Bands`] arena is available.
    ///
    /// A full run first advances to the next multiple of eight slots, so the
    /// returned `base` is cache-line aligned. A zero mask allocates nothing and
    /// is not added to [`Self::runs`].
    pub fn allocate(&mut self, mask: u8) -> RunAllocation {
        let nodes = u64::from(mask.count_ones());
        if nodes == 0 {
            return RunAllocation {
                base: self.next_slot,
                mask,
                nodes: 0,
                padding_bytes: 0,
            };
        }

        let base = if mask == FULL_MASK {
            self.next_slot.next_multiple_of(FULL_RUN_NODES)
        } else {
            self.next_slot
        };
        let padding_bytes = (base - self.next_slot) * NODE_BYTES;
        let end = base
            .checked_add(nodes)
            .expect("run allocation overflows the node address space");
        assert!(
            end <= NODE_INDEX_MAX + 1,
            "run allocation exceeds the u48 node address space"
        );

        self.next_slot = end;
        self.padding_bytes += padding_bytes;
        let allocation = RunAllocation {
            base,
            mask,
            nodes,
            padding_bytes,
        };
        self.runs.push(allocation);
        allocation
    }

    /// Plans a run and appends its nodes to `level`'s band.
    ///
    /// `children` must contain exactly `popcount(mask)` node words in ascending
    /// child order. Alignment padding is appended first as mask-0 placeholder
    /// words, which are never referenced, so the first real child lands at the
    /// returned [`RunAllocation::base`].
    ///
    /// # Panics
    ///
    /// Panics if `children` does not match `popcount(mask)`, if the allocator's
    /// cursor has drifted from `bands.len(level)`, or if a node's level does
    /// not name the band (the last two are [`Bands::append`] contracts).
    pub fn allocate_into(
        &mut self,
        bands: &mut Bands,
        level: u32,
        mask: u8,
        children: &[Node],
    ) -> RunAllocation {
        assert_eq!(
            children.len() as u64,
            u64::from(mask.count_ones()),
            "run needs popcount(mask) child nodes"
        );
        assert_eq!(
            bands.len(level),
            self.next_slot,
            "run allocator and level {level} band are out of step"
        );

        let allocation = self.allocate(mask);
        // Placeholders are level-correct dead words with a zero mask. They are
        // only there to move the first child to an aligned slot.
        let padding = Node::new(0, level, 0);
        for _ in 0..allocation.padding_slots() {
            bands.append(level, padding);
        }
        for &child in children {
            bands.append(level, child);
        }
        debug_assert_eq!(bands.len(level), allocation.end());
        allocation
    }

    /// The next free slot, i.e. the band length this allocator expects.
    pub const fn next_slot(&self) -> u64 {
        self.next_slot
    }

    /// Number of allocated non-empty runs.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Number of full (`mask == 0xFF`) runs.
    pub fn full_run_count(&self) -> usize {
        self.runs.iter().filter(|run| run.is_full()).count()
    }

    /// Total alignment padding in bytes across all runs.
    pub const fn padding_bytes(&self) -> u64 {
        self.padding_bytes
    }

    /// Total alignment padding in node slots.
    pub const fn padding_slots(&self) -> u64 {
        self.padding_bytes / NODE_BYTES
    }

    /// Total slot space handed out, including padding.
    pub const fn allocated_bytes(&self) -> u64 {
        self.next_slot * NODE_BYTES
    }

    /// All runs in allocation order.
    pub fn runs(&self) -> &[RunAllocation] {
        &self.runs
    }

    /// Padding recorded for run `index`, or `None` past the last run.
    pub fn padding_bytes_of(&self, index: usize) -> Option<u64> {
        self.runs.get(index).map(|run| run.padding_bytes)
    }

    /// True when nothing has been allocated.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// The interned content of one run, kept for deallocation and statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RunMeta {
    hash: u64,
    mask: u8,
    nodes: u64,
    padding_bytes: u64,
}

/// Resolves a level to its per-level slot, panicking outside the chunk's bands.
#[inline]
fn band_slot(level: u32) -> usize {
    assert!(
        level <= ROOT_LEVEL,
        "intern level {level} is outside 0..={ROOT_LEVEL}"
    );
    level as usize
}

/// Hashes the full run key: the storage level, the presence mask and every
/// ordered child word.
///
/// The hash is not the identity: [`RunInterner`] always confirms a candidate
/// with [`RunInterner::matches`] before reusing its base.
#[inline]
fn content_hash(level: u32, mask: u8, children: &[Node]) -> u64 {
    let mut hash = Fx::default();
    hash.write_u64(u64::from(level));
    hash.write_u64(u64::from(mask));
    for child in children {
        hash.write_u64(child.encode());
    }
    hash.finish()
}

/// A hash-consed set of sibling runs over one [`Bands`] arena.
///
/// The interner owns the bands and one [`RunAllocator`] per level. A run is
/// stored in the band named by `level`, exactly like
/// [`RunAllocator::allocate_into`], so every child word must itself carry
/// `level`. The key is `(level, mask, ordered child words)`; equality is
/// checked against the bytes in the band, never against the hash alone.
///
/// Interning is idempotent: the same key returns the same base until that run
/// is deallocated. New content appends a run, so a hash collision costs an
/// extra run but can never return the wrong base.
///
/// A zero mask allocates nothing (M1-11). It returns the unused sentinel base
/// `0`; a zero-mask parent is never emitted, so the value is never stored in a
/// node word.
#[derive(Clone, Debug)]
pub struct RunInterner {
    bands: Bands,
    allocators: [RunAllocator; BAND_COUNT],
    /// Content hash -> candidate bases in insertion order. A bucket can hold
    /// more than one base when distinct keys share a hash.
    index: [FxMap<u64, Vec<u64>>; BAND_COUNT],
    /// Base -> the content that owns it, used by equality checks and
    /// deallocation. Only live runs are present.
    runs: [FxMap<u64, RunMeta>; BAND_COUNT],
    hits: u64,
    misses: u64,
    dead_runs: u64,
    dead_bytes: u64,
}

impl RunInterner {
    /// Creates an empty interner.
    pub fn new() -> Self {
        Self {
            bands: Bands::new(),
            allocators: core::array::from_fn(|_| RunAllocator::new()),
            index: core::array::from_fn(|_| FxMap::default()),
            runs: core::array::from_fn(|_| FxMap::default()),
            hits: 0,
            misses: 0,
            dead_runs: 0,
            dead_bytes: 0,
        }
    }

    /// Interns one sibling run into `level`'s band and returns its base.
    ///
    /// `children` must contain exactly `popcount(mask)` node words in ascending
    /// child order, and each word must carry `level`. The same level, mask and
    /// ordered children always return the same base while the run is live.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`, if `children` does not match
    /// `popcount(mask)`, or if a child's level does not name the band.
    pub fn intern(&mut self, level: u32, mask: u8, children: &[Node]) -> u64 {
        let hash = content_hash(level, mask, children);
        self.intern_with_hash(level, mask, children, hash)
    }

    /// [`Self::intern`] with an explicit content hash.
    ///
    /// Production callers use [`Self::intern`]. Tests use this private form to
    /// force different keys onto one hash and exercise the collision path.
    fn intern_with_hash(&mut self, level: u32, mask: u8, children: &[Node], hash: u64) -> u64 {
        let slot = band_slot(level);
        assert_eq!(
            children.len() as u64,
            u64::from(mask.count_ones()),
            "run needs popcount(mask) child nodes"
        );
        for child in children {
            assert_eq!(
                child.level(),
                level,
                "interned child level does not match its band level {level}"
            );
        }

        // An empty parent is not stored at all (M1-11). The base is a sentinel
        // because no node word will ever point at it.
        if mask == 0 {
            return 0;
        }

        let found = self.index[slot].get(&hash).and_then(|bases| {
            bases
                .iter()
                .copied()
                .find(|&base| self.matches(slot, mask, children, base))
        });
        if let Some(base) = found {
            self.hits += 1;
            return base;
        }

        let allocation =
            self.allocators[slot].allocate_into(&mut self.bands, level, mask, children);
        let base = allocation.base();
        self.runs[slot].insert(
            base,
            RunMeta {
                hash,
                mask,
                nodes: allocation.nodes(),
                padding_bytes: allocation.padding_bytes(),
            },
        );
        self.index[slot].entry(hash).or_default().push(base);
        self.misses += 1;
        base
    }

    /// True when `base` names a live run at `level` with exactly this mask and
    /// this ordered child content.
    ///
    /// The level is the band slot, so comparing masks and child words is
    /// sufficient; the stored words already carry the band's level.
    fn matches(&self, slot: usize, mask: u8, children: &[Node], base: u64) -> bool {
        let Some(meta) = self.runs[slot].get(&base) else {
            return false;
        };
        if meta.mask != mask {
            return false;
        }
        let Ok(start) = usize::try_from(base) else {
            return false;
        };
        let Some(end) = start.checked_add(children.len()) else {
            return false;
        };
        self.bands.band(slot as u32).get(start..end) == Some(children)
    }

    /// Retires the live run starting at `base` in `level`'s band.
    ///
    /// The run is removed from the intern index, so interning the same content
    /// again appends a fresh run instead of returning this base. The bytes are
    /// **not** returned to the arena: bands are append-only, so reclamation is
    /// the offline compaction step (M1-12). Returns `false` when `base` is not
    /// a live interned run.
    ///
    /// # Panics
    ///
    /// Panics if `level > ROOT_LEVEL`.
    ///
    /// # Caller contract
    ///
    /// Retire a run only when no live root or in-flight frame can still reach
    /// it. Reference counting and the epoch list arrive with COW (M1-10) and
    /// the invariant checker (M1-14); this method cannot prove that condition.
    pub fn deallocate(&mut self, level: u32, base: u64) -> bool {
        let slot = band_slot(level);
        let Some(meta) = self.runs[slot].remove(&base) else {
            return false;
        };

        if let Some(bucket) = self.index[slot].get_mut(&meta.hash) {
            if let Some(position) = bucket.iter().position(|&candidate| candidate == base) {
                bucket.swap_remove(position);
            }
            if bucket.is_empty() {
                self.index[slot].remove(&meta.hash);
            }
        }

        self.dead_runs += 1;
        self.dead_bytes += meta.nodes * NODE_BYTES + meta.padding_bytes;
        true
    }

    /// The bands built so far.
    pub const fn bands(&self) -> &Bands {
        &self.bands
    }

    /// Consumes the interner and returns its bands.
    ///
    /// The allocators and intern index are dropped; callers that keep building
    /// a chunk should continue to use the interner.
    pub fn into_bands(self) -> Bands {
        self.bands
    }

    /// Number of live interned runs across all levels.
    pub fn live_runs(&self) -> usize {
        self.runs.iter().map(FxMap::len).sum()
    }

    /// Number of runs ever appended, including retired ones.
    pub fn allocated_runs(&self) -> usize {
        self.allocators.iter().map(RunAllocator::run_count).sum()
    }

    /// Number of intern calls that reused an existing run.
    pub const fn hit_count(&self) -> u64 {
        self.hits
    }

    /// Number of intern calls that appended a new run.
    pub const fn miss_count(&self) -> u64 {
        self.misses
    }

    /// Number of runs retired by [`Self::deallocate`].
    pub const fn dead_run_count(&self) -> u64 {
        self.dead_runs
    }

    /// Bytes owned by retired runs, including their alignment padding.
    ///
    /// These bytes are still physically present because bands are append-only;
    /// they are the input to the compaction trigger (M1-12).
    pub const fn dead_bytes(&self) -> u64 {
        self.dead_bytes
    }

    /// Total alignment padding inserted across all allocated runs.
    pub fn padding_bytes(&self) -> u64 {
        self.allocators
            .iter()
            .map(RunAllocator::padding_bytes)
            .sum()
    }

    /// Total slot bytes handed out across all levels, including padding.
    pub fn allocated_bytes(&self) -> u64 {
        self.allocators
            .iter()
            .map(RunAllocator::allocated_bytes)
            .sum()
    }
}

impl Default for RunInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::BAND_ALIGN_BYTES;

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn children(count: usize, level: u32) -> Vec<Node> {
        (0..count)
            .map(|i| Node::new(1, level, i as u64 + 1))
            .collect()
    }

    fn address_of(bands: &Bands, level: u32, run: RunAllocation) -> usize {
        bands.band(level).as_ptr() as usize + run.byte_offset() as usize
    }

    #[test]
    fn run_align_full_run_starts_on_a_cache_line() {
        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();

        let run = alloc.allocate_into(&mut bands, 2, FULL_MASK, &children(8, 2));

        assert!(run.is_full());
        assert_eq!(run.nodes(), FULL_RUN_NODES);
        assert_eq!(run.base(), 0);
        assert_eq!(run.padding_bytes(), 0);
        assert!(run.is_cache_line_aligned());
        assert_eq!(address_of(&bands, 2, run) % BAND_ALIGN_BYTES, 0);
        assert_eq!(bands.len(2), 8);
    }

    #[test]
    fn run_align_partial_runs_are_packed_without_padding() {
        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();

        let one = alloc.allocate_into(&mut bands, 1, 0b0000_0001, &children(1, 1));
        let two = alloc.allocate_into(&mut bands, 1, 0b0000_0011, &children(2, 1));
        let one_again = alloc.allocate_into(&mut bands, 1, 0b0000_1000, &children(1, 1));

        assert_eq!((one.base(), two.base(), one_again.base()), (0, 1, 3));
        assert_eq!(one.padding_bytes(), 0);
        assert_eq!(two.padding_bytes(), 0);
        assert_eq!(one_again.padding_bytes(), 0);
        assert_eq!(alloc.padding_bytes(), 0);
        assert_eq!(bands.len(1), 4);

        // The children follow the allocation order with no gaps: slot 0 is
        // the first child of the first run, slot 1 the first of the second.
        let bases: Vec<u64> = bands.band(1).iter().map(|node| node.base()).collect();
        assert_eq!(bases, vec![1, 1, 2, 1]);
    }

    #[test]
    fn run_align_full_run_pads_the_gap_before_it() {
        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();

        let partial = alloc.allocate_into(&mut bands, 3, 0b0000_0111, &children(3, 3));
        let full = alloc.allocate_into(&mut bands, 3, FULL_MASK, &children(8, 3));
        let next = alloc.allocate_into(&mut bands, 3, FULL_MASK, &children(8, 3));

        assert_eq!(partial.base(), 0);
        assert_eq!(full.base(), 8);
        assert_eq!(full.padding_bytes(), 5 * NODE_BYTES);
        assert_eq!(full.padding_slots(), 5);
        assert_eq!(next.base(), 16);
        assert_eq!(next.padding_bytes(), 0);

        assert_eq!(alloc.padding_bytes(), 40);
        assert_eq!(alloc.padding_slots(), 5);
        assert_eq!(alloc.allocated_bytes(), 24 * NODE_BYTES);

        // The gap is real band storage, filled with dead mask-0 words.
        assert_eq!(bands.len(3), 24);
        assert!(bands.band(3)[3..8].iter().all(|node| node.mask() == 0));
        assert_eq!(address_of(&bands, 3, full) % BAND_ALIGN_BYTES, 0);
    }

    #[test]
    fn run_align_full_run_alignment_holds_at_every_padding_offset() {
        for lead in 0..FULL_RUN_NODES as usize {
            let mut bands = Bands::new();
            let mut alloc = RunAllocator::new();

            // Give the band `lead` slots of occupancy, then a full run.
            let mask = (1u8 << lead) - 1; // lead present children, 0 for lead 0
            alloc.allocate_into(&mut bands, 5, mask, &children(lead, 5));
            let full = alloc.allocate_into(&mut bands, 5, FULL_MASK, &children(8, 5));

            let expected_base = if lead == 0 { 0 } else { 8 };
            assert_eq!(full.base(), expected_base, "lead {lead}");
            assert_eq!(full.padding_bytes(), (8 - lead as u64) % 8 * NODE_BYTES);
            assert_eq!(address_of(&bands, 5, full) % BAND_ALIGN_BYTES, 0);
        }
    }

    #[test]
    fn run_align_zero_mask_allocates_nothing() {
        let mut alloc = RunAllocator::new();
        let none = alloc.allocate(0);

        assert_eq!(none.base(), 0);
        assert_eq!(none.nodes(), 0);
        assert_eq!(none.padding_bytes(), 0);
        assert!(!none.is_full());
        assert_eq!(alloc.next_slot(), 0);
        assert_eq!(alloc.run_count(), 0);
        assert!(alloc.is_empty());

        let mut bands = Bands::new();
        let none = alloc.allocate_into(&mut bands, 1, 0, &[]);
        assert_eq!(none.base(), 0);
        assert_eq!(bands.len(1), 0);
        assert_eq!(alloc.run_count(), 0);

        // The next real run starts where the empty one would have.
        let run = alloc.allocate_into(&mut bands, 1, 0b1, &children(1, 1));
        assert_eq!(run.base(), 0);
        assert_eq!(bands.len(1), 1);
    }

    /// The M1-03 acceptance test: 1,000 random masks, every full run aligned,
    /// every partial run packed, and the padding accounting exact.
    #[test]
    fn run_align_every_full_run_is_aligned_over_1k_random_masks() {
        const CASES: usize = 1000;
        const LEVEL: u32 = 4;

        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut expected_next = 0u64;
        let mut expected_padding = 0u64;
        let mut expected_runs = 0usize;
        let mut expected_full = 0usize;

        for case in 0..CASES {
            // Keep the sample random but guarantee at least one full run.
            let mask = if case == 0 {
                FULL_MASK
            } else {
                (rng(&mut state) & 0xFF) as u8
            };
            let count = mask.count_ones() as usize;
            let run = alloc.allocate_into(&mut bands, LEVEL, mask, &children(count, LEVEL));

            if count == 0 {
                assert_eq!(run.base(), expected_next, "case {case}");
                assert_eq!(run.nodes(), 0);
                assert_eq!(run.padding_bytes(), 0);
                continue;
            }

            expected_runs += 1;
            let expected_base = if mask == FULL_MASK {
                expected_next.next_multiple_of(FULL_RUN_NODES)
            } else {
                expected_next
            };
            let expected_pad = (expected_base - expected_next) * NODE_BYTES;

            assert_eq!(run.base(), expected_base, "case {case} mask {mask:#010b}");
            assert_eq!(run.nodes(), count as u64);
            assert_eq!(run.padding_bytes(), expected_pad, "case {case}");
            assert!(run.padding_bytes() <= MAX_RUN_PADDING_BYTES);
            assert_eq!(bands.len(LEVEL), run.end(), "case {case}");

            if mask == FULL_MASK {
                expected_full += 1;
                assert_eq!(run.base() % FULL_RUN_NODES, 0, "case {case}");
                assert!(run.is_cache_line_aligned(), "case {case}");
                assert_eq!(
                    address_of(&bands, LEVEL, run) % FULL_RUN_ALIGN_BYTES as usize,
                    0,
                    "case {case}: a full run must start on a cache line"
                );
            }

            expected_next = run.end();
            expected_padding += expected_pad;
            assert_eq!(alloc.next_slot(), expected_next);
        }

        assert!(expected_full >= 1);
        assert!(
            expected_runs < CASES,
            "the sample should include empty masks"
        );
        assert_eq!(alloc.run_count(), expected_runs);
        assert_eq!(alloc.full_run_count(), expected_full);
        assert_eq!(alloc.padding_bytes(), expected_padding);
        assert_eq!(alloc.padding_slots() * NODE_BYTES, expected_padding);
        assert_eq!(alloc.allocated_bytes(), expected_next * NODE_BYTES);

        // Padding is queryable per run and adds up to the aggregate.
        let per_run: u64 = (0..alloc.run_count())
            .map(|index| alloc.padding_bytes_of(index).unwrap())
            .sum();
        assert_eq!(per_run, expected_padding);
        assert_eq!(alloc.padding_bytes_of(alloc.run_count()), None);
    }

    #[test]
    fn run_align_records_padding_per_run() {
        let mut alloc = RunAllocator::new();

        let masks = [0b1u8, 0b111, FULL_MASK, 0b1, FULL_MASK];
        for &mask in &masks {
            alloc.allocate(mask);
        }

        assert_eq!(alloc.run_count(), masks.len());
        assert_eq!(alloc.full_run_count(), 2);
        assert_eq!(alloc.runs()[0].padding_bytes(), 0);
        assert_eq!(alloc.runs()[1].padding_bytes(), 0);
        assert_eq!(alloc.runs()[2].padding_bytes(), 4 * NODE_BYTES); // 4 -> 8
        assert_eq!(alloc.runs()[3].padding_bytes(), 0); // 16 -> 16
        assert_eq!(alloc.runs()[4].padding_bytes(), 7 * NODE_BYTES); // 17 -> 24
        assert_eq!(alloc.padding_bytes_of(2), Some(32));
        assert_eq!(alloc.padding_bytes_of(4), Some(56));
        assert_eq!(alloc.padding_bytes(), 88);
        assert_eq!(alloc.padding_slots(), 11);
        assert_eq!(alloc.next_slot(), 32);
    }

    #[test]
    #[should_panic(expected = "popcount(mask)")]
    fn run_align_allocate_into_checks_the_child_count() {
        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();
        alloc.allocate_into(&mut bands, 1, 0b0000_0011, &children(3, 1));
    }

    #[test]
    #[should_panic(expected = "out of step")]
    fn run_align_allocate_into_checks_the_band_cursor() {
        let mut bands = Bands::new();
        let mut alloc = RunAllocator::new();
        bands.append(1, Node::new(0, 1, 0));
        alloc.allocate_into(&mut bands, 1, 0b0000_0001, &children(1, 1));
    }

    #[test]
    fn run_intern_same_key_twice_returns_the_same_base() {
        let mut pool = RunInterner::new();
        let kids = children(3, 2);

        let first = pool.intern(2, 0b0000_0111, &kids);
        let second = pool.intern(2, 0b0000_0111, &kids.clone());

        assert_eq!(first, second);
        assert_eq!(first, 0);
        assert_eq!(pool.bands().band(2), kids.as_slice());
        assert_eq!(pool.bands().len(2), 3);
        assert_eq!(pool.live_runs(), 1);
        assert_eq!(pool.allocated_runs(), 1);
        assert_eq!(pool.hit_count(), 1);
        assert_eq!(pool.miss_count(), 1);
        assert_eq!(pool.padding_bytes(), 0);
        assert_eq!(pool.allocated_bytes(), 3 * NODE_BYTES);
    }

    /// The mask is part of the key even when two masks have the same popcount
    /// and therefore the same ordered child words.
    #[test]
    fn run_intern_key_includes_mask_and_level() {
        let mut pool = RunInterner::new();
        let level_two = children(2, 2);
        let level_three = children(2, 3);

        let mask_a = pool.intern(2, 0b0000_0011, &level_two);
        let mask_b = pool.intern(2, 0b0000_1100, &level_two);
        let other_level = pool.intern(3, 0b0000_0011, &level_three);

        assert_eq!(mask_a, 0);
        assert_eq!(mask_b, 2);
        assert_ne!(mask_a, mask_b);
        assert_eq!(pool.bands().len(2), 4);
        assert_eq!(pool.bands().len(3), 2);

        // Re-interning each key returns its own base, including the key whose
        // base value is reused by a different level band.
        assert_eq!(pool.intern(2, 0b0000_0011, &level_two), mask_a);
        assert_eq!(pool.intern(2, 0b0000_1100, &level_two), mask_b);
        assert_eq!(pool.intern(3, 0b0000_0011, &level_three), other_level);
        assert_eq!(pool.live_runs(), 3);
    }

    /// Force three distinct keys onto one hash. The equality check must keep
    /// their bases separate and keep each key idempotent.
    #[test]
    fn run_intern_hash_collision_compares_full_content() {
        let mut pool = RunInterner::new();
        let kids = children(2, 2);
        let other_kids = vec![Node::new(1, 2, 90), Node::new(1, 2, 91)];
        let forced_hash = 0xfeed_face_cafe_beef;

        let same_mask = pool.intern_with_hash(2, 0b0000_0011, &kids, forced_hash);
        let other_mask = pool.intern_with_hash(2, 0b0000_1100, &kids, forced_hash);
        let other_children = pool.intern_with_hash(2, 0b0000_0011, &other_kids, forced_hash);

        assert_eq!((same_mask, other_mask, other_children), (0, 2, 4));
        let bucket = pool.index[2]
            .get(&forced_hash)
            .expect("the forced hash has a bucket");
        assert_eq!(bucket.as_slice(), &[same_mask, other_mask, other_children]);

        // A collision costs a run, never a wrong base.
        assert_eq!(
            pool.intern_with_hash(2, 0b0000_0011, &kids, forced_hash),
            same_mask
        );
        assert_eq!(
            pool.intern_with_hash(2, 0b0000_1100, &kids, forced_hash),
            other_mask
        );
        assert_eq!(
            pool.intern_with_hash(2, 0b0000_0011, &other_kids, forced_hash),
            other_children
        );
        assert_eq!(pool.allocated_runs(), 3);
        assert_eq!(pool.live_runs(), 3);
    }

    /// M1-04 acceptance: over 10k runs, distinct content gets a distinct base
    /// (so differing masks cannot silently share a run) and a repeated key
    /// returns its original base without appending.
    #[test]
    fn run_intern_10k_runs_are_content_addressed() {
        use std::collections::HashMap;

        const CASES: usize = 10_000;
        type Key = (u32, u8, Vec<u64>);

        let mut pool = RunInterner::new();
        let mut state = 0x0ddc_0ffe_e15e_beefu64;
        let mut entries: Vec<(u32, u8, Vec<Node>, u64)> = Vec::new();
        let mut by_key: HashMap<Key, u64> = HashMap::new();
        let mut by_address: HashMap<(u32, u64), Key> = HashMap::new();
        let mut expected_hits = 0u64;
        let mut expected_misses = 0u64;

        for case in 0..CASES {
            let reuse = case > 0 && rng(&mut state).is_multiple_of(3);
            let (level, mask, kids, key) = if reuse {
                let (level, mask, kids, _) = &entries[(rng(&mut state) as usize) % entries.len()];
                let key = (
                    *level,
                    *mask,
                    kids.iter().map(|node| node.encode()).collect(),
                );
                (*level, *mask, kids.clone(), key)
            } else {
                let level = (rng(&mut state) % (ROOT_LEVEL as u64 + 1)) as u32;
                // Non-zero masks only; the empty case has its own test.
                let mask = (rng(&mut state) % 255 + 1) as u8;
                let count = mask.count_ones() as usize;
                let kids: Vec<Node> = (0..count)
                    .map(|i| Node::new(1, level, case as u64 * 8 + i as u64 + 1))
                    .collect();
                let key = (level, mask, kids.iter().map(|node| node.encode()).collect());
                (level, mask, kids, key)
            };

            let before = pool.allocated_runs();
            let base = pool.intern(level, mask, &kids);

            if let Some(&existing) = by_key.get(&key) {
                assert_eq!(base, existing, "case {case}: key must be idempotent");
                assert_eq!(
                    pool.allocated_runs(),
                    before,
                    "case {case}: a hit must not append"
                );
                expected_hits += 1;
            } else {
                assert_eq!(
                    pool.allocated_runs(),
                    before + 1,
                    "case {case}: a miss appends exactly one run"
                );
                assert!(
                    by_address.insert((level, base), key.clone()).is_none(),
                    "case {case}: two distinct keys share base {base} in level {level}"
                );
                by_key.insert(key.clone(), base);
                entries.push((level, mask, kids, base));
                expected_misses += 1;
            }
        }

        assert!(expected_hits > 0);
        assert_eq!(expected_hits + expected_misses, CASES as u64);
        assert_eq!(pool.hit_count(), expected_hits);
        assert_eq!(pool.miss_count(), expected_misses);
        assert_eq!(pool.live_runs(), entries.len());
        assert_eq!(pool.allocated_runs(), entries.len());
    }

    /// Different masks with the same ordered children must stay distinct: the
    /// mask is part of the run key, not just the child count.
    #[test]
    fn run_intern_different_masks_never_share_over_10k_runs() {
        use std::collections::HashMap;

        const CASES: usize = 10_000;
        const LEVEL: u32 = 2;
        const MASKS: [u8; 4] = [0b0000_0011, 0b0000_1100, 0b0000_0101, 0b0000_1010];

        let mut pool = RunInterner::new();
        let mut by_address: HashMap<u64, (usize, u8)> = HashMap::new();

        for case in 0..CASES {
            let group = case / MASKS.len();
            let mask = MASKS[case % MASKS.len()];
            let kids = [
                Node::new(1, LEVEL, group as u64 * 2 + 1),
                Node::new(1, LEVEL, group as u64 * 2 + 2),
            ];

            let base = pool.intern(LEVEL, mask, &kids);
            assert!(
                by_address.insert(base, (group, mask)).is_none(),
                "case {case}: mask {mask:#010b} merged with another run of group {group}"
            );
            assert_eq!(pool.live_runs(), case + 1);
        }

        assert_eq!(pool.allocated_runs(), CASES);
        assert_eq!(pool.hit_count(), 0);
    }

    #[test]
    fn run_intern_deallocate_retires_a_run_and_reintern_appends() {
        let mut pool = RunInterner::new();
        let kids = children(3, 4);

        let first = pool.intern(4, 0b0000_0111, &kids);
        assert_eq!((first, pool.live_runs()), (0, 1));

        assert!(pool.deallocate(4, first));
        assert_eq!(pool.live_runs(), 0);
        assert_eq!(pool.dead_run_count(), 1);
        assert_eq!(pool.dead_bytes(), 3 * NODE_BYTES);
        assert!(!pool.deallocate(4, first));
        assert_eq!(pool.dead_run_count(), 1);
        assert!(!pool.deallocate(4, 99));

        // The retired key is no longer interned: the same content appends a new
        // run, while the old bytes stay in the append-only band.
        let second = pool.intern(4, 0b0000_0111, &kids);
        assert_eq!(second, 3);
        assert_eq!(pool.bands().len(4), 6);
        assert_eq!(pool.live_runs(), 1);
        assert_eq!(pool.allocated_runs(), 2);
        assert_eq!(pool.miss_count(), 2);
        assert_eq!(pool.hit_count(), 0);
    }

    #[test]
    fn run_intern_deallocate_includes_padding_in_dead_bytes() {
        let mut pool = RunInterner::new();

        let partial = pool.intern(5, 0b0000_0111, &children(3, 5));
        let full = pool.intern(5, FULL_MASK, &children(8, 5));
        assert_eq!(partial, 0);
        assert_eq!(full, 8);
        assert_eq!(pool.padding_bytes(), 5 * NODE_BYTES);

        assert!(pool.deallocate(5, full));
        assert_eq!(pool.dead_bytes(), 8 * NODE_BYTES + 5 * NODE_BYTES);
        assert_eq!(pool.padding_bytes(), 5 * NODE_BYTES);
        assert_eq!(pool.bands().len(5), 16);
    }

    #[test]
    fn run_intern_empty_mask_allocates_nothing() {
        let mut pool = RunInterner::new();

        assert_eq!(pool.intern(1, 0, &[]), 0);
        assert_eq!(pool.intern(1, 0, &[]), 0);
        assert_eq!(pool.bands().len(1), 0);
        assert_eq!(pool.allocated_runs(), 0);
        assert_eq!(pool.live_runs(), 0);
        assert_eq!(pool.hit_count() + pool.miss_count(), 0);

        // The sentinel is not a stored run; a real run can still use base zero.
        let real = pool.intern(1, 0b0000_0001, &children(1, 1));
        assert_eq!(real, 0);
        assert_eq!(pool.bands().len(1), 1);
    }

    #[test]
    #[should_panic(expected = "popcount(mask)")]
    fn run_intern_checks_the_child_count() {
        let mut pool = RunInterner::new();
        pool.intern(1, 0b0000_0011, &children(3, 1));
    }

    #[test]
    #[should_panic(expected = "does not match its band")]
    fn run_intern_checks_the_child_levels() {
        let mut pool = RunInterner::new();
        pool.intern(2, 0b0000_0001, &children(1, 1));
    }

    #[test]
    #[should_panic(expected = "outside 0..=")]
    fn run_intern_checks_the_level_bound() {
        let mut pool = RunInterner::new();
        pool.intern(ROOT_LEVEL + 1, 0b0000_0001, &[Node::new(1, 0, 0)]);
    }
}
