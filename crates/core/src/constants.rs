//! Fixed voxel, block and chunk geometry.
//!
//! Every size, shift and mask in the engine derives from these values; no other
//! module may spell out the chunk edge, the block edge or the resulting level
//! count. The chunk edge is the one tunable parameter ([`CHUNK_BITS`]); the
//! block edge and the voxel size are fixed by the spec.
//!
//! # Level numbering
//!
//! Levels count **down from the chunk root**: level `0` is the whole chunk and
//! every level halves the covered extent, so a node at level `l` covers
//! `LEVEL_SIZE[l]` voxels per axis. [`ROOT_LEVEL`] is the deepest level, where
//! a node covers a single voxel. [`BLOCK_LEVEL`] is the level whose nodes cover
//! exactly one 16³ block — the cut used for block-granular occlusion, light and
//! remeshing.
//!
//! Note that the SVDAG pool in `voxel-storage` stores its levels with the
//! opposite sense (root at `CHUNK_BITS`, leaf at `0`); its levels are
//! `ROOT_LEVEL - depth`.
//!
//! # Example
//!
//! ```
//! use voxel_core::constants::{
//!     aabb_extent, BLOCK_LEVEL, CHUNK_BITS, CHUNK_DIM, ROOT_LEVEL, VOXEL_SIZE,
//! };
//!
//! // The chunk edge follows `CHUNK_BITS`: 4 m at 64³, 8 m at 128³.
//! assert_eq!(aabb_extent(0), CHUNK_DIM as f32 * VOXEL_SIZE);
//! assert_eq!(aabb_extent(BLOCK_LEVEL), 1.0); // a 16³ block is 1 m³
//! assert_eq!(aabb_extent(ROOT_LEVEL), VOXEL_SIZE); // a leaf is one voxel
//! assert_eq!(CHUNK_DIM, 1 << CHUNK_BITS);
//! ```

/// Edge length of one voxel in world units (metres).
///
/// 6.25 cm, i.e. 16 voxels per metre.
pub const VOXEL_SIZE: f32 = 0.0625;

/// Voxels per block edge. A block is `BLOCK_DIM`³ voxels, one cubic metre.
pub const BLOCK_DIM: usize = 16;

/// `log2(BLOCK_DIM)`: split levels between the chunk edge and the block edge.
pub const BLOCK_BITS: u32 = 4;

/// `log2` of the chunk edge in voxels.
///
/// This is the single parameter of the chunk layout: 6 gives the 64³ baseline,
/// 7 gives the pre-registered 128³ target, selected by the `chunk128` feature.
/// Everything below is derived from it.
#[cfg(not(feature = "chunk128"))]
pub const CHUNK_BITS: u32 = 6;

/// `log2` of the chunk edge in voxels, at the pre-registered 128³ edge.
///
/// Selected by the `chunk128` feature. Everything below is derived from it.
#[cfg(feature = "chunk128")]
pub const CHUNK_BITS: u32 = 7;

/// `log2` of the chunk edge of the M1 baseline (64³ blocks).
///
/// [`CHUNK_BITS`] is either this baseline or [`TARGET_CHUNK_BITS`]. The
/// residency gate (M1-36) names both shapes explicitly so it can compare them
/// from either build without assuming which one the feature selected.
pub const BASELINE_CHUNK_BITS: u32 = 6;

/// `log2` of the chunk edge of the pre-registered 128³ target.
pub const TARGET_CHUNK_BITS: u32 = BASELINE_CHUNK_BITS + 1;

/// Voxels per edge of the M1 baseline chunk.
pub const BASELINE_CHUNK_DIM: usize = 1 << BASELINE_CHUNK_BITS;

/// Voxels per edge of the pre-registered target chunk.
pub const TARGET_CHUNK_DIM: usize = 1 << TARGET_CHUNK_BITS;

/// Voxels per chunk edge ([`CHUNK_BITS`] as a count).
pub const CHUNK_DIM: usize = 1 << CHUNK_BITS;

/// Voxels in one chunk.
pub const CHUNK_VOLUME: usize = CHUNK_DIM * CHUNK_DIM * CHUNK_DIM;

/// Deepest level of a chunk: `CHUNK_BITS` halvings from the root.
///
/// A node at this level covers one voxel. See the [module docs](self) for the
/// level-numbering convention.
pub const ROOT_LEVEL: u32 = CHUNK_BITS;

/// Level whose nodes cover exactly one block, i.e. `BLOCK_DIM` voxels per edge.
pub const BLOCK_LEVEL: u32 = CHUNK_BITS - BLOCK_BITS;

/// Blocks per chunk edge.
const BLOCKS_PER_EDGE: usize = CHUNK_DIM / BLOCK_DIM;

/// Blocks in one chunk.
pub const BLOCKS_PER_CHUNK: usize = BLOCKS_PER_EDGE * BLOCKS_PER_EDGE * BLOCKS_PER_EDGE;

/// Number of levels in a chunk: level `0` through [`ROOT_LEVEL`] inclusive.
const LEVEL_COUNT: usize = ROOT_LEVEL as usize + 1;

/// Voxels per axis covered by a node at level `l`, indexed by `l`.
///
/// `LEVEL_SIZE[0] == CHUNK_DIM`, `LEVEL_SIZE[BLOCK_LEVEL] == BLOCK_DIM` and
/// `LEVEL_SIZE[ROOT_LEVEL] == 1`; each level halves the previous one.
pub const LEVEL_SIZE: [usize; LEVEL_COUNT] = level_sizes();

const fn level_sizes() -> [usize; LEVEL_COUNT] {
    let mut sizes = [0; LEVEL_COUNT];
    let mut level = 0;
    while level < LEVEL_COUNT {
        sizes[level] = CHUNK_DIM >> level;
        level += 1;
    }
    sizes
}

/// World-space edge length of a node at `level`, in metres.
///
/// `level` must be in `0..=ROOT_LEVEL`; this is the CPU reference for the
/// implicit node AABBs the GPU traversal builds from origin and level.
pub const fn aabb_extent(level: u32) -> f32 {
    LEVEL_SIZE[level as usize] as f32 * VOXEL_SIZE
}

// The constants are a contract: every relationship below is checked at compile
// time, so a change to `CHUNK_BITS` or `BLOCK_BITS` that leaves the layout
// inconsistent fails the build instead of mis-sizing a buffer at runtime.
const _: () = {
    assert!(BLOCK_DIM == 1 << BLOCK_BITS);

    // The two frozen shapes in ADR-002 and pre-registration §2. A build must
    // select exactly one of them; the residency gate compares both by name.
    assert!(BASELINE_CHUNK_BITS == 6);
    assert!(TARGET_CHUNK_BITS == 7);
    assert!(BASELINE_CHUNK_DIM == 64);
    assert!(TARGET_CHUNK_DIM == 128);
    assert!(TARGET_CHUNK_DIM == BASELINE_CHUNK_DIM * 2);
    assert!(CHUNK_BITS == BASELINE_CHUNK_BITS || CHUNK_BITS == TARGET_CHUNK_BITS);

    assert!(CHUNK_DIM == 1 << CHUNK_BITS);
    assert!(CHUNK_VOLUME == CHUNK_DIM * CHUNK_DIM * CHUNK_DIM);

    assert!(ROOT_LEVEL == CHUNK_BITS);
    assert!(BLOCK_LEVEL == CHUNK_BITS - BLOCK_BITS);
    assert!(BLOCK_LEVEL < ROOT_LEVEL);
    assert!(CHUNK_DIM == BLOCK_DIM << BLOCK_LEVEL);
    // Both shifts and the `LEVEL_SIZE` fill must stay in range for `usize`.
    assert!(CHUNK_BITS < usize::BITS);

    assert!(BLOCKS_PER_EDGE == 1 << BLOCK_LEVEL);
    assert!(BLOCKS_PER_EDGE * BLOCK_DIM == CHUNK_DIM);
    assert!(BLOCKS_PER_CHUNK == BLOCKS_PER_EDGE * BLOCKS_PER_EDGE * BLOCKS_PER_EDGE);
    assert!(BLOCKS_PER_CHUNK * BLOCK_DIM * BLOCK_DIM * BLOCK_DIM == CHUNK_VOLUME);

    assert!(LEVEL_SIZE.len() == LEVEL_COUNT);
    assert!(LEVEL_SIZE[0] == CHUNK_DIM);
    assert!(LEVEL_SIZE[BLOCK_LEVEL as usize] == BLOCK_DIM);
    assert!(LEVEL_SIZE[ROOT_LEVEL as usize] == 1);

    let mut level = 1;
    while level < LEVEL_COUNT {
        assert!(LEVEL_SIZE[level] * 2 == LEVEL_SIZE[level - 1]);
        level += 1;
    }

    assert!(VOXEL_SIZE * BLOCK_DIM as f32 == 1.0);
    assert!(aabb_extent(0) == CHUNK_DIM as f32 * VOXEL_SIZE);
    assert!(aabb_extent(BLOCK_LEVEL) == 1.0);
    assert!(aabb_extent(ROOT_LEVEL) == VOXEL_SIZE);
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The default build is the 64³ M1 baseline. The 128³ edge is opt-in via
    /// the `chunk128` feature.
    #[cfg(not(feature = "chunk128"))]
    #[test]
    fn default_build_uses_the_64_edge() {
        assert_eq!(CHUNK_BITS, 6);
        assert_eq!(CHUNK_DIM, 64);
        assert_eq!(ROOT_LEVEL, 6);
        assert_eq!(BLOCK_LEVEL, 2);
    }

    /// The opt-in build moves every derived value to the 128³ edge.
    #[cfg(feature = "chunk128")]
    #[test]
    fn chunk128_build_uses_the_128_edge() {
        assert_eq!(CHUNK_BITS, 7);
        assert_eq!(CHUNK_DIM, 128);
        assert_eq!(ROOT_LEVEL, 7);
        assert_eq!(BLOCK_LEVEL, 3);
    }

    /// Compile-time assertions can only check what the compiler folds; this
    /// exercises the same relationships as values at runtime.
    #[test]
    fn constants_are_mutually_consistent() {
        assert_eq!(BLOCK_DIM, 1 << BLOCK_BITS);
        assert_eq!(CHUNK_DIM, 1 << CHUNK_BITS);
        assert_eq!(CHUNK_VOLUME, CHUNK_DIM.pow(3));
        assert_eq!(ROOT_LEVEL, CHUNK_BITS);
        assert_eq!(BLOCK_LEVEL, CHUNK_BITS - BLOCK_BITS);
        assert_eq!(BLOCKS_PER_CHUNK, (CHUNK_DIM / BLOCK_DIM).pow(3));
        assert_eq!(BLOCKS_PER_CHUNK * BLOCK_DIM.pow(3), CHUNK_VOLUME);
        assert_eq!(LEVEL_SIZE.len(), ROOT_LEVEL as usize + 1);
    }

    /// The two frozen edges are named explicitly so a gate can compare them
    /// from either build; the compiled edge must be one of them.
    #[test]
    fn baseline_and_target_edges_are_both_named() {
        assert_eq!(BASELINE_CHUNK_DIM, 1 << BASELINE_CHUNK_BITS);
        assert_eq!(TARGET_CHUNK_DIM, 1 << TARGET_CHUNK_BITS);
        assert_eq!(TARGET_CHUNK_DIM, BASELINE_CHUNK_DIM * 2);
        assert!(
            CHUNK_BITS == BASELINE_CHUNK_BITS || CHUNK_BITS == TARGET_CHUNK_BITS,
            "the build must select the baseline or target edge"
        );
    }

    /// The values must work in `const` items, not just at runtime: buffer
    /// sizes, band counts and shader constants are built from them.
    #[test]
    fn constants_are_usable_in_const_context() {
        const VOXELS_PER_CHUNK: usize = CHUNK_VOLUME;
        const BLOCKS: usize = BLOCKS_PER_CHUNK;
        const CHUNK_EXTENT: f32 = aabb_extent(0);
        const BLOCK_EXTENT: f32 = aabb_extent(BLOCK_LEVEL);
        const VOXEL_EXTENT: f32 = aabb_extent(ROOT_LEVEL);

        assert_eq!(VOXELS_PER_CHUNK, CHUNK_DIM.pow(3));
        assert_eq!(BLOCKS, BLOCKS_PER_CHUNK);
        assert_eq!(CHUNK_EXTENT, VOXEL_SIZE * CHUNK_DIM as f32);
        // The physical anchor: a block is one cubic metre.
        assert_eq!(BLOCK_EXTENT, 1.0);
        assert_eq!(VOXEL_EXTENT, VOXEL_SIZE);
    }

    #[test]
    fn level_sizes_halve_from_the_chunk_root() {
        assert_eq!(LEVEL_SIZE[0], CHUNK_DIM);
        assert_eq!(LEVEL_SIZE[BLOCK_LEVEL as usize], BLOCK_DIM);
        assert_eq!(LEVEL_SIZE[ROOT_LEVEL as usize], 1);

        for level in 1..LEVEL_COUNT {
            assert_eq!(LEVEL_SIZE[level], LEVEL_SIZE[level - 1] / 2);
            assert_eq!(
                aabb_extent(level as u32),
                aabb_extent(level as u32 - 1) / 2.0
            );
        }
    }

    /// The AABB reference the GPU's implicit boxes are checked against.
    #[test]
    fn aabb_extent_covers_each_level_in_metres() {
        assert_eq!(aabb_extent(0), VOXEL_SIZE * CHUNK_DIM as f32);
        assert_eq!(aabb_extent(BLOCK_LEVEL), 1.0); // 16 voxels
        assert_eq!(aabb_extent(ROOT_LEVEL), VOXEL_SIZE); // 1 voxel
    }
}
