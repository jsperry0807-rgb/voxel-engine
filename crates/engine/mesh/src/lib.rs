//! Block-granular content pipeline (M2-03).
//!
//! A 64³ or 128³ chunk is too large to copy into a padded volume for meshing:
//! at 128³ the padded copy is `130³ × 2 B = 4.39 MB`. The driver instead walks
//! the chunk one 16³ block at a time and fills a reusable `18³` scratch, adding
//! the one-voxel border the mesher and AO sampler need.
//!
//! The scratch is a fixed [`BlockScratch`]: `18³ × 2 B = 11,664 B` per worker,
//! well under the M2-03 budget of 64 KiB.

use std::mem::size_of;

use voxel_storage::{
    AIR, Apron, BLOCK_DIM, CHUNK_DIM, CHUNK_VOLUME, Face as ApronFace, local_index,
};

mod greedy;
pub use greedy::{GreedyQuad, MergeKey, corner_ao, greedy_block};

/// Voxels along one block edge.
pub const BLOCK_EDGE: usize = BLOCK_DIM;
/// Voxels along one scratch edge: a block plus a one-voxel border.
pub const SCRATCH_EDGE: usize = BLOCK_EDGE + 2;
/// Voxels in one scratch.
pub const SCRATCH_VOXELS: usize = SCRATCH_EDGE * SCRATCH_EDGE * SCRATCH_EDGE;
/// Bytes in one scratch at the raw `u16` material width.
pub const SCRATCH_BYTES: usize = SCRATCH_VOXELS * size_of::<u16>();
/// M2-03 scratch budget per worker.
pub const SCRATCH_BUDGET_BYTES: usize = 64 * 1024;
const _: () = assert!(SCRATCH_BYTES <= SCRATCH_BUDGET_BYTES);

/// A block's coordinates inside its chunk, in block units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl BlockPos {
    /// Creates a block position.
    pub const fn new(x: u32, y: u32, z: u32) -> Self {
        Self { x, y, z }
    }

    /// The chunk-local voxel coordinate of the block's minimum corner.
    pub const fn origin(self) -> [u32; 3] {
        [
            self.x * BLOCK_EDGE as u32,
            self.y * BLOCK_EDGE as u32,
            self.z * BLOCK_EDGE as u32,
        ]
    }
}

/// One reusable 18³ scratch around a 16³ block.
///
/// Local coordinate `(1, 1, 1)` is the block's minimum corner; `0` and `17`
/// are its one-voxel border. The border reads the neighbouring block inside
/// the chunk, or the matching [`Apron`] face at a chunk boundary.
///
/// Cross-edge and cross-corner voxels that lie outside two or three chunk
/// faces are not represented by the six face planes and read as air. The AO
/// seam work (M2-09) can extend the source if that matters at a chunk corner.
#[derive(Clone)]
pub struct BlockScratch {
    voxels: [u16; SCRATCH_VOXELS],
}

impl Default for BlockScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockScratch {
    /// An all-air scratch.
    pub fn new() -> Self {
        Self {
            voxels: [AIR; SCRATCH_VOXELS],
        }
    }

    /// The flat scratch buffer, in `x + y*18 + z*18*18` order.
    pub fn as_slice(&self) -> &[u16] {
        &self.voxels
    }

    /// The material at scratch-local `(x, y, z)`.
    pub fn get(&self, x: u32, y: u32, z: u32) -> u16 {
        self.voxels[index(x, y, z)]
    }

    /// Builds a scratch from an already-filled 18³ volume.
    ///
    /// The driver uses this shape internally; tests and future offline tools can
    /// use it to exercise the AO sampler without a world.
    pub fn from_voxels(voxels: [u16; SCRATCH_VOXELS]) -> Self {
        Self { voxels }
    }

    fn set(&mut self, x: usize, y: usize, z: usize, material: u16) {
        self.voxels[x + y * SCRATCH_EDGE + z * SCRATCH_EDGE * SCRATCH_EDGE] = material;
    }
}

/// Scratch index for scratch-local `(x, y, z)`.
pub const fn index(x: u32, y: u32, z: u32) -> usize {
    x as usize + y as usize * SCRATCH_EDGE + z as usize * SCRATCH_EDGE * SCRATCH_EDGE
}

/// Visits every 16³ block in `chunk` with its 18³ scratch.
///
/// `chunk` is the resident chunk volume in `local_index` order. `apron` is the
/// six-face apron from [`voxel_storage::world::World::apron`]. The driver
/// allocates no chunk-wide buffer: it owns exactly one [`BlockScratch`] and
/// reuses it for every block.
///
/// The callback receives an immutable scratch; M2-04's greedy mesher is the
/// first consumer that turns it into quads.
pub fn for_each_block<F>(chunk: &[u16], apron: &Apron, mut visit: F)
where
    F: FnMut(BlockPos, &BlockScratch),
{
    assert_eq!(
        chunk.len(),
        CHUNK_VOLUME,
        "chunk must contain exactly CHUNK_VOLUME voxels"
    );
    let chunk_edge = CHUNK_DIM as i32;
    let blocks = chunk_edge / BLOCK_EDGE as i32;
    let mut scratch = BlockScratch::new();

    for bz in 0..blocks {
        for by in 0..blocks {
            for bx in 0..blocks {
                fill_scratch(chunk, apron, bx, by, bz, &mut scratch);
                visit(BlockPos::new(bx as u32, by as u32, bz as u32), &scratch);
            }
        }
    }
}

fn fill_scratch(
    chunk: &[u16],
    apron: &Apron,
    bx: i32,
    by: i32,
    bz: i32,
    scratch: &mut BlockScratch,
) {
    let chunk_edge = CHUNK_DIM as i32;
    let origin = [
        bx * BLOCK_EDGE as i32,
        by * BLOCK_EDGE as i32,
        bz * BLOCK_EDGE as i32,
    ];
    for sz in 0..SCRATCH_EDGE {
        for sy in 0..SCRATCH_EDGE {
            for sx in 0..SCRATCH_EDGE {
                let x = origin[0] + sx as i32 - 1;
                let y = origin[1] + sy as i32 - 1;
                let z = origin[2] + sz as i32 - 1;
                let material = if (0..chunk_edge).contains(&x)
                    && (0..chunk_edge).contains(&y)
                    && (0..chunk_edge).contains(&z)
                {
                    chunk[local_index(x as u32, y as u32, z as u32) as usize]
                } else {
                    apron_material(apron, x, y, z, chunk_edge)
                };
                scratch.set(sx, sy, sz, material);
            }
        }
    }
}

/// Reads an outside voxel from the one face plane that contains it.
///
/// A voxel outside two or three chunk faces is a cross-edge/cross-corner
/// neighbour; the apron does not carry it, so it reads as air.
fn apron_material(apron: &Apron, x: i32, y: i32, z: i32, edge: i32) -> u16 {
    let ox = x < 0 || x >= edge;
    let oy = y < 0 || y >= edge;
    let oz = z < 0 || z >= edge;
    if u8::from(ox) + u8::from(oy) + u8::from(oz) != 1 {
        return AIR;
    }

    let (face, u, v) = if x == -1 {
        (ApronFace::NegX, z, y)
    } else if x == edge {
        (ApronFace::PosX, z, y)
    } else if y == -1 {
        (ApronFace::NegY, z, x)
    } else if y == edge {
        (ApronFace::PosY, z, x)
    } else if z == -1 {
        (ApronFace::NegZ, x, y)
    } else {
        (ApronFace::PosZ, x, y)
    };
    apron.get(face, u as u32, v as u32)
}

#[cfg(test)]
mod tests {
    use std::mem::size_of_val;

    use super::*;

    #[test]
    fn scratch_budget() {
        assert_eq!(
            size_of::<BlockScratch>(),
            SCRATCH_BYTES,
            "BlockScratch must be exactly the advertised bytes"
        );
        let chunk = noise_chunk();
        let apron = Apron::new();
        let mut peak = 0;
        for_each_block(&chunk, &apron, |_, scratch| {
            peak = peak.max(size_of_val(scratch.as_slice()));
        });
        assert_eq!(peak, SCRATCH_BYTES);
        assert!(peak <= SCRATCH_BUDGET_BYTES);
    }

    #[test]
    fn visits_every_block_and_matches_the_chunk_interior() {
        let chunk = noise_chunk();
        let apron = Apron::new();
        let blocks = (CHUNK_DIM / BLOCK_EDGE) as u32;
        let mut visited = 0;

        for_each_block(&chunk, &apron, |pos, scratch| {
            visited += 1;
            let origin = pos.origin();
            for z in 1..=BLOCK_EDGE as u32 {
                for y in 1..=BLOCK_EDGE as u32 {
                    for x in 1..=BLOCK_EDGE as u32 {
                        let expected = chunk[local_index(
                            origin[0] + x - 1,
                            origin[1] + y - 1,
                            origin[2] + z - 1,
                        ) as usize];
                        assert_eq!(
                            scratch.get(x, y, z),
                            expected,
                            "block {pos:?} local ({x}, {y}, {z})"
                        );
                    }
                }
            }
        });

        assert_eq!(visited, blocks * blocks * blocks);
    }

    #[test]
    fn border_reads_the_matching_apron_face() {
        let chunk = noise_chunk();
        let mut apron = Apron::new();
        for face in ApronFace::ALL {
            for v in 0..voxel_storage::Apron::SIDE {
                for u in 0..voxel_storage::Apron::SIDE {
                    apron.set(face, u, v, 100 + face.index() as u16);
                }
            }
        }

        let blocks = (CHUNK_DIM / BLOCK_EDGE) as u32;
        for_each_block(&chunk, &apron, |pos, scratch| {
            // The x = -1 border of the first block.
            if pos.x == 0 {
                for z in 1..=BLOCK_EDGE as u32 {
                    for y in 1..=BLOCK_EDGE as u32 {
                        assert_eq!(
                            scratch.get(0, y, z),
                            100 + ApronFace::NegX.index() as u16,
                            "NegX border at {pos:?} ({y}, {z})"
                        );
                    }
                }
            }
            // The x = CHUNK_DIM border of the last block.
            if pos.x + 1 == blocks {
                for z in 1..=BLOCK_EDGE as u32 {
                    for y in 1..=BLOCK_EDGE as u32 {
                        assert_eq!(
                            scratch.get(SCRATCH_EDGE as u32 - 1, y, z),
                            100 + ApronFace::PosX.index() as u16,
                            "PosX border at {pos:?} ({y}, {z})"
                        );
                    }
                }
            }
        });
    }

    fn noise_chunk() -> Vec<u16> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        (0..CHUNK_VOLUME)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 7) as u16
            })
            .collect()
    }
}
