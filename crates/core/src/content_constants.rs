//! Light, probe and occlusion budgets (spec §4.2 and §7).
//!
//! Three per-chunk content volumes share the block grid:
//!
//! - **Direct light** is one 16 B cell per block, so a chunk holds
//!   [`LIGHT_CELLS_PER_CHUNK`] cells and [`LIGHT_BYTES_PER_CHUNK`] bytes.
//! - **Probes** are one 12 B SH4 RGB probe per block at the near LOD, so the
//!   near-residency grid costs [`PROBE_BYTES_PER_CHUNK`] bytes. The mid ring is
//!   half resolution and far chunks carry no probes at all.
//! - **Occlusion** is one visibility bit per block, packed into
//!   [`BLOCK_OCCLUSION_BYTES`] bytes per chunk.
//!
//! All three are content-timeline volumes: they are rebuilt when a chunk's
//! blocks change, never per frame. Their sizes therefore follow
//! [`CHUNK_BITS`](crate::constants::CHUNK_BITS) exactly.
//!
//! # Probe rings
//!
//! Probe density falls with distance from the camera in two rings:
//!
//! | Ring | Radius | Spacing | Storage |
//! | --- | ---: | ---: | --- |
//! | Near | [`PROBE_NEAR_RADIUS_M`] | 1 m | per block |
//! | Mid | [`PROBE_MID_RADIUS_M`] | 2 m | half resolution |
//! | Far | beyond mid | — | none |
//!
//! The radii are tied to the default 128/256/512 m LOD rings: the near ring
//! covers LOD0, the mid ring covers LOD1, and the far ring (LOD2) carries no
//! probes at all. Past the mid ring shading uses direct light and vertex AO
//! only.
//!
//! # Example
//!
//! ```
//! use voxel_core::constants::BLOCKS_PER_CHUNK;
//! use voxel_core::content_constants::{
//!     BLOCK_OCCLUSION_BYTES, LIGHT_BYTES_PER_CELL, LIGHT_BYTES_PER_CHUNK, PROBE_BYTES,
//!     PROBE_BYTES_PER_CHUNK,
//! };
//!
//! // One cell per block: 1 KiB light, 768 B probes and 8 B occlusion at 64³
//! // (8 KiB, 6 KiB and 64 B at the pre-registered 128³ edge).
//! assert_eq!(LIGHT_BYTES_PER_CHUNK, BLOCKS_PER_CHUNK * LIGHT_BYTES_PER_CELL);
//! assert_eq!(PROBE_BYTES_PER_CHUNK, BLOCKS_PER_CHUNK * PROBE_BYTES);
//! assert_eq!(BLOCK_OCCLUSION_BYTES, BLOCKS_PER_CHUNK.div_ceil(8));
//! ```

use crate::constants::{BLOCK_DIM, BLOCK_LEVEL, BLOCKS_PER_CHUNK, CHUNK_DIM, aabb_extent};

/// Voxels per edge of one light cell.
///
/// One cell per block, matching the mesher's 16³ block granularity.
pub const LIGHT_CELL_VOXELS: usize = BLOCK_DIM;

/// Light cells per chunk edge.
pub const LIGHT_CELLS_PER_EDGE: usize = CHUNK_DIM / LIGHT_CELL_VOXELS;

/// Light cells in one chunk: one per block.
pub const LIGHT_CELLS_PER_CHUNK: usize =
    LIGHT_CELLS_PER_EDGE * LIGHT_CELLS_PER_EDGE * LIGHT_CELLS_PER_EDGE;

/// Bytes per light cell: RGB plus intensity, or an SH2 equivalent.
pub const LIGHT_BYTES_PER_CELL: usize = 16;

/// Light storage for one chunk at the current [`CHUNK_BITS`].
pub const LIGHT_BYTES_PER_CHUNK: usize = LIGHT_CELLS_PER_CHUNK * LIGHT_BYTES_PER_CELL;

/// Width of the light apron, in blocks.
///
/// Propagation and AO sampling read one block beyond each chunk face so
/// neighbouring chunks agree at their borders.
pub const LIGHT_APRON_BLOCKS: usize = 1;

/// Bytes per near-LOD probe: SH4 RGB8.
pub const PROBE_BYTES: usize = 12;

/// Probe spacing in the near ring, in blocks (1 m at the gameplay scale).
pub const PROBE_NEAR_SPACING_M: u32 = 1;

/// Probe spacing in the mid ring, in blocks: half the near resolution.
pub const PROBE_MID_SPACING_M: u32 = 2;

/// Near-LOD probes per chunk: one per block.
pub const PROBES_PER_CHUNK: usize = BLOCKS_PER_CHUNK;

/// Near-LOD probe storage for one chunk at the current [`CHUNK_BITS`].
///
/// The mid ring stores one probe per 2 m cell on each axis, i.e. one eighth of
/// this, and far chunks store none.
pub const PROBE_BYTES_PER_CHUNK: usize = PROBES_PER_CHUNK * PROBE_BYTES;

/// Outer radius of the per-block probe ring, in metres.
///
/// Matches the default LOD0 ring: every LOD0 chunk has per-block probes.
pub const PROBE_NEAR_RADIUS_M: u32 = 128;

/// Outer radius of the half-resolution probe ring, in metres.
///
/// Matches the default LOD1 ring; LOD2 and beyond have no probes.
pub const PROBE_MID_RADIUS_M: u32 = 256;

/// Probe ring boundaries, ordered outward.
///
/// A chunk at distance `r` from the camera uses the first ring whose radius is
/// at least `r`; past the last entry the chunk has no probe volume.
pub const PROBE_RING_RADII_M: [u32; 2] = [PROBE_NEAR_RADIUS_M, PROBE_MID_RADIUS_M];

/// Bounded occlusion horizon, in metres.
///
/// The block flood-fill is seeded at the camera and stops here; beyond it the
/// traversal is frustum-only. The horizon keeps the fill at `O(horizon³)`.
pub const OCCLUSION_HORIZON_M: u32 = 128;

/// Occlusion horizon in blocks.
///
/// A gameplay block is one metre on an edge, so this equals
/// [`OCCLUSION_HORIZON_M`] as a count. It is what the flood-fill iterates over.
pub const OCCLUSION_HORIZON_BLOCKS: usize = OCCLUSION_HORIZON_M as usize;

/// Visibility bits per block in the occlusion fill.
pub const BLOCK_OCCLUSION_BITS_PER_BLOCK: usize = 1;

/// Packed block-visibility bits in one chunk.
pub const BLOCK_OCCLUSION_BITS: usize = BLOCKS_PER_CHUNK * BLOCK_OCCLUSION_BITS_PER_BLOCK;

/// Packed block-visibility storage for one chunk at the current [`CHUNK_BITS`].
///
/// 8 B at 64³ and 64 B at 128³. The fill always covers the chunk, so the
/// ceiling is exact for every supported edge.
pub const BLOCK_OCCLUSION_BYTES: usize = BLOCK_OCCLUSION_BITS.div_ceil(8);

// These sizes are a contract with the light, probe and occlusion passes. The
// spec fixes the per-cell widths and the horizon; the per-chunk counts must
// follow the block grid so that a `CHUNK_BITS` change re-sizes every buffer.
const _: () = {
    assert!(LIGHT_CELL_VOXELS == 16);
    assert!(LIGHT_CELL_VOXELS == BLOCK_DIM);
    assert!(LIGHT_CELLS_PER_EDGE == 1 << BLOCK_LEVEL);
    assert!(LIGHT_CELLS_PER_CHUNK == BLOCKS_PER_CHUNK);
    assert!(LIGHT_BYTES_PER_CELL == 16);
    assert!(LIGHT_BYTES_PER_CHUNK == LIGHT_CELLS_PER_CHUNK * 16);
    assert!(LIGHT_APRON_BLOCKS == 1);

    assert!(PROBE_BYTES == 12);
    assert!(PROBE_NEAR_SPACING_M == 1);
    assert!(PROBE_MID_SPACING_M == PROBE_NEAR_SPACING_M * 2);
    assert!(PROBES_PER_CHUNK == BLOCKS_PER_CHUNK);
    assert!(PROBE_BYTES_PER_CHUNK == PROBES_PER_CHUNK * 12);
    assert!(PROBE_NEAR_RADIUS_M < PROBE_MID_RADIUS_M);
    assert!(PROBE_RING_RADII_M[0] == PROBE_NEAR_RADIUS_M);
    assert!(PROBE_RING_RADII_M[1] == PROBE_MID_RADIUS_M);

    assert!(OCCLUSION_HORIZON_M == 128);
    // The horizon and the ring radii are world metres; a block is one metre.
    assert!(aabb_extent(BLOCK_LEVEL) == 1.0);
    assert!(OCCLUSION_HORIZON_BLOCKS == OCCLUSION_HORIZON_M as usize);
    assert!(BLOCK_OCCLUSION_BITS_PER_BLOCK == 1);
    assert!(BLOCK_OCCLUSION_BITS == BLOCKS_PER_CHUNK);
    assert!(BLOCK_OCCLUSION_BITS.is_multiple_of(8));
    assert!(BLOCK_OCCLUSION_BYTES == BLOCK_OCCLUSION_BITS / 8);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{BLOCK_BITS, CHUNK_BITS};

    /// Blocks in a chunk at an arbitrary edge, so both the 64³ baseline and the
    /// pre-registered 128³ target can be checked from one test run.
    const fn blocks_per_chunk_at(chunk_bits: u32) -> usize {
        let per_edge = 1usize << (chunk_bits - BLOCK_BITS);
        per_edge * per_edge * per_edge
    }

    const fn light_bytes_at(chunk_bits: u32) -> usize {
        blocks_per_chunk_at(chunk_bits) * LIGHT_BYTES_PER_CELL
    }

    const fn probe_bytes_at(chunk_bits: u32) -> usize {
        blocks_per_chunk_at(chunk_bits) * PROBE_BYTES
    }

    const fn occlusion_bytes_at(chunk_bits: u32) -> usize {
        (blocks_per_chunk_at(chunk_bits) * BLOCK_OCCLUSION_BITS_PER_BLOCK).div_ceil(8)
    }

    #[test]
    fn content_constants_lock_the_spec_values() {
        assert_eq!(LIGHT_CELL_VOXELS, 16);
        assert_eq!(LIGHT_BYTES_PER_CELL, 16);
        assert_eq!(PROBE_BYTES, 12);
        assert_eq!(PROBE_NEAR_RADIUS_M, 128);
        assert_eq!(PROBE_MID_RADIUS_M, 256);
        assert_eq!(PROBE_RING_RADII_M, [128, 256]);
        assert_eq!(OCCLUSION_HORIZON_M, 128);
        assert_eq!(OCCLUSION_HORIZON_BLOCKS, 128);
        assert_eq!(BLOCK_OCCLUSION_BYTES, occlusion_bytes_at(CHUNK_BITS));
    }

    /// Light is one cell per block, so the chunk counts follow the block grid.
    #[test]
    fn light_sizes_derive_from_the_block_grid() {
        assert_eq!(
            LIGHT_CELLS_PER_EDGE * LIGHT_CELLS_PER_EDGE * LIGHT_CELLS_PER_EDGE,
            BLOCKS_PER_CHUNK
        );
        assert_eq!(LIGHT_CELLS_PER_CHUNK, BLOCKS_PER_CHUNK);
        assert_eq!(LIGHT_CELLS_PER_CHUNK, blocks_per_chunk_at(CHUNK_BITS));
        assert_eq!(LIGHT_BYTES_PER_CHUNK, light_bytes_at(CHUNK_BITS));

        // Spec §7: 64 cells = 1 KiB at 64³, 512 cells = 8 KiB at 128³.
        assert_eq!(blocks_per_chunk_at(6), 64);
        assert_eq!(light_bytes_at(6), 1 << 10);
        assert_eq!(blocks_per_chunk_at(7), 512);
        assert_eq!(light_bytes_at(7), 1 << 13);
    }

    /// Probes are 12 B per block at the near LOD, one eighth of that at mid.
    #[test]
    fn probe_sizes_derive_from_the_near_grid() {
        assert_eq!(PROBES_PER_CHUNK, BLOCKS_PER_CHUNK);
        assert_eq!(PROBES_PER_CHUNK, blocks_per_chunk_at(CHUNK_BITS));
        assert_eq!(PROBE_BYTES_PER_CHUNK, probe_bytes_at(CHUNK_BITS));

        // Spec §7: 768 B at 64³, 6 KiB at 128³.
        assert_eq!(probe_bytes_at(6), 768);
        assert_eq!(probe_bytes_at(7), 6 << 10);

        // Half resolution on each axis is one eighth of the storage.
        assert_eq!(PROBE_MID_SPACING_M, 2 * PROBE_NEAR_SPACING_M);
        assert_eq!(probe_bytes_at(6) / 8, 96);
    }

    #[test]
    fn probe_rings_are_ordered_and_bound_the_default_lod_rings() {
        assert_eq!(PROBE_NEAR_RADIUS_M, 128);
        assert_eq!(PROBE_MID_RADIUS_M, 256);
        assert_eq!(
            PROBE_RING_RADII_M,
            [PROBE_NEAR_RADIUS_M, PROBE_MID_RADIUS_M]
        );
        // Radii are whole metres; the block is the flood-fill and probe unit.
        assert!(PROBE_NEAR_RADIUS_M.is_multiple_of(BLOCK_DIM as u32));
        assert!(PROBE_MID_RADIUS_M.is_multiple_of(BLOCK_DIM as u32));

        for pair in PROBE_RING_RADII_M.windows(2) {
            assert!(pair[0] < pair[1], "probe rings must grow outward");
        }
    }

    /// Occlusion is one bit per block and covers the whole chunk.
    #[test]
    fn occlusion_bytes_are_one_bit_per_block() {
        assert_eq!(BLOCK_OCCLUSION_BITS_PER_BLOCK, 1);
        assert_eq!(BLOCK_OCCLUSION_BITS, BLOCKS_PER_CHUNK);
        assert_eq!(
            BLOCK_OCCLUSION_BYTES,
            occlusion_bytes_at(CHUNK_BITS),
            "current edge must match the derived size"
        );

        // Spec §4.2: 8 B at 64³, 64 B at 128³.
        assert_eq!(occlusion_bytes_at(6), 8);
        assert_eq!(occlusion_bytes_at(7), 64);

        // Bits pack exactly, even at the pre-registered edge.
        assert!(blocks_per_chunk_at(7).is_multiple_of(8));
        assert_eq!(BLOCK_OCCLUSION_BYTES * 8, BLOCK_OCCLUSION_BITS);
    }

    /// The constants are consumed by buffer-size expressions, so they have to
    /// work in a `const` context as well as in tests.
    #[test]
    fn content_constants_are_usable_in_const_context() {
        const LIGHT_SCRATCH: usize = LIGHT_CELLS_PER_CHUNK * LIGHT_BYTES_PER_CELL;
        const PROBE_SCRATCH: usize = PROBES_PER_CHUNK * PROBE_BYTES;
        const OCCLUSION_WORDS: usize = BLOCK_OCCLUSION_BITS / u32::BITS as usize;
        const HORIZON_SIDE: usize = OCCLUSION_HORIZON_BLOCKS;

        assert_eq!(LIGHT_SCRATCH, LIGHT_BYTES_PER_CHUNK);
        assert_eq!(PROBE_SCRATCH, PROBE_BYTES_PER_CHUNK);
        assert_eq!(OCCLUSION_WORDS * size_of::<u32>(), BLOCK_OCCLUSION_BYTES);
        assert_eq!(HORIZON_SIDE, OCCLUSION_HORIZON_M as usize);
    }
}
