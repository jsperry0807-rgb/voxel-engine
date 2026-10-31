//! Per-block dirty flags derived from a COW edit batch (M2-22).
//!
//! The remesh scheduler (M2-23) works in blocks, not voxels: one 16³ block is
//! one mesh unit, so an edit only has to say *which blocks* changed. This module
//! turns a batch of `(voxel index, material)` edits into a bitset with one bit
//! per block.
//!
//! # The apron
//!
//! A block's mesh reads its `16³` voxels plus a one-voxel border on every face
//! (AO, face culling and the block scratch's apron). A voxel on a block boundary
//! is therefore read by the block on both sides of that boundary:
//!
//! - an interior voxel dirties its own block;
//! - a voxel on one block face dirties two blocks;
//! - an edge voxel dirties four, and a corner voxel eight (the `2×2×2` blocks
//!   whose scratches contain it).
//!
//! A voxel on the chunk border has no block beyond the border, so it dirties
//! only the blocks inside this chunk. The block across the chunk border reads
//! the edited voxel through its apron plane; [`DirtyBlocks::mark_apron_cell`]
//! marks exactly that side, so the streaming layer can dirty the neighbour
//! without over-marking a whole face. Light propagation keeps its own dirty
//! region (`LightState`); this bitset is the remesh set.
//!
//! # The diff
//!
//! [`DirtyBlocks::from_edits`] compares each edit against the **pre-edit**
//! [`Svdag`] and marks added, removed and changed voxels; a write of the material
//! the voxel already holds marks nothing. Edits to the same voxel collapse with
//! last-wins semantics, matching [`Svdag::apply_edits`], so a batch that changes
//! a voxel and changes it back is not dirty.
//!
//! A solid → solid change only marks the voxel's own block. The mesher's face
//! culling and AO are binary (`material == 0` versus not) and the vertex carries
//! no light, so a neighbouring block cannot change. This precision relies on
//! that; if a later pass makes culling or AO depend on the neighbour's material,
//! the builder must fall back to [`DirtyBlocks::mark_voxel`] for every edit.
//!
//! # Layout
//!
//! Block `(x, y, z)` has linear index `x + y·n + z·n²` (`x` fastest), and bit
//! `index % 64` of word `index / 64`. The words hold 1 bit per block: 1 word at
//! 64³ and 8 words at 128³.

use std::collections::BTreeMap;

use voxel_core::constants::BLOCK_DIM;
use voxel_storage::dag::Svdag;
use voxel_storage::{AIR, CHUNK_DIM, CHUNK_VOLUME, Face, local_xyz};

use crate::BlockPos;

/// Blocks per chunk edge at the compiled chunk edge.
pub const BLOCKS_PER_CHUNK_EDGE: usize = CHUNK_DIM / BLOCK_DIM;

/// One dirty bit per block, in a flat word array.
///
/// The bitset is a reusable scratch: [`Self::clear`] resets it between edits,
/// [`Self::clear_block`] removes one block the scheduler has meshed, and
/// [`Self::iter`] yields the dirty blocks in ascending linear order. It carries
/// no chunk position and no generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirtyBlocks {
    blocks_per_edge: usize,
    words: Vec<u64>,
}

impl DirtyBlocks {
    /// An empty bitset at the compiled chunk edge.
    #[must_use]
    pub fn new() -> Self {
        Self::for_edge(BLOCKS_PER_CHUNK_EDGE)
    }

    /// An empty bitset for a chunk with `blocks_per_edge` blocks per edge.
    ///
    /// [`Self::mark_block`], [`Self::is_dirty`] and [`Self::iter`] work at any
    /// edge. [`Self::mark_voxel`] and [`Self::from_edits`] assume the bitset
    /// covers the compiled chunk edge.
    ///
    /// # Panics
    ///
    /// Panics if `blocks_per_edge` is zero.
    #[must_use]
    pub fn for_edge(blocks_per_edge: usize) -> Self {
        assert!(blocks_per_edge > 0, "a chunk needs at least one block");
        let blocks = blocks_per_edge * blocks_per_edge * blocks_per_edge;
        Self {
            blocks_per_edge,
            words: vec![0; blocks.div_ceil(64)],
        }
    }

    /// Blocks per chunk edge.
    pub const fn blocks_per_edge(&self) -> usize {
        self.blocks_per_edge
    }

    /// The raw words, one bit per block, LSB first within each word.
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Total blocks the bitset covers.
    pub fn len(&self) -> usize {
        self.blocks_per_edge.pow(3)
    }

    /// True when no block is dirty.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&word| word == 0)
    }

    /// Number of dirty blocks.
    pub fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Clears every bit, keeping the allocation.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    /// True when `block` is dirty.
    ///
    /// # Panics
    ///
    /// Panics if the block is outside the chunk.
    pub fn is_dirty(&self, block: BlockPos) -> bool {
        let index = self.index(block);
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }

    /// Marks one block dirty.
    ///
    /// # Panics
    ///
    /// Panics if the block is outside the chunk.
    pub fn mark_block(&mut self, block: BlockPos) {
        let index = self.index(block);
        self.words[index / 64] |= 1u64 << (index % 64);
    }

    /// Clears one block, returning true when it was dirty.
    ///
    /// The scheduler uses this to retire a block it meshed this frame while
    /// keeping the rest of the backlog.
    ///
    /// # Panics
    ///
    /// Panics if the block is outside the chunk.
    pub fn clear_block(&mut self, block: BlockPos) -> bool {
        let index = self.index(block);
        let mask = 1u64 << (index % 64);
        let was_dirty = self.words[index / 64] & mask != 0;
        self.words[index / 64] &= !mask;
        was_dirty
    }

    /// Adds every dirty block of `other` to this set.
    ///
    /// # Panics
    ///
    /// Panics if the two bitsets cover different chunk edges.
    pub fn union(&mut self, other: &Self) {
        assert_eq!(
            self.blocks_per_edge, other.blocks_per_edge,
            "dirty bitsets must cover the same chunk edge"
        );
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    /// Marks every block whose mesh reads this voxel.
    ///
    /// A block's scratch covers its own `16³` voxels plus a one-voxel border, so
    /// a voxel is read by the blocks on both sides of every block boundary it
    /// touches. Taking the product over the three axes gives the AO apron:
    /// 1, 2, 4 or 8 blocks for an interior, face, edge or corner voxel. A
    /// chunk-border voxel has no block beyond the border and marks only the
    /// blocks inside this chunk.
    ///
    /// # Panics
    ///
    /// Panics if the voxel is outside the chunk.
    pub fn mark_voxel(&mut self, x: u32, y: u32, z: u32) {
        let edge = (self.blocks_per_edge * BLOCK_DIM) as u32;
        assert!(
            x < edge && y < edge && z < edge,
            "voxel ({x}, {y}, {z}) is outside the {edge}³ chunk"
        );
        let coord = [x as usize, y as usize, z as usize];

        // Inclusive block range per axis. Both the first and the last voxel of
        // a block lie in the neighbouring block's one-voxel border.
        let mut range = [[0usize; 2]; 3];
        for axis in 0..3 {
            let block = coord[axis] / BLOCK_DIM;
            let local = coord[axis] % BLOCK_DIM;
            range[axis] = [block, block];
            if local == 0 && block > 0 {
                range[axis][0] = block - 1;
            }
            if local == BLOCK_DIM - 1 && block + 1 < self.blocks_per_edge {
                range[axis][1] = block + 1;
            }
        }

        for z in range[2][0]..=range[2][1] {
            for y in range[1][0]..=range[1][1] {
                for x in range[0][0]..=range[0][1] {
                    self.mark_block(BlockPos::new(x as u32, y as u32, z as u32));
                }
            }
        }
    }

    /// Marks the blocks that read one apron cell of `face`.
    ///
    /// `u` and `v` are the apron plane's coordinates ([`Face::offset`]); the
    /// bitset belongs to the chunk that will read the cell through its apron.
    /// The inside voxel adjacent to the plane is used, so this is exactly the
    /// neighbour-side counterpart of [`Self::mark_voxel`]: one to four blocks on
    /// the facing layer, not the whole face.
    ///
    /// # Panics
    ///
    /// Panics if `u` or `v` is outside the compiled chunk edge.
    pub fn mark_apron_cell(&mut self, face: Face, u: u32, v: u32) {
        let edge = (self.blocks_per_edge * BLOCK_DIM) as u32;
        assert!(
            u < edge && v < edge,
            "apron cell ({u}, {v}) is outside the {edge}² plane"
        );
        let inside = edge - 1;
        let (x, y, z) = match face {
            Face::NegX => (0, v, u),
            Face::PosX => (inside, v, u),
            Face::NegY => (v, 0, u),
            Face::PosY => (v, inside, u),
            Face::NegZ => (u, v, 0),
            Face::PosZ => (u, v, inside),
        };
        self.mark_voxel(x, y, z);
    }

    /// Marks the block(s) for one edit, when the material actually changes.
    ///
    /// Added (air → solid) and removed (solid → air) edits mark
    /// [`Self::mark_voxel`]'s apron. A changed (solid → other solid) edit marks
    /// only the voxel's own block, because the mesher's culling and AO are
    /// binary and no neighbouring block can change.
    ///
    /// Returns true when the write changed the voxel (`before != after`).
    ///
    /// # Panics
    ///
    /// Panics if the voxel is outside the chunk.
    #[must_use]
    pub fn mark_edit(&mut self, x: u32, y: u32, z: u32, before: u16, after: u16) -> bool {
        if before == after {
            return false;
        }
        if before != AIR && after != AIR {
            self.mark_block(BlockPos::new(
                x / BLOCK_DIM as u32,
                y / BLOCK_DIM as u32,
                z / BLOCK_DIM as u32,
            ));
            return true;
        }
        self.mark_voxel(x, y, z);
        true
    }

    /// Derives the dirty blocks of an edit batch against the **pre-edit** chunk.
    ///
    /// Added, removed and changed voxels mark their block and apron as
    /// [`Self::mark_edit`] describes; a write of the material already present
    /// marks nothing. Repeated indices collapse with last-wins semantics,
    /// matching [`Svdag::apply_edits`], so a batch that reverts a voxel before it
    /// is published is not dirty.
    ///
    /// The caller applies the same batch after computing the bitset; this
    /// function never mutates the chunk. Passing the post-edit chunk yields an
    /// empty set, so compute the bitset first.
    ///
    /// # Panics
    ///
    /// Panics if an index is not below the chunk volume.
    #[must_use]
    pub fn from_edits<I>(before: &Svdag, edits: I) -> Self
    where
        I: IntoIterator<Item = (u32, u16)>,
    {
        let mut latest: BTreeMap<u32, u16> = BTreeMap::new();
        for (index, material) in edits {
            latest.insert(index, material);
        }

        let mut dirty = Self::new();
        for (index, material) in latest {
            assert!(
                (index as usize) < CHUNK_VOLUME,
                "voxel index {index} is outside the chunk"
            );
            let (x, y, z) = local_xyz(index);
            let _ = dirty.mark_edit(x, y, z, before.get(x, y, z), material);
        }
        dirty
    }

    /// Removes up to `budget` dirty blocks in ascending linear order.
    ///
    /// `out` is cleared first and reused. A zero budget removes nothing and
    /// leaves `out` empty. This is the scheduler's drain (M2-23): it owns the
    /// layout decode, so callers never re-derive the bit-to-block mapping.
    pub fn take_first(&mut self, budget: usize, out: &mut Vec<BlockPos>) -> usize {
        out.clear();
        if budget == 0 || self.is_empty() {
            return 0;
        }

        let edge = self.blocks_per_edge;
        let blocks = self.len();
        'words: for (word_index, &word) in self.words.iter().enumerate() {
            let mut word = word;
            while word != 0 {
                if out.len() >= budget {
                    break 'words;
                }
                let bit = word.trailing_zeros() as usize;
                word &= word - 1;
                let index = word_index * 64 + bit;
                if index >= blocks {
                    continue;
                }
                out.push(BlockPos::new(
                    (index % edge) as u32,
                    ((index / edge) % edge) as u32,
                    (index / (edge * edge)) as u32,
                ));
            }
        }

        for block in out.iter() {
            self.clear_block(*block);
        }
        out.len()
    }

    /// Iterates the dirty blocks in ascending linear order (`x` fastest).
    pub fn iter(&self) -> impl Iterator<Item = BlockPos> + '_ {
        let edge = self.blocks_per_edge;
        let blocks = self.len();
        self.words
            .iter()
            .enumerate()
            .flat_map(move |(word_index, &word)| {
                (0..64).filter_map(move |bit| {
                    if word & (1u64 << bit) == 0 {
                        return None;
                    }
                    let index = word_index * 64 + bit;
                    if index >= blocks {
                        return None;
                    }
                    Some(BlockPos::new(
                        (index % edge) as u32,
                        ((index / edge) % edge) as u32,
                        (index / (edge * edge)) as u32,
                    ))
                })
            })
    }

    fn index(&self, block: BlockPos) -> usize {
        let edge = self.blocks_per_edge;
        assert!(
            (block.x as usize) < edge && (block.y as usize) < edge && (block.z as usize) < edge,
            "block ({}, {}, {}) is outside the {edge}³ chunk",
            block.x,
            block.y,
            block.z
        );
        block.x as usize + block.y as usize * edge + block.z as usize * edge * edge
    }
}

impl Default for DirtyBlocks {
    fn default() -> Self {
        Self::new()
    }
}
