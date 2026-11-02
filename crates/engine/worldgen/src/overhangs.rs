//! Occasional overhangs and arches from a 3D density perturbation.
//!
//! The 2D heightfield is a single threshold per column, so it can never produce
//! an overhang: if the top block is solid, every block below it is solid too.
//! This module adds a small **3D density** term near the surface:
//!
//! ```text
//! density(x, y, z) = (surface(x, z) − y) + A · noise3(x, y, z)
//! solid            = density > 0
//! ```
//!
//! The vertical noise wavelength (10 m) is much shorter than the amplitude
//! (20 m), so the density is not monotone in `y`. A positive noise lobe can
//! hold solid rock above a negative lobe, which is an overhang.
//!
//! A second 3D component adds the occasional **arch**: a cell-hash span above
//! the local surface with a leg at each end. It is a deterministic density
//! addition, not a new material, and the two legs guarantee that the span is
//! supported. The noise perturbation is low-frequency horizontally (64 m), so
//! overhangs are occasional rather than a shredded surface.
//!
//! # No floating rock
//!
//! A continuous density threshold can still leave a shelf that is not attached
//! to anything. [`prune_unsupported`] removes every solid cell that is not
//! 6-connected to the ground (the bottom layer) or to a side boundary (the
//! terrain continues outside the sampled box). The chunk builder runs it after
//! the surface and overhang fill, so a shelf either connects to the massif or
//! disappears; it never floats.
//!
//! # Determinism
//!
//! One seeded 3D noise stream and integer arithmetic. The density is a pure
//! function of `(seed, x, y, z, surface)`, so two workers and two machines
//! produce the same shelf and the same prune result.

use crate::fixed::{self, Coord, ONE};
use crate::hash;
use crate::noise::Noise;

/// Horizontal wavelength of the overhang perturbation, in metres.
pub const OVERHANG_WAVELENGTH_XZ_M: i64 = 64;

/// Vertical wavelength of the overhang perturbation, in metres.
///
/// Much shorter than the amplitude, which is what makes the density
/// non-monotone in `y` and therefore able to hold a shelf over air.
pub const OVERHANG_WAVELENGTH_Y_M: i64 = 10;

/// Maximum rise or fall of the perturbed surface, in metres.
pub const OVERHANG_AMPLITUDE_M: i32 = 20;

/// Horizontal cell edge for the occasional arch, in metres.
pub const ARCH_CELL_M: i64 = 64;

/// Share of arch cells that contain an arch, in percent.
pub const ARCH_PERCENT: u64 = 3;

const OVERHANG_SALT: u64 = 0x4F56_4552_0001;
const ARCH_SALT: u64 = 0x4F56_4552_0002;

/// Q16.16 scale that maps a Q16.16 metre coordinate into noise space.
const fn scale_m(wavelength_m: i64) -> i32 {
    (ONE as i64 / wavelength_m) as i32
}

/// The seeded 3D density perturbation.
#[derive(Clone, Copy, Debug)]
pub struct OverhangField {
    seed: u64,
    noise: Noise,
}

impl OverhangField {
    /// Create a field from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self {
            seed,
            noise: Noise::new(seed ^ OVERHANG_SALT),
        }
    }

    /// The seed this field was created with.
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Signed density at a voxel, Q16.16; positive is solid.
    ///
    /// `surface` and `y` are Q16.16 metres. The caller may sample anywhere;
    /// values far below the surface are strongly positive and far above are
    /// strongly negative.
    pub fn density(&self, x: Coord, y: Coord, z: Coord, surface: Coord) -> i32 {
        let perturbation = self.noise.value_3d(
            fixed::coord_scale(x, scale_m(OVERHANG_WAVELENGTH_XZ_M)),
            fixed::coord_scale(y, scale_m(OVERHANG_WAVELENGTH_Y_M)),
            fixed::coord_scale(z, scale_m(OVERHANG_WAVELENGTH_XZ_M)),
        );
        let rise = fixed::mul(perturbation, OVERHANG_AMPLITUDE_M << 16);
        let base = (surface - y) as i32;
        base + rise
    }

    /// True when the base density is positive or an arch occupies the voxel.
    pub fn solid(&self, x: Coord, y: Coord, z: Coord, surface: Coord) -> bool {
        self.density(x, y, z, surface) > 0 || self.arch_solid(x, y, z, surface)
    }

    /// True when a cell-hash arch occupies this voxel.
    ///
    /// An arch is a solid span above the local surface with a leg at each
    /// end. The span is supported by both legs, so it can never be an
    /// unsupported island; the air under it is the arch opening. The
    /// 3 × 3 cell check keeps the horizontal footprint stable across a chunk
    /// border, exactly like a cave shaft.
    fn arch_solid(&self, x: Coord, y: Coord, z: Coord, surface: Coord) -> bool {
        let xm = x >> 16;
        let zm = z >> 16;
        let cell_x = xm.div_euclid(ARCH_CELL_M);
        let cell_z = zm.div_euclid(ARCH_CELL_M);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let h = hash::hash2(self.seed ^ ARCH_SALT, cell_x + dx, cell_z + dz);
                if h % 100 >= ARCH_PERCENT {
                    continue;
                }
                let centre_x = (cell_x + dx) * ARCH_CELL_M + 8 + ((h >> 8) % 48) as i64;
                let centre_z = (cell_z + dz) * ARCH_CELL_M + 8 + ((h >> 16) % 48) as i64;
                let along_x = (h >> 24) & 1 == 0;
                let half = 5 + ((h >> 25) % 5) as i64; // 5..=9 m
                let height = 6 + ((h >> 29) % 4) as i64; // 6..=9 m
                let thickness = 2 + ((h >> 33) % 2) as i64; // 2..=3 m
                let spread = 1 + ((h >> 35) % 2) as i64; // 1..=2 m

                let (along, across) = if along_x {
                    (xm - centre_x, zm - centre_z)
                } else {
                    (zm - centre_z, xm - centre_x)
                };
                if along.abs() > half || across.abs() > spread {
                    continue;
                }

                // The span rises one metre toward its centre, so it reads as
                // an arch rather than a flat beam.
                let rise = ((half - along.abs()) / 3) as i32;
                let bottom = surface + ((((height - thickness) as i32 + rise) as i64) << 16);
                let top = bottom + (thickness << 16);
                if y >= bottom && y < top {
                    return true;
                }
                // A leg at each end reaches the lowest perturbed terrain, so
                // the span is supported even where the base density dips.
                let leg_bottom = surface - ((OVERHANG_AMPLITUDE_M as i64) << 16);
                if along.abs() >= half - 1 && y >= leg_bottom && y < bottom {
                    return true;
                }
            }
        }
        false
    }

    /// True for a solid voxel with air directly below it.
    ///
    /// That is the signature of an overhang shelf. The caller supplies the
    /// same surface for both samples; the density is the only state.
    pub fn is_overhang(&self, x: Coord, y: Coord, z: Coord, surface: Coord) -> bool {
        self.solid(x, y, z, surface) && !self.solid(x, y - (ONE as Coord), z, surface)
    }
}

/// Remove solid cells that are not connected to ground or a side boundary.
///
/// `solid` is indexed `x + nx * (y + ny * z)` (x fastest, then y, then z).
/// Seeds are the solid cells on `y = 0` and on the four vertical side faces;
/// a side is treated as supported because the terrain continues outside the
/// sampled box. Every remaining solid cell must be 6-connected to a seed.
///
/// Returns the number of cells removed. The function is intended as the
/// post-fill fixup; a well-formed overhang is already supported, so it removes
/// only the occasional isolated lobe.
pub fn prune_unsupported(solid: &mut [bool], dims: [usize; 3]) -> usize {
    let [nx, ny, nz] = dims;
    assert!(nx > 0 && ny > 0 && nz > 0, "dims must be non-zero");
    assert_eq!(solid.len(), nx * ny * nz, "mask length must match dims");

    let index = |x: usize, y: usize, z: usize| x + nx * (y + ny * z);
    let mut supported = vec![false; solid.len()];
    let mut stack = Vec::new();

    // Ground layer.
    for z in 0..nz {
        for x in 0..nx {
            let i = index(x, 0, z);
            if solid[i] {
                supported[i] = true;
                stack.push((x, 0, z));
            }
        }
    }
    // Vertical side faces: the sampled box is a window into a larger terrain.
    for y in 0..ny {
        for z in 0..nz {
            for x in [0, nx - 1] {
                let i = index(x, y, z);
                if solid[i] && !supported[i] {
                    supported[i] = true;
                    stack.push((x, y, z));
                }
            }
        }
        for x in 0..nx {
            for z in [0, nz - 1] {
                let i = index(x, y, z);
                if solid[i] && !supported[i] {
                    supported[i] = true;
                    stack.push((x, y, z));
                }
            }
        }
    }

    while let Some((x, y, z)) = stack.pop() {
        let mut visit = |vx: usize, vy: usize, vz: usize| {
            let i = index(vx, vy, vz);
            if solid[i] && !supported[i] {
                supported[i] = true;
                stack.push((vx, vy, vz));
            }
        };
        if x > 0 {
            visit(x - 1, y, z);
        }
        if x + 1 < nx {
            visit(x + 1, y, z);
        }
        if y > 0 {
            visit(x, y - 1, z);
        }
        if y + 1 < ny {
            visit(x, y + 1, z);
        }
        if z > 0 {
            visit(x, y, z - 1);
        }
        if z + 1 < nz {
            visit(x, y, z + 1);
        }
    }

    let mut removed = 0;
    for (i, cell) in solid.iter_mut().enumerate() {
        if *cell && !supported[i] {
            *cell = false;
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metres(value: i32) -> Coord {
        (value as i64) << 16
    }

    fn index(x: usize, y: usize, z: usize, dims: [usize; 3]) -> usize {
        x + dims[0] * (y + dims[1] * z)
    }

    #[test]
    fn overhang_density_is_deterministic() {
        let field = OverhangField::new(7);
        let a = field.density(metres(11), metres(38), metres(13), metres(40));
        let b = field.density(metres(11), metres(38), metres(13), metres(40));
        assert_eq!(a, b);
        assert!(field.solid(metres(11), metres(0), metres(13), metres(40)));
        assert!(!field.solid(
            metres(11),
            metres(40 + OVERHANG_AMPLITUDE_M + 1),
            metres(13),
            metres(40)
        ));
    }

    #[test]
    fn overhang_density_has_a_bounded_rise() {
        let field = OverhangField::new(3);
        for x in (0..64).step_by(7) {
            for z in (0..64).step_by(11) {
                for y in 0..=40 {
                    let _ = field.solid(metres(x), metres(y), metres(z), metres(40));
                }
                assert!(
                    !field.solid(
                        metres(x),
                        metres(40 + OVERHANG_AMPLITUDE_M + 1),
                        metres(z),
                        metres(40)
                    ),
                    "a shelf rose above the amplitude at ({x}, {z})"
                );
            }
        }
    }

    #[test]
    fn overhang_arch_component_has_a_supported_span() {
        let field = OverhangField::new(1);
        let surface = metres(40);
        let mut checked = 0;
        for cz in -4..=4 {
            for cx in -4..=4 {
                let h = hash::hash2(field.seed ^ ARCH_SALT, cx, cz);
                if h % 100 >= ARCH_PERCENT {
                    continue;
                }
                let centre_x = cx * ARCH_CELL_M + 8 + ((h >> 8) % 48) as i64;
                let centre_z = cz * ARCH_CELL_M + 8 + ((h >> 16) % 48) as i64;
                let along_x = (h >> 24) & 1 == 0;
                let half = 5 + ((h >> 25) % 5) as i64;
                let height = 6 + ((h >> 29) % 4) as i64;
                let thickness = 2 + ((h >> 33) % 2) as i64;
                let rise = (half / 3) as i32;
                let bottom = surface + ((((height - thickness) as i32 + rise) as i64) << 16);
                let span_y = bottom + (ONE as Coord) / 2;
                assert!(
                    field.arch_solid(
                        metres(centre_x as i32),
                        span_y,
                        metres(centre_z as i32),
                        surface
                    ),
                    "cell ({cx}, {cz}) has no span"
                );

                let (leg_x, leg_z) = if along_x {
                    (centre_x + half, centre_z)
                } else {
                    (centre_x, centre_z + half)
                };
                assert!(
                    field.arch_solid(
                        metres(leg_x as i32),
                        surface - (ONE as Coord),
                        metres(leg_z as i32),
                        surface
                    ),
                    "cell ({cx}, {cz}) has no leg"
                );
                assert!(
                    !field.arch_solid(
                        metres(centre_x as i32),
                        surface + (ONE as Coord),
                        metres(centre_z as i32),
                        surface
                    ),
                    "cell ({cx}, {cz}) has no opening"
                );

                checked += 1;
                if checked == 3 {
                    break;
                }
            }
            if checked == 3 {
                break;
            }
        }
        assert!(checked > 0, "no selected arch cell");
    }

    #[test]
    fn prune_removes_a_floating_block() {
        let dims = [8, 8, 8];
        let mut solid = vec![false; dims[0] * dims[1] * dims[2]];
        solid[index(3, 0, 3, dims)] = true;
        solid[index(3, 5, 3, dims)] = true;
        assert_eq!(prune_unsupported(&mut solid, dims), 1);
        assert!(solid[index(3, 0, 3, dims)]);
        assert!(!solid[index(3, 5, 3, dims)]);
    }

    #[test]
    fn prune_keeps_an_attached_arch() {
        let dims = [9, 8, 5];
        let mut solid = vec![false; dims[0] * dims[1] * dims[2]];
        for y in 0..=1 {
            for z in 0..dims[2] {
                for x in 0..dims[0] {
                    solid[index(x, y, z, dims)] = true;
                }
            }
        }
        for y in 2..=4 {
            for z in 1..dims[2] - 1 {
                solid[index(2, y, z, dims)] = true;
                solid[index(6, y, z, dims)] = true;
            }
        }
        for z in 1..dims[2] - 1 {
            for x in 2..=6 {
                solid[index(x, 5, z, dims)] = true;
            }
        }
        assert_eq!(prune_unsupported(&mut solid, dims), 0);
    }
}
