//! Leaf runs: compacted `u16` material payloads (M1-06).
//!
//! A level-1 internal node covers eight voxels. Its present children are
//! material IDs, not further nodes, and they live in the leaf arena rather than
//! in a level band. This module groups those IDs into **leaf runs**: the
//! materials of one level-1 parent, in ascending child order, packed with no
//! padding and at most eight per run (16 B). A full run is exactly
//! [`LEAF_RUN_BYTES`].
//!
//! Material [`AIR`] is `0` and means **absent**. It is never stored in a run;
//! the parent's mask records which child indices exist, and child `c` is read
//! at `base + popcount(mask & ((1 << c) - 1))`. Reading an absent child — a
//! clear mask bit, a non-level-1 parent, or a slot past the arena tail — yields
//! air (`0`), never an error.
//!
//! # Sharing
//!
//! [`LeafInterner`] hash-conses runs on their ordered material IDs. The mask is
//! deliberately **not** part of the key: it only decides which positions are
//! visible, so two level-1 parents with the same present materials share one
//! run even when their masks differ. Equality is checked against the stored
//! materials after a hash hit, so a collision costs an extra run but can never
//! return the wrong base.
//!
//! Zero materials allocate nothing and intern to the sentinel base `0`, which
//! no node word references because an all-air parent has mask `0` (M1-11).
//!
//! # Example
//!
//! ```
//! use voxel_storage::leaf::{
//!     AIR, LEAF_RUN_BYTES, LeafInterner, fetch_leaf,
//! };
//! use voxel_storage::{Bands, Node};
//!
//! let mut bands = Bands::new();
//! let mut leaves = LeafInterner::new();
//!
//! // Two present materials; the third child is air and is not stored.
//! let (mask, base) = leaves.intern_dense(&mut bands, &[3, 0, 4, 0, 0, 0, 0, 0]);
//! assert_eq!(mask, 0b0000_0101);
//! assert_eq!(bands.leaves(), &[3, 4]);
//! assert_eq!(LEAF_RUN_BYTES, 16);
//!
//! let parent = Node::new(mask, 1, base);
//! assert_eq!(fetch_leaf(&bands, parent, 0), 3);
//! assert_eq!(fetch_leaf(&bands, parent, 1), AIR); // absent
//! assert_eq!(fetch_leaf(&bands, parent, 2), 4);
//!
//! // A second parent with the same present materials shares the run.
//! let (other_mask, other_base) =
//!     leaves.intern_dense(&mut bands, &[0, 3, 0, 0, 4, 0, 0, 0]);
//! assert_eq!(other_base, base);
//! assert_ne!(other_mask, mask);
//! assert_eq!(bands.leaf_len(), 2);
//! ```
//!
//! [`AIR`]: AIR

use std::hash::Hasher;

use crate::bands::Bands;
use crate::hash::{Fx, FxMap};
use crate::node_word::Node;

/// Number of material IDs in a full leaf run: one per level-1 child.
pub const LEAF_RUN_MATERIALS: usize = 8;

/// Size of one leaf material ID in bytes.
pub const LEAF_MATERIAL_BYTES: u64 = 2;

/// Size of a full leaf run in bytes: eight `u16` material IDs.
pub const LEAF_RUN_BYTES: u64 = LEAF_RUN_MATERIALS as u64 * LEAF_MATERIAL_BYTES;

/// The absent material: air is always id `0` and is never stored in a run.
pub const AIR: u16 = 0;

// The run width is a contract with the leaf arena payload and ADR-011.
const _: () = {
    assert!(LEAF_MATERIAL_BYTES == core::mem::size_of::<u16>() as u64);
    assert!(LEAF_RUN_BYTES == 16);
    assert!(LEAF_RUN_MATERIALS == 8);
};

/// Compact slot of child `c` within a level-1 parent's leaf run.
///
/// Identical to the internal-node rule: present children are stored in
/// ascending child order, so the slot is
/// `popcount(mask & ((1 << c) - 1))`. Returns `None` when bit `c` is clear or
/// `c` is not a child index.
#[inline]
pub const fn leaf_slot(mask: u8, c: u32) -> Option<u32> {
    crate::run::child_slot(mask, c)
}

/// Absolute leaf-arena slot of child `c`: `base + leaf_slot(mask, c)`.
///
/// Returns `None` for an absent or out-of-range child, or when the slot would
/// leave the `u48` address space.
#[inline]
pub const fn leaf_index(base: u64, mask: u8, c: u32) -> Option<u64> {
    crate::run::child_index(base, mask, c)
}

/// Reads child `c` of a level-1 `parent` as a material ID.
///
/// Returns [`AIR`] (`0`) when the child is absent, when `parent` is not a
/// level-1 node, or when the computed slot is past the leaf arena tail. Air is
/// the empty voxel, so a missing leaf is a value, not an error.
#[inline]
pub fn fetch_leaf(bands: &Bands, parent: Node, c: u32) -> u16 {
    if parent.level() != 1 {
        return AIR;
    }
    match parent.child_index(c).and_then(|index| bands.leaf(index)) {
        Some(material) => material,
        None => AIR,
    }
}

/// The interned content of one leaf run, kept for equality and statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LeafMeta {
    hash: u64,
    materials: u8,
}

impl LeafMeta {
    /// Bytes occupied by this run in the leaf arena.
    const fn bytes(self) -> u64 {
        self.materials as u64 * LEAF_MATERIAL_BYTES
    }
}

/// Hashes one leaf run key: its length and every ordered material ID.
///
/// The hash is a lookup accelerator only; [`LeafInterner::matches`] confirms a
/// candidate against the arena before its base is reused.
#[inline]
fn content_hash(materials: &[u16]) -> u64 {
    let mut hash = Fx::default();
    hash.write_u64(materials.len() as u64);
    for &material in materials {
        hash.write_u16(material);
    }
    hash.finish()
}

/// A hash-consed set of leaf runs over one [`Bands`] leaf arena.
///
/// The interner does not own the arena; [`Self::intern`] and
/// [`Self::intern_dense`] take a mutable [`Bands`] and append to its leaf
/// arena. Runs are keyed on their ordered material IDs only, so identical
/// payloads share one base across parents and chunks that use the same pool.
///
/// A run must contain between one and [`LEAF_RUN_MATERIALS`] materials, none of
/// which may be [`AIR`]. Callers that hold a dense eight-child buffer should
/// use [`Self::intern_dense`], which removes air and returns the parent mask.
#[derive(Clone, Debug, Default)]
pub struct LeafInterner {
    /// Content hash -> candidate bases in insertion order. A bucket can hold
    /// more than one base when distinct keys share a hash.
    index: FxMap<u64, Vec<u64>>,
    /// Base -> the content that owns it, used by equality checks and
    /// deallocation. Only live runs are present.
    runs: FxMap<u64, LeafMeta>,
    hits: u64,
    misses: u64,
    allocated_runs: u64,
    dead_runs: u64,
    dead_bytes: u64,
}

impl LeafInterner {
    /// Creates an empty interner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns one compact leaf run and returns its first arena slot.
    ///
    /// `materials` contains exactly the present children in ascending child
    /// order. The same ordered materials always return the same base while the
    /// run is live. An empty slice allocates nothing and returns the sentinel
    /// base `0`; that value is never referenced because an all-air parent has
    /// mask `0`.
    ///
    /// # Panics
    ///
    /// Panics if `materials` holds more than [`LEAF_RUN_MATERIALS`] IDs or any
    /// [`AIR`] ID. Air is absent and belongs in the parent's mask, not in a
    /// stored run.
    pub fn intern(&mut self, bands: &mut Bands, materials: &[u16]) -> u64 {
        assert!(
            materials.len() <= LEAF_RUN_MATERIALS,
            "a leaf run holds at most {LEAF_RUN_MATERIALS} materials"
        );
        if materials.is_empty() {
            return 0;
        }
        assert!(
            materials.iter().all(|&material| material != AIR),
            "air ({AIR}) is absent and must not be stored in a leaf run"
        );
        let hash = content_hash(materials);
        self.intern_with_hash(bands, materials, hash)
    }

    /// [`Self::intern`] with an explicit content hash.
    ///
    /// Production callers use [`Self::intern`]. Tests use this private form to
    /// force different keys onto one hash and exercise the collision path.
    fn intern_with_hash(&mut self, bands: &mut Bands, materials: &[u16], hash: u64) -> u64 {
        let found = self.index.get(&hash).and_then(|bases| {
            bases
                .iter()
                .copied()
                .find(|&base| self.matches(bands, materials, base))
        });
        if let Some(base) = found {
            self.hits += 1;
            return base;
        }

        let base = bands.leaf_len();
        for &material in materials {
            bands.append_leaf(material);
        }
        self.runs.insert(
            base,
            LeafMeta {
                hash,
                materials: materials.len() as u8,
            },
        );
        self.index.entry(hash).or_default().push(base);
        self.allocated_runs += 1;
        self.misses += 1;
        base
    }

    /// Interns a dense eight-child buffer, dropping air, and returns
    /// `(parent mask, run base)`.
    ///
    /// `children[c]` is the material of child `c`; [`AIR`] clears bit `c`. The
    /// stored run contains only the non-air materials in ascending child order.
    /// An all-air buffer returns `(0, 0)` and allocates nothing.
    pub fn intern_dense(
        &mut self,
        bands: &mut Bands,
        children: &[u16; LEAF_RUN_MATERIALS],
    ) -> (u8, u64) {
        let mut mask = 0u8;
        let mut materials = [0u16; LEAF_RUN_MATERIALS];
        let mut count = 0usize;
        for (c, &material) in children.iter().enumerate() {
            if material == AIR {
                continue;
            }
            mask |= 1 << c;
            materials[count] = material;
            count += 1;
        }
        let base = self.intern(bands, &materials[..count]);
        (mask, base)
    }

    /// True when `base` names a live run with exactly these ordered materials.
    fn matches(&self, bands: &Bands, materials: &[u16], base: u64) -> bool {
        let Some(meta) = self.runs.get(&base) else {
            return false;
        };
        if usize::from(meta.materials) != materials.len() {
            return false;
        }
        let Ok(start) = usize::try_from(base) else {
            return false;
        };
        let Some(end) = start.checked_add(materials.len()) else {
            return false;
        };
        bands.leaves().get(start..end) == Some(materials)
    }

    /// Retires the live run starting at `base`.
    ///
    /// The run is removed from the intern index, so interning the same
    /// materials again appends a fresh run instead of returning this base. The
    /// slots are **not** returned to the arena: leaf arenas are append-only, so
    /// reclamation is offline compaction (M1-12). Returns `false` when `base`
    /// is not a live run.
    ///
    /// # Caller contract
    ///
    /// Retire a run only when no live root or in-flight frame can still reach
    /// it. Reference counting and the epoch list arrive with the versioned GPU
    /// pool (M4-01…M4-08); the invariant checker (M1-14) verifies reachability,
    /// but this method cannot prove that condition.
    pub fn deallocate(&mut self, base: u64) -> bool {
        let Some(meta) = self.runs.remove(&base) else {
            return false;
        };

        if let Some(bucket) = self.index.get_mut(&meta.hash) {
            if let Some(position) = bucket.iter().position(|&candidate| candidate == base) {
                bucket.swap_remove(position);
            }
            if bucket.is_empty() {
                self.index.remove(&meta.hash);
            }
        }

        self.dead_runs += 1;
        self.dead_bytes += meta.bytes();
        true
    }

    /// Number of live interned leaf runs.
    pub fn live_runs(&self) -> usize {
        self.runs.len()
    }

    /// Number of runs ever appended, including retired ones.
    pub const fn allocated_runs(&self) -> u64 {
        self.allocated_runs
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

    /// Bytes owned by retired runs.
    ///
    /// These bytes are still physically present because the arena is
    /// append-only; they are input to the compaction trigger (M1-12).
    pub const fn dead_bytes(&self) -> u64 {
        self.dead_bytes
    }

    /// Material slots held by live runs, excluding retired runs.
    pub fn live_materials(&self) -> u64 {
        self.runs
            .values()
            .map(|meta| u64::from(meta.materials))
            .sum()
    }

    /// Bytes held by live runs in the leaf arena.
    pub fn live_bytes(&self) -> u64 {
        self.live_materials() * LEAF_MATERIAL_BYTES
    }

    /// True when no run has ever been interned.
    pub const fn is_empty(&self) -> bool {
        self.allocated_runs == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift64, so a failure is reproducible.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn full_run(start: u16) -> [u16; LEAF_RUN_MATERIALS] {
        core::array::from_fn(|i| start + i as u16)
    }

    /// M1-06 acceptance: identical material sequences intern to one shared base
    /// and are stored only once.
    #[test]
    fn leaf_runs_identical_materials_are_shared() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let first = leaves.intern(&mut bands, &[3, 7, 11]);
        let second = leaves.intern(&mut bands, &[3, 7, 11]);

        assert_eq!(first, second);
        assert_eq!(first, 0);
        assert_eq!(bands.leaves(), &[3, 7, 11]);
        assert_eq!(bands.leaf_len(), 3);
        assert_eq!(leaves.live_runs(), 1);
        assert_eq!(leaves.allocated_runs(), 1);
        assert_eq!(leaves.hit_count(), 1);
        assert_eq!(leaves.miss_count(), 1);
        assert_eq!(leaves.live_materials(), 3);
        assert_eq!(leaves.live_bytes(), 6);
    }

    /// The key is the ordered material sequence, including its length.
    #[test]
    fn leaf_runs_key_separates_order_and_length() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let ab = leaves.intern(&mut bands, &[1, 2]);
        let ba = leaves.intern(&mut bands, &[2, 1]);
        let a = leaves.intern(&mut bands, &[1]);
        let abc = leaves.intern(&mut bands, &[1, 2, 3]);
        let aa = leaves.intern(&mut bands, &[1, 1]);

        assert_eq!((ab, ba, a, abc, aa), (0, 2, 4, 5, 8));
        assert_eq!(leaves.live_runs(), 5);
        assert_eq!(leaves.hit_count(), 0);
        assert_eq!(leaves.miss_count(), 5);

        // Each key stays idempotent after its neighbours were appended.
        assert_eq!(leaves.intern(&mut bands, &[1, 2]), ab);
        assert_eq!(leaves.intern(&mut bands, &[2, 1]), ba);
        assert_eq!(leaves.intern(&mut bands, &[1]), a);
        assert_eq!(leaves.intern(&mut bands, &[1, 2, 3]), abc);
        assert_eq!(leaves.intern(&mut bands, &[1, 1]), aa);
        assert_eq!(leaves.hit_count(), 5);
        assert_eq!(bands.leaf_len(), 10);
    }

    /// Air is absent in a dense buffer: it clears the parent mask, is not
    /// stored, and reads back as `0`.
    #[test]
    fn leaf_runs_air_is_absent_and_reads_as_zero() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let (mask, base) = leaves.intern_dense(&mut bands, &[0, 7, 0, 9, 0, 0, 11, 0]);

        assert_eq!(mask, 0b0100_1010);
        assert_eq!(base, 0);
        assert_eq!(bands.leaves(), &[7, 9, 11]);
        assert_eq!(leaves.live_runs(), 1);

        let parent = Node::new(mask, 1, base);
        assert_eq!(fetch_leaf(&bands, parent, 0), AIR);
        assert_eq!(fetch_leaf(&bands, parent, 1), 7);
        assert_eq!(fetch_leaf(&bands, parent, 2), AIR);
        assert_eq!(fetch_leaf(&bands, parent, 3), 9);
        assert_eq!(fetch_leaf(&bands, parent, 4), AIR);
        assert_eq!(fetch_leaf(&bands, parent, 5), AIR);
        assert_eq!(fetch_leaf(&bands, parent, 6), 11);
        assert_eq!(fetch_leaf(&bands, parent, 7), AIR);

        // A parent from another level has no leaf children.
        assert_eq!(fetch_leaf(&bands, Node::new(mask, 2, base), 1), AIR);
        assert_eq!(fetch_leaf(&bands, Node::new(mask, 0, base), 1), AIR);
    }

    /// A full leaf run is eight materials = 16 B; partial runs pack with no
    /// padding.
    #[test]
    fn leaf_runs_full_run_is_sixteen_bytes_and_partials_pack() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let full = leaves.intern(&mut bands, &full_run(20));
        assert_eq!(full, 0);
        assert_eq!(bands.leaf_len(), LEAF_RUN_MATERIALS as u64);
        assert_eq!(bands.leaf_len() * LEAF_MATERIAL_BYTES, LEAF_RUN_BYTES);

        let partial = leaves.intern(&mut bands, &[30, 31, 32]);
        assert_eq!(partial, 8);
        assert_eq!(bands.leaf_len(), 11);
        assert_eq!(bands.leaves()[8..], [30, 31, 32]);

        // A second full run starts immediately after the partial one.
        let second_full = leaves.intern(&mut bands, &full_run(40));
        assert_eq!(second_full, 11);
        assert_eq!(bands.leaf_len(), 19);
    }

    /// Parents with different masks but the same present materials share one
    /// run: the mask, not the run, decides which positions are visible.
    #[test]
    fn leaf_runs_share_across_different_masks() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let (mask_a, base_a) = leaves.intern_dense(&mut bands, &[5, 6, 0, 0, 0, 0, 0, 0]);
        let (mask_b, base_b) = leaves.intern_dense(&mut bands, &[0, 0, 5, 0, 0, 6, 0, 0]);

        assert_eq!((mask_a, mask_b), (0b0000_0011, 0b0010_0100));
        assert_eq!(base_a, base_b);
        assert_eq!(bands.leaves(), &[5, 6]);
        assert_eq!(leaves.live_runs(), 1);
        assert_eq!(leaves.hit_count(), 1);

        assert_eq!(fetch_leaf(&bands, Node::new(mask_a, 1, base_a), 0), 5);
        assert_eq!(fetch_leaf(&bands, Node::new(mask_a, 1, base_a), 1), 6);
        assert_eq!(fetch_leaf(&bands, Node::new(mask_b, 1, base_b), 2), 5);
        assert_eq!(fetch_leaf(&bands, Node::new(mask_b, 1, base_b), 5), 6);
    }

    #[test]
    fn leaf_runs_all_air_allocates_nothing() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let (mask, base) = leaves.intern_dense(&mut bands, &[AIR; LEAF_RUN_MATERIALS]);

        assert_eq!((mask, base), (0, 0));
        assert_eq!(bands.leaf_len(), 0);
        assert_eq!(bands.leaf_range(), 0..0);
        assert_eq!(leaves.live_runs(), 0);
        assert_eq!(leaves.allocated_runs(), 0);
        assert_eq!(leaves.hit_count() + leaves.miss_count(), 0);
        assert!(leaves.is_empty());

        // The sentinel is not a stored run; a real run can still use base zero.
        let real = leaves.intern(&mut bands, &[1]);
        assert_eq!(real, 0);
        assert_eq!(bands.leaf_len(), 1);
        assert!(!leaves.is_empty());
    }

    /// Force distinct keys onto one hash: equality must keep their bases apart
    /// and each key idempotent.
    #[test]
    fn leaf_runs_hash_collision_compares_full_content() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();
        let forced_hash = 0xfeed_face_cafe_beef;

        let first = leaves.intern_with_hash(&mut bands, &[1, 2], forced_hash);
        let second = leaves.intern_with_hash(&mut bands, &[2, 1], forced_hash);
        let third = leaves.intern_with_hash(&mut bands, &[1, 2, 3], forced_hash);

        assert_eq!((first, second, third), (0, 2, 4));
        let bucket = leaves
            .index
            .get(&forced_hash)
            .expect("the forced hash has a bucket");
        assert_eq!(bucket.as_slice(), &[first, second, third]);

        assert_eq!(
            leaves.intern_with_hash(&mut bands, &[1, 2], forced_hash),
            first
        );
        assert_eq!(
            leaves.intern_with_hash(&mut bands, &[2, 1], forced_hash),
            second
        );
        assert_eq!(
            leaves.intern_with_hash(&mut bands, &[1, 2, 3], forced_hash),
            third
        );
        assert_eq!(leaves.live_runs(), 3);
        assert_eq!(leaves.allocated_runs(), 3);
        assert_eq!(leaves.hit_count(), 3);
    }

    #[test]
    fn leaf_runs_deallocate_retires_and_reintern_appends() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        let first = leaves.intern(&mut bands, &[4, 5, 6]);
        assert_eq!((first, leaves.live_runs()), (0, 1));

        assert!(leaves.deallocate(first));
        assert_eq!(leaves.live_runs(), 0);
        assert_eq!(leaves.dead_run_count(), 1);
        assert_eq!(leaves.dead_bytes(), 3 * LEAF_MATERIAL_BYTES);
        assert!(!leaves.deallocate(first));
        assert!(!leaves.deallocate(99));
        assert_eq!(leaves.dead_run_count(), 1);

        // The retired key is no longer interned: the same content appends a
        // fresh run, while the old slots stay in the append-only arena.
        let second = leaves.intern(&mut bands, &[4, 5, 6]);
        assert_eq!(second, 3);
        assert_eq!(bands.leaves(), &[4, 5, 6, 4, 5, 6]);
        assert_eq!(leaves.live_runs(), 1);
        assert_eq!(leaves.allocated_runs(), 2);
        assert_eq!(leaves.miss_count(), 2);
        assert_eq!(leaves.hit_count(), 0);
    }

    /// The M1-06 dedup property over 10k calls: repeated content returns its
    /// original base without appending; distinct content gets a distinct base.
    #[test]
    fn leaf_runs_10k_calls_are_content_addressed() {
        use std::collections::HashMap;

        const CASES: usize = 10_000;

        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();
        let mut state = 0x0ddc_0ffe_e15e_beefu64;
        let mut entries: Vec<(Vec<u16>, u64)> = Vec::new();
        let mut by_key: HashMap<Vec<u16>, u64> = HashMap::new();
        let mut by_base: HashMap<u64, Vec<u16>> = HashMap::new();
        let mut expected_hits = 0u64;
        let mut expected_misses = 0u64;
        let mut expected_slots = 0u64;

        for case in 0..CASES {
            let reuse = case > 0 && rng(&mut state).is_multiple_of(3);
            let materials = if reuse {
                entries[(rng(&mut state) as usize) % entries.len()]
                    .0
                    .clone()
            } else {
                let count = (rng(&mut state) % LEAF_RUN_MATERIALS as u64) as usize + 1;
                (0..count)
                    .map(|_| (rng(&mut state) % (u16::MAX as u64 - 1)) as u16 + 1)
                    .collect::<Vec<_>>()
            };

            let before = leaves.allocated_runs();
            let base = leaves.intern(&mut bands, &materials);

            if let Some(&existing) = by_key.get(&materials) {
                assert_eq!(base, existing, "case {case}: key must be idempotent");
                assert_eq!(
                    leaves.allocated_runs(),
                    before,
                    "case {case}: a hit must not append"
                );
                expected_hits += 1;
            } else {
                assert_eq!(
                    leaves.allocated_runs(),
                    before + 1,
                    "case {case}: a miss appends exactly one run"
                );
                assert!(
                    by_base.insert(base, materials.clone()).is_none(),
                    "case {case}: two distinct keys share base {base}"
                );
                by_key.insert(materials.clone(), base);
                entries.push((materials.clone(), base));
                expected_slots += materials.len() as u64;
                expected_misses += 1;
            }
        }

        assert!(expected_hits > 0);
        assert_eq!(expected_hits + expected_misses, CASES as u64);
        assert_eq!(leaves.hit_count(), expected_hits);
        assert_eq!(leaves.miss_count(), expected_misses);
        assert_eq!(leaves.live_runs(), entries.len());
        assert_eq!(leaves.allocated_runs(), entries.len() as u64);
        assert_eq!(bands.leaf_len(), expected_slots);
        assert_eq!(leaves.live_materials(), expected_slots);
    }

    /// The read side follows the parent's popcount addressing for every mask.
    #[test]
    fn leaf_runs_fetch_matches_popcount_addressing() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();

        for mask in 0..=u8::MAX {
            let dense: [u16; LEAF_RUN_MATERIALS] = core::array::from_fn(|c| {
                if mask & (1 << c) == 0 {
                    AIR
                } else {
                    u16::from(mask) * 8 + c as u16 + 1
                }
            });
            let (stored_mask, base) = leaves.intern_dense(&mut bands, &dense);
            assert_eq!(stored_mask, mask);
            let parent = Node::new(stored_mask, 1, base);

            for c in 0..LEAF_RUN_MATERIALS as u32 {
                let expected = if mask & (1 << c) == 0 {
                    AIR
                } else {
                    dense[c as usize]
                };
                assert_eq!(
                    fetch_leaf(&bands, parent, c),
                    expected,
                    "mask {mask:#010b}, child {c}"
                );
            }
            assert_eq!(fetch_leaf(&bands, parent, LEAF_RUN_MATERIALS as u32), AIR);
        }
    }

    #[test]
    #[should_panic(expected = "at most 8 materials")]
    fn leaf_runs_reject_more_than_eight_materials() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();
        leaves.intern(&mut bands, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    #[should_panic(expected = "air (0) is absent")]
    fn leaf_runs_reject_stored_air() {
        let mut bands = Bands::new();
        let mut leaves = LeafInterner::new();
        leaves.intern(&mut bands, &[1, AIR, 3]);
    }
}
