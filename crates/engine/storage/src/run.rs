//! Sibling-run allocation over a level band (M1-03).
//!
//! A parent's present children form one **run**: `popcount(mask)` contiguous
//! nodes in ascending child order (ADR-011). This module hands out those runs
//! inside a [`Bands`] level band and records the alignment padding each one
//! costs.
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
//! # Example
//!
//! ```
//! use voxel_storage::bands::Bands;
//! use voxel_storage::node_word::Node;
//! use voxel_storage::run::{RunAllocator, FULL_MASK};
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
//! ```

use crate::bands::Bands;
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
}
