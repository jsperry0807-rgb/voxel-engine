//! Block-occlusion flood-fill reference (M1-33).
//!
//! This is the slow, exact oracle for the per-frame block-occlusion pass
//! (M5-01). It answers the question ADR-006 fixes: **can the camera reach this
//! block through non-opaque neighbours?** — not "is the segment to this block
//! clear?". The fill is deliberately conservative: it over-includes through any
//! one-block gap and around corners, but never lets a reachable block be
//! marked occluded.
//!
//! # Grid
//!
//! The fill runs on **world block coordinates**, one cell per 16³ block. A
//! block is one metre on an edge ([`BLOCK_EDGE_M`]), so a camera position in
//! metres maps to the block containing it with [`camera_block`]. The seed is
//! the camera's block.
//!
//! # Fill rule
//!
//! The seed is always visible. Face-adjacent blocks are visited in the fixed
//! [`NEIGHBOURS`] order, positive axis before negative:
//!
//! 1. every neighbour of a reached cell is marked visible, because an opaque
//!    surface next to open space is exactly what the raster path must draw;
//! 2. a neighbour continues the fill only when [`is_opaque`](#opaque) reports it
//!    **non-opaque**.
//!
//! An opaque block is therefore a visible leaf, not a wall the fill crosses.
//! This is what keeps a solid cave wall in the draw set while the cave behind
//! it stays occluded.
//!
//! <a id="opaque"></a>
//! The caller supplies opacity through a closure. A block is non-opaque when
//! any part of it can pass the fill: air, a liquid, glass, leaves, or a
//! partially filled block. Only a block that is fully opaque on every voxel may
//! return `true`. Being conservative here is safe; reporting a passable block
//! as opaque would create a hole.
//!
//! # Horizon
//!
//! The horizon is a **Chebyshev radius in blocks**: the fill covers the cube
//! `seed - horizon ..= seed + horizon` on every axis, i.e. `(2h + 1)³` cells.
//! At the default [`DEFAULT_HORIZON_BLOCKS`] that is a 257³ cube, about 2.1 MB
//! at one bit per block. Anything outside the cube is conservatively visible:
//! [`OcclusionFill::is_visible`] returns `true` there, just as the traversal
//! falls back to the frustum beyond the horizon. The cube is the natural cut
//! for a compute dispatch and keeps the flood cost independent of scene depth.
//!
//! The cube is convex, so any straight segment between two covered blocks stays
//! covered. That is what lets the no-false-negative test use line of sight as a
//! weaker oracle than the connectivity fill.
//!
//! # Bits
//!
//! One bit per block, x fastest, then y, then z:
//!
//! ```text
//! index = (x - origin.x)
//!       + (y - origin.y) * side
//!       + (z - origin.z) * side * side
//! ```
//!
//! [`OcclusionFill::chunk_bits`] repacks a chunk into [`BLOCK_OCCLUSION_BYTES`]
//! bytes using the same block order as the chunk's block table
//! (`x + y·B + z·B²`), so the reference can be compared directly with the GPU
//! buffer in M5-01.
//!
//! # Determinism
//!
//! The seed and neighbour orders are fixed and the traversal is a FIFO queue.
//! The resulting bit set is a property of the reachable region, not the order,
//! but the fixed order makes counters and diagnostics reproducible too. The
//! opacity closure is called only for cells that are not already queued; it
//! must be pure.

use std::collections::VecDeque;

use voxel_core::constants::{BLOCK_DIM, BLOCK_LEVEL, BLOCKS_PER_CHUNK, CHUNK_DIM, aabb_extent};
use voxel_core::content_constants::{
    BLOCK_OCCLUSION_BITS_PER_BLOCK, BLOCK_OCCLUSION_BYTES, OCCLUSION_HORIZON_BLOCKS,
};

use super::cull::Vec3;
use crate::ChunkPos;

/// World-space edge of one block, in metres.
///
/// The layout fixes a block at 16³ voxels of [`VOXEL_SIZE`](voxel_core::constants::VOXEL_SIZE),
/// i.e. one cubic metre. The flood fill works in these units.
pub const BLOCK_EDGE_M: f64 = aabb_extent(BLOCK_LEVEL) as f64;

/// Default occlusion horizon, in blocks.
///
/// Mirrors [`OCCLUSION_HORIZON_BLOCKS`]; the fill covers a
/// `(2 * horizon + 1)³` cube around the camera block.
pub const DEFAULT_HORIZON_BLOCKS: u32 = OCCLUSION_HORIZON_BLOCKS as u32;

/// Face-neighbour order used by the fill.
///
/// Axes in `x`, `y`, `z` order, positive direction before negative on each
/// axis. The order does not change the reachable set; it keeps a run
/// reproducible.
pub const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// The block containing a world-space camera position.
///
/// # Panics
///
/// Panics if a coordinate is non-finite or outside the `i32` block range.
pub fn camera_block(camera: Vec3) -> [i32; 3] {
    [
        block_axis(camera.x),
        block_axis(camera.y),
        block_axis(camera.z),
    ]
}

fn block_axis(metres: f64) -> i32 {
    assert!(
        metres.is_finite(),
        "a camera coordinate must be finite, got {metres}"
    );
    let block = (metres / BLOCK_EDGE_M).floor();
    assert!(
        block >= i32::MIN as f64 && block <= i32::MAX as f64,
        "camera coordinate {metres} is outside the block coordinate range"
    );
    block as i32
}

/// Per-block visibility bits produced by one camera-seeded fill.
///
/// The bitset covers the horizon cube around [`Self::seed`]; one bit per block,
/// x fastest. Queries outside the cube resolve visible, because the traversal
/// lets the frustum decide beyond the horizon (ADR-006).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcclusionFill {
    seed: [i32; 3],
    horizon: u32,
    min: [i32; 3],
    side: usize,
    words: Vec<u64>,
    reached: usize,
}

impl OcclusionFill {
    /// The camera block the fill was seeded from.
    pub const fn seed(&self) -> [i32; 3] {
        self.seed
    }

    /// The Chebyshev horizon, in blocks.
    pub const fn horizon_blocks(&self) -> u32 {
        self.horizon
    }

    /// The lowest block coordinate covered by the bitset.
    pub const fn origin(&self) -> [i32; 3] {
        self.min
    }

    /// Blocks per edge of the covered cube, `2 * horizon + 1`.
    pub const fn side(&self) -> usize {
        self.side
    }

    /// The raw one-bit-per-block words.
    ///
    /// Trailing bits in the last word are zero. Use [`Self::is_visible`] for
    /// bounds-safe reads; this accessor exists for parity hashing against the
    /// GPU buffer.
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Number of blocks the fill traversed.
    ///
    /// This is the seed, even when it is opaque, plus every non-opaque block
    /// reachable through non-opaque neighbours. Opaque boundary blocks are
    /// visible but are not traversed, so this is the **traversed volume** M1-34
    /// compares with the visible volume.
    pub const fn reached_blocks(&self) -> usize {
        self.reached
    }

    /// True when the block lies inside the horizon cube.
    pub fn is_within_horizon(&self, block: [i32; 3]) -> bool {
        let dx = i64::from(block[0]) - i64::from(self.seed[0]);
        let dy = i64::from(block[1]) - i64::from(self.seed[1]);
        let dz = i64::from(block[2]) - i64::from(self.seed[2]);
        dx.abs() <= i64::from(self.horizon)
            && dy.abs() <= i64::from(self.horizon)
            && dz.abs() <= i64::from(self.horizon)
    }

    /// True when the block is visible.
    ///
    /// Blocks outside the horizon cube are conservatively visible: the fill
    /// does not know them and the frustum alone decides.
    pub fn is_visible(&self, block: [i32; 3]) -> bool {
        let Some(index) = self.index(block) else {
            return true;
        };
        self.bit(index)
    }

    /// True when the block is inside the horizon and was not reached.
    ///
    /// A block outside the horizon is never reported occluded.
    pub fn is_occluded(&self, block: [i32; 3]) -> bool {
        self.is_within_horizon(block) && !self.is_visible(block)
    }

    /// Visible blocks inside the horizon cube, in `z, y, x` order with x
    /// fastest.
    pub fn visible_blocks(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.blocks_with_visibility(true)
    }

    /// Occluded blocks inside the horizon cube, in `z, y, x` order with x
    /// fastest.
    pub fn occluded_blocks(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.blocks_with_visibility(false)
    }

    /// Number of visible bits inside the horizon cube.
    pub fn visible_count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Number of occluded bits inside the horizon cube.
    pub fn occluded_count(&self) -> usize {
        self.volume() - self.visible_count()
    }

    /// Packs one chunk's block bits into the per-chunk occlusion layout.
    ///
    /// The returned vector is exactly [`BLOCK_OCCLUSION_BYTES`] long. Bit
    /// `x + y·B + z·B²` is `1` when the block is visible; blocks
    /// outside the horizon set their bit, so a chunk fully beyond the horizon
    /// reads all ones. This is the shape M5-01 compares against the compute
    /// pass.
    pub fn chunk_bits(&self, chunk: ChunkPos) -> Vec<u8> {
        let blocks_per_edge = CHUNK_DIM / BLOCK_DIM;
        let mut bytes = vec![0u8; BLOCK_OCCLUSION_BYTES];
        for lz in 0..blocks_per_edge {
            for ly in 0..blocks_per_edge {
                for lx in 0..blocks_per_edge {
                    let global = [
                        i64::from(chunk.x) * blocks_per_edge as i64 + lx as i64,
                        i64::from(chunk.y) * blocks_per_edge as i64 + ly as i64,
                        i64::from(chunk.z) * blocks_per_edge as i64 + lz as i64,
                    ];
                    if self.is_visible_i64(global) {
                        let bit =
                            lx + ly * blocks_per_edge + lz * blocks_per_edge * blocks_per_edge;
                        bytes[bit / 8] |= 1 << (bit % 8);
                    }
                }
            }
        }
        bytes
    }

    fn volume(&self) -> usize {
        self.side * self.side * self.side
    }

    fn blocks_with_visibility(&self, visible: bool) -> impl Iterator<Item = [i32; 3]> + '_ {
        let side = self.side;
        (0..side).flat_map(move |z| {
            (0..side).flat_map(move |y| {
                (0..side).filter_map(move |x| {
                    let index = x + y * side + z * side * side;
                    if self.bit(index) != visible {
                        return None;
                    }
                    Some([
                        self.min[0] + x as i32,
                        self.min[1] + y as i32,
                        self.min[2] + z as i32,
                    ])
                })
            })
        })
    }

    fn is_visible_i64(&self, block: [i64; 3]) -> bool {
        let origin = [
            i64::from(self.min[0]),
            i64::from(self.min[1]),
            i64::from(self.min[2]),
        ];
        let side = self.side as i64;
        let local = [
            block[0] - origin[0],
            block[1] - origin[1],
            block[2] - origin[2],
        ];
        if local
            .iter()
            .any(|coordinate| *coordinate < 0 || *coordinate >= side)
        {
            return true;
        }
        let index = (local[0] + local[1] * side + local[2] * side * side) as usize;
        self.bit(index)
    }

    /// Chunk-local index of a world block, or `None` outside the cube.
    fn index(&self, block: [i32; 3]) -> Option<usize> {
        let local = [
            i64::from(block[0]) - i64::from(self.min[0]),
            i64::from(block[1]) - i64::from(self.min[1]),
            i64::from(block[2]) - i64::from(self.min[2]),
        ];
        let side = self.side as i64;
        if local
            .iter()
            .any(|coordinate| *coordinate < 0 || *coordinate >= side)
        {
            return None;
        }
        Some((local[0] + local[1] * side + local[2] * side * side) as usize)
    }

    fn bit(&self, index: usize) -> bool {
        self.words[index / 64] & (1u64 << (index % 64)) != 0
    }

    fn set_bit(&mut self, index: usize) {
        self.words[index / 64] |= 1u64 << (index % 64);
    }
}

/// Runs a camera-seeded fill from a world-space camera position.
///
/// This is [`flood_fill_from_block`] with the seed derived through
/// [`camera_block`].
pub fn flood_fill<F>(camera: Vec3, horizon_blocks: u32, is_opaque: F) -> OcclusionFill
where
    F: FnMut(i32, i32, i32) -> bool,
{
    flood_fill_from_block(camera_block(camera), horizon_blocks, is_opaque)
}

/// Runs a camera-seeded, 6-connected fill from a block coordinate.
///
/// `is_opaque(x, y, z)` classifies a **world block**; see the
/// [opaque rule](self#opaque). It is consulted only for neighbours that are not
/// already queued, so it should be pure. The fill covers a
/// `(2 * horizon_blocks + 1)³` cube; blocks outside it are not visited and
/// resolve visible.
///
/// The seed is always visible and is always processed, even when the camera is
/// inside a solid block. In that degenerate case its six face neighbours are
/// still marked visible, and non-opaque neighbours seed the normal fill. This is
/// the "when seed policy is ambiguous, resolve visible" rule from ADR-006.
///
/// # Panics
///
/// Panics if the horizon is too large for the resulting volume or block range
/// to be represented.
pub fn flood_fill_from_block<F>(
    seed: [i32; 3],
    horizon_blocks: u32,
    mut is_opaque: F,
) -> OcclusionFill
where
    F: FnMut(i32, i32, i32) -> bool,
{
    let horizon = i64::from(horizon_blocks);
    let side_i64 = horizon
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .expect("occlusion horizon overflows the block coordinate range");
    assert!(
        side_i64 <= i32::MAX as i64,
        "occlusion horizon is too large for a block-coordinate cube"
    );
    let side = usize::try_from(side_i64).expect("the horizon side is positive");

    let volume = side
        .checked_pow(3)
        .expect("the occlusion cube volume overflows usize");
    let words = volume.div_ceil(64);

    // The whole cube must be addressable in `i32` block coordinates, not just
    // the seed's lower corner: `blocks_with_visibility` materialises world
    // positions by adding the local offset.
    let radius = i32::try_from(horizon).expect("the horizon is a block count");
    let mut origin = [0; 3];
    for axis in 0..3 {
        seed[axis]
            .checked_sub(radius)
            .expect("seed block leaves the block coordinate range");
        seed[axis]
            .checked_add(radius)
            .expect("seed block leaves the block coordinate range");
        origin[axis] = seed[axis] - radius;
    }

    let mut fill = OcclusionFill {
        seed,
        horizon: horizon_blocks,
        min: origin,
        side,
        words: vec![0u64; words],
        reached: 1,
    };
    let mut queued = vec![0u64; fill.words.len()];
    let mut queue = VecDeque::new();

    let seed_index = fill
        .index(seed)
        .expect("the seed is the centre of its own cube");
    fill.set_bit(seed_index);
    set_queued(&mut queued, seed_index);
    queue.push_back(seed);

    while let Some([x, y, z]) = queue.pop_front() {
        for (dx, dy, dz) in NEIGHBOURS {
            let (Some(nx), Some(ny), Some(nz)) =
                (x.checked_add(dx), y.checked_add(dy), z.checked_add(dz))
            else {
                continue;
            };
            let Some(index) = fill.index([nx, ny, nz]) else {
                continue;
            };

            filled(&mut fill.words, index);
            if queued_at(&queued, index) {
                continue;
            }
            if !is_opaque(nx, ny, nz) {
                set_queued(&mut queued, index);
                fill.reached += 1;
                queue.push_back([nx, ny, nz]);
            }
        }
    }

    fill
}

fn filled(words: &mut [u64], index: usize) {
    words[index / 64] |= 1u64 << (index % 64);
}

fn queued_at(words: &[u64], index: usize) -> bool {
    words[index / 64] & (1u64 << (index % 64)) != 0
}

fn set_queued(words: &mut [u64], index: usize) {
    words[index / 64] |= 1u64 << (index % 64);
}

// The reference speaks the same per-block units as ADR-006 and the M5 buffer.
const _: () = {
    assert!(BLOCK_OCCLUSION_BITS_PER_BLOCK == 1);
    assert!(BLOCK_OCCLUSION_BYTES * 8 == BLOCKS_PER_CHUNK);
    // The block is the natural 1 m cell the horizon and the fill count in.
    assert!(aabb_extent(BLOCK_LEVEL) == 1.0);
    assert!(DEFAULT_HORIZON_BLOCKS as usize == OCCLUSION_HORIZON_BLOCKS);
};
