//! Six 1-voxel face planes around a chunk (M2-01).
//!
//! The mesher needs the voxels just outside a chunk's six faces to cull
//! backfaces at the border without densifying the chunk or its neighbours.
//! [`Apron`] carries those `6 x CHUNK_DIM²` [`MaterialId`]s as raw `u16`s,
//! overlay first, exactly as [`World::get`] reports them.
//!
//! Each face plane comes from exactly one chunk: the neighbour sharing that
//! face. A neighbour that is not resident contributes [`AIR`]; load the six
//! face neighbours first when a border must be exact.
//!
//! A plane is exact for culling and for ambient occlusion of faces whose
//! normal matches that plane. AO samples at an edge or corner belong to an
//! edge or corner neighbour instead, so they read as [`AIR`] here; load those
//! neighbours too when corner-exact AO is required.

use crate::gpu::GpuSink;
use crate::world::World;
use crate::{AIR, CHUNK_DIM, ChunkPos, local_index};
use voxel_block::MaterialId;

/// Side length of one face plane, in voxels.
pub const SIDE: u32 = CHUNK_DIM as u32;
/// Voxels in one face plane.
pub const PLANE: usize = SIDE as usize * SIDE as usize;
/// Voxels in the whole apron.
const LEN: usize = Face::ALL.len() * PLANE;

/// One face of a chunk's 1-voxel apron.
///
/// Plane texel `(u, v)` maps to an offset from the chunk origin as
/// [`Face::offset`] documents: `X` faces use `u = z, v = y`, `Y` faces use
/// `u = z, v = x`, and `Z` faces use `u = x, v = y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Face {
    /// The plane at `x = -1`.
    NegX,
    /// The plane at `x = CHUNK_DIM`.
    PosX,
    /// The plane at `y = -1`.
    NegY,
    /// The plane at `y = CHUNK_DIM`.
    PosY,
    /// The plane at `z = -1`.
    NegZ,
    /// The plane at `z = CHUNK_DIM`.
    PosZ,
}

impl Face {
    /// Every face, in [`Face::index`] order.
    pub const ALL: [Self; 6] = [
        Self::NegX,
        Self::PosX,
        Self::NegY,
        Self::PosY,
        Self::NegZ,
        Self::PosZ,
    ];

    /// Index into the six planes.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The offset from the chunk origin of plane texel `(u, v)`.
    ///
    /// Exactly one component is outside `0..CHUNK_DIM`; the other two are the
    /// texel's coordinates on the plane. The result is therefore a coordinate
    /// in the `(CHUNK_DIM + 2)³` padded volume, offset by one.
    ///
    /// # Panics
    ///
    /// Panics if `u` or `v` is not below [`SIDE`] (debug builds only).
    pub fn offset(self, u: u32, v: u32) -> (i32, i32, i32) {
        debug_assert!(
            u < SIDE && v < SIDE,
            "apron texel ({u}, {v}) is outside {SIDE}²"
        );
        let edge = CHUNK_DIM as i32;
        let u = u as i32;
        let v = v as i32;
        match self {
            Self::NegX => (-1, v, u),
            Self::PosX => (edge, v, u),
            Self::NegY => (v, -1, u),
            Self::PosY => (v, edge, u),
            Self::NegZ => (u, v, -1),
            Self::PosZ => (u, v, edge),
        }
    }
}

/// The six 1-voxel face planes around one chunk.
///
/// The planes are stored face by face, row-major in `(u, v)`: the texel at
/// `(u, v)` is at index `u + v * SIDE` inside its [`Apron::face`] slice.
#[derive(Clone, PartialEq, Eq)]
pub struct Apron {
    planes: Vec<u16>,
}

impl Apron {
    /// Side length of one face plane, in voxels.
    pub const SIDE: u32 = crate::apron::SIDE;
    /// Voxels in one face plane.
    pub const PLANE: usize = crate::apron::PLANE;

    /// An all-[`AIR`] apron, used for missing neighbours.
    pub fn new() -> Self {
        Self {
            planes: vec![AIR; LEN],
        }
    }

    /// One face plane, `PLANE` entries long.
    #[must_use]
    pub fn face(&self, face: Face) -> &[u16] {
        let start = face.index() * PLANE;
        &self.planes[start..start + PLANE]
    }

    /// The material at plane texel `(u, v)`.
    ///
    /// # Panics
    ///
    /// Panics if `u` or `v` is not below [`SIDE`].
    #[must_use]
    pub fn get(&self, face: Face, u: u32, v: u32) -> u16 {
        self.planes[index(face, u, v)]
    }

    /// The material at plane texel `(u, v)`, as a typed id.
    ///
    /// # Panics
    ///
    /// Panics if `u` or `v` is not below [`SIDE`].
    #[must_use]
    pub fn material(&self, face: Face, u: u32, v: u32) -> MaterialId {
        MaterialId::from_raw(self.get(face, u, v))
    }

    /// Replaces the material at plane texel `(u, v)`.
    ///
    /// # Panics
    ///
    /// Panics if `u` or `v` is not below [`SIDE`].
    pub fn set(&mut self, face: Face, u: u32, v: u32, material: u16) {
        let index = index(face, u, v);
        self.planes[index] = material;
    }

    /// Reads all six planes around `pos` into a fresh apron.
    pub(crate) fn gather<S: GpuSink>(world: &World<S>, pos: ChunkPos) -> Self {
        let mut apron = Self::new();
        Self::gather_into(world, pos, &mut apron);
        apron
    }

    /// Refills `out` with the six planes around `pos`, reusing its allocation.
    ///
    /// Every plane is reset to [`AIR`] first, so a reused apron never leaks the
    /// previous chunk's border. Keep one `Apron` per mesher to avoid a
    /// `6 x SIDE²` allocation on every chunk.
    pub(crate) fn gather_into<S: GpuSink>(world: &World<S>, pos: ChunkPos, out: &mut Apron) {
        out.planes.fill(AIR);
        let dim = CHUNK_DIM as i32;
        for face in Face::ALL {
            // Only the fixed component is non-zero for texel (0, 0), so it
            // names the neighbour chunk for the whole plane.
            let (ox, oy, oz) = face.offset(0, 0);
            let neighbour = ChunkPos::new(
                pos.x + ox.div_euclid(dim),
                pos.y + oy.div_euclid(dim),
                pos.z + oz.div_euclid(dim),
            );
            let Some(chunk) = world.chunk(neighbour) else {
                continue;
            };
            let plane = out.plane_mut(face);
            for v in 0..SIDE {
                for u in 0..SIDE {
                    let (dx, dy, dz) = face.offset(u, v);
                    let (x, y, z) = (
                        dx.rem_euclid(dim) as u32,
                        dy.rem_euclid(dim) as u32,
                        dz.rem_euclid(dim) as u32,
                    );
                    let local = local_index(x, y, z);
                    plane[(u + v * SIDE) as usize] = chunk.material(local);
                }
            }
        }
    }

    fn plane_mut(&mut self, face: Face) -> &mut [u16] {
        let start = face.index() * PLANE;
        &mut self.planes[start..start + PLANE]
    }
}

impl Default for Apron {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Apron {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Apron")
            .field("side", &SIDE)
            .field("faces", &Face::ALL.len())
            .finish_non_exhaustive()
    }
}

#[track_caller]
fn index(face: Face, u: u32, v: u32) -> usize {
    assert!(
        u < SIDE && v < SIDE,
        "apron texel ({u}, {v}) is outside {SIDE}²"
    );
    face.index() * PLANE + (u + v * SIDE) as usize
}

#[cfg(test)]
mod tests {
    use super::{Apron, Face, SIDE};
    use crate::CHUNK_DIM;

    #[test]
    fn offsets_stay_inside_the_padded_volume() {
        for face in Face::ALL {
            for v in 0..SIDE {
                for u in 0..SIDE {
                    let (x, y, z) = face.offset(u, v);
                    assert!(x >= -1 && x <= CHUNK_DIM as i32);
                    assert!(y >= -1 && y <= CHUNK_DIM as i32);
                    assert!(z >= -1 && z <= CHUNK_DIM as i32);
                    let outside = [x, y, z]
                        .iter()
                        .filter(|axis| **axis == -1 || **axis == CHUNK_DIM as i32)
                        .count();
                    assert_eq!(outside, 1, "{face:?} ({u}, {v}) must cross one face");
                }
            }
        }
    }

    #[test]
    fn set_and_get_round_trip_every_face() {
        let mut apron = Apron::new();
        for face in Face::ALL {
            assert!(apron.face(face).iter().all(|&material| material == 0));
            apron.set(face, 0, 0, 7 + face.index() as u16);
            apron.set(face, SIDE - 1, SIDE - 1, 11);
            assert_eq!(apron.get(face, 0, 0), 7 + face.index() as u16);
            assert_eq!(apron.get(face, SIDE - 1, SIDE - 1), 11);
        }
        assert_eq!(apron.face(Face::PosZ).len(), super::PLANE);
    }
}
