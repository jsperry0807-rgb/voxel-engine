//! 3D cave carve: spaghetti, cheese chambers, shafts, galleries and fluids.
//!
//! The cave field is a pure function of `(seed, x, y, z, context)`, like every
//! other generator channel. It is sampled at 1 m block centres and returns
//! `None` for solid rock or a [`CaveVoxel`] naming the morphology and fill.
//!
//! # The five types
//!
//! | Type | Field | Shape |
//! |---|---|---|
//! | spaghetti | two independent 3D noise iso-surfaces | winding tubes |
//! | cheese | one low-frequency 3D noise above a threshold | rounded chambers |
//! | shaft | deterministic 24 m cell hash | narrow vertical cylinders |
//! | gallery | a 2D iso-line plus a floor band | walkable horizontal corridors |
//! | water/lava | fill decided by the water and lava tables | fluid-filled voids |
//!
//! Spaghetti is the intersection of two zero-crossing sheets: each sheet alone
//! is a wall, and their intersection is a tube. Cheese is the opposite regime,
//! a low-frequency blob threshold. Shafts come from a coarse cell hash so they
//! have a controlled radius, and every query checks the 3 × 3 neighbouring
//! cells, so a shaft's horizontal cross-section is identical across a chunk
//! border. Galleries are a 2D iso-line extruded through a 3 m floor band, which
//! gives them a flat, walkable floor and a natural corridor shape.
//!
//! Fluid is not a separate morphology: any carved voxel at or below the water
//! table is water, and at or below the lava table is lava. That keeps "water
//! cave" and "lava cave" independent of whether the tunnel is spaghetti, a
//! chamber or a gallery.
//!
//! # Safety rules
//!
//! - Nothing is carved above the local surface, below the floor, or more than
//!   2,000 m below the surface (the M2.5-15 depth budget).
//! - A water-covered column keeps the **first 2 m of its bed** solid. That
//!   seals oceans, lakes and rivers from a cave opening directly underneath
//!   them; deeper voids are still carved.
//!
//! # Determinism
//!
//! There is no ordered RNG and no floating point. Noise, the cell hash and the
//! fluid tables are integer; the same seed and context produce the same voxel
//! on every worker.

use crate::fixed::{self, Coord, ONE};
use crate::hash;
use crate::noise::Noise;

/// Deepest carve below the local surface, in metres.
pub const MAX_CAVE_DEPTH_M: i32 = 2_000;

/// Thickness of the solid seal under a water body, in metres.
pub const WATER_SEAL_M: i32 = 2;

/// Spaghetti field wavelength, in metres.
pub const SPAGHETTI_WAVELENGTH_M: i64 = 40;

/// Half-width of each spaghetti iso-surface, Q16.16.
pub const SPAGHETTI_THRESHOLD: i32 = ONE * 5 / 100;

/// Cheese field wavelength, in metres.
pub const CHEESE_WAVELENGTH_M: i64 = 48;

/// Cheese chambers are values above this threshold, Q16.16.
pub const CHEESE_THRESHOLD: i32 = ONE * 39 / 100;

/// Gallery centre-line wavelength, in metres.
pub const GALLERY_WAVELENGTH_M: i64 = 40;

/// Gallery floor-height wavelength, in metres.
pub const GALLERY_HEIGHT_WAVELENGTH_M: i64 = 80;

/// Half-width of the gallery centre line, Q16.16.
pub const GALLERY_WIDTH: i32 = ONE * 10 / 100;

/// Gallery height above its floor, in metres.
pub const GALLERY_HEIGHT_M: i32 = 3;

/// Shallowest gallery floor, in metres below the surface.
pub const GALLERY_BASE_DEPTH_M: i32 = 20;

/// How far the gallery floor can wander down, in metres.
pub const GALLERY_SWING_M: i32 = 24;

/// Shaft cell edge in the horizontal plane, in metres.
pub const SHAFT_CELL_M: i64 = 24;

/// Share of shaft cells that contain a shaft, in percent.
pub const SHAFT_PERCENT: u64 = 55;

/// Shaft top, in metres below the surface.
pub const SHAFT_TOP_M: i32 = 3;

/// Shaft bottom, in metres below the surface.
pub const SHAFT_BOTTOM_M: i32 = 90;

const SPAGHETTI_A_SALT: u64 = 0x4341_5645_0001;
const SPAGHETTI_B_SALT: u64 = 0x4341_5645_0002;
const CHEESE_SALT: u64 = 0x4341_5645_0003;
const GALLERY_SALT: u64 = 0x4341_5645_0004;
const GALLERY_HEIGHT_SALT: u64 = 0x4341_5645_0005;
const SHAFT_SALT: u64 = 0x4341_5645_0006;

/// Q16.16 scale that maps a Q16.16 metre coordinate into noise space.
const fn scale_m(wavelength_m: i64) -> i32 {
    (ONE as i64 / wavelength_m) as i32
}

/// Cave morphology.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaveShape {
    /// A winding tube from two noise iso-surfaces.
    Spaghetti,
    /// A rounded chamber from low-frequency noise.
    Cheese,
    /// A narrow vertical cylinder from the cell hash.
    Shaft,
    /// A walkable horizontal corridor with a flat floor.
    Gallery,
}

impl CaveShape {
    /// Every morphology, for balanced-density accounting.
    pub const ALL: [Self; 4] = [Self::Spaghetti, Self::Cheese, Self::Shaft, Self::Gallery];
}

/// What fills a carved voxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CaveFill {
    /// Open air.
    Air,
    /// Water; the caller may connect it to the local water body.
    Water,
    /// Lava; geothermal or deep.
    Lava,
}

/// The result of sampling one voxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CaveVoxel {
    /// Morphology that carved the voxel.
    pub shape: CaveShape,
    /// Fluid that occupies it.
    pub fill: CaveFill,
}

/// The local surface and fluid tables for one column.
///
/// `surface` is the solid bed height. For an underwater column it is the
/// seabed/lakebed/riverbed, not the water plane; [`underwater`](Self::underwater)
/// is what enables the 2 m solid seal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaveContext {
    /// Solid surface height, Q16.16 metres.
    pub surface: Coord,
    /// True under an ocean, lake or river.
    pub underwater: bool,
    /// Voids at or below this height fill with water, Q16.16 metres.
    pub water_table: Coord,
    /// Voids at or below this height fill with lava, Q16.16 metres.
    pub lava_table: Coord,
}

impl CaveContext {
    /// A context with every input explicit.
    pub const fn new(
        surface: Coord,
        underwater: bool,
        water_table: Coord,
        lava_table: Coord,
    ) -> Self {
        Self {
            surface,
            underwater,
            water_table,
            lava_table,
        }
    }

    /// A dry column with the given fluid tables.
    pub const fn land(surface: Coord, water_table: Coord, lava_table: Coord) -> Self {
        Self::new(surface, false, water_table, lava_table)
    }

    /// A water-covered column with the given fluid tables.
    pub const fn submerged(surface: Coord, water_table: Coord, lava_table: Coord) -> Self {
        Self::new(surface, true, water_table, lava_table)
    }
}

/// The seeded cave field.
#[derive(Clone, Copy, Debug)]
pub struct CaveField {
    spaghetti_a: Noise,
    spaghetti_b: Noise,
    cheese: Noise,
    gallery: Noise,
    gallery_height: Noise,
    seed: u64,
}

impl CaveField {
    /// Create a cave field from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self {
            spaghetti_a: Noise::new(seed ^ SPAGHETTI_A_SALT),
            spaghetti_b: Noise::new(seed ^ SPAGHETTI_B_SALT),
            cheese: Noise::new(seed ^ CHEESE_SALT),
            gallery: Noise::new(seed ^ GALLERY_SALT),
            gallery_height: Noise::new(seed ^ GALLERY_HEIGHT_SALT),
            seed,
        }
    }

    /// The seed this field was created with.
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Carve one voxel, or return `None` for solid rock.
    ///
    /// `x`, `y` and `z` are Q16.16 metres. The caller samples block centres;
    /// the result is independent of how the volume is split into chunks.
    pub fn sample(&self, x: Coord, y: Coord, z: Coord, context: &CaveContext) -> Option<CaveVoxel> {
        if y > context.surface {
            return None;
        }
        if y < 0 {
            return None;
        }
        let max_depth = (MAX_CAVE_DEPTH_M as i64) << 16;
        if (context.surface - y) > max_depth {
            return None;
        }
        let seal = (WATER_SEAL_M as i64) << 16;
        if context.underwater && (context.surface - y) < seal {
            return None;
        }

        let shape = if self.shaft_at(x, y, z, context) {
            CaveShape::Shaft
        } else if self.gallery_at(x, y, z, context) {
            CaveShape::Gallery
        } else if self.cheese_at(x, y, z) {
            CaveShape::Cheese
        } else if self.spaghetti_at(x, y, z) {
            CaveShape::Spaghetti
        } else {
            return None;
        };

        let fill = if y <= context.lava_table {
            CaveFill::Lava
        } else if y <= context.water_table {
            CaveFill::Water
        } else {
            CaveFill::Air
        };
        Some(CaveVoxel { shape, fill })
    }

    /// Spaghetti: the intersection of two zero-crossing sheets.
    fn spaghetti_at(&self, x: Coord, y: Coord, z: Coord) -> bool {
        let scale = scale_m(SPAGHETTI_WAVELENGTH_M);
        let sx = fixed::coord_scale(x, scale);
        let sy = fixed::coord_scale(y, scale);
        let sz = fixed::coord_scale(z, scale);
        self.spaghetti_a.value_3d(sx, sy, sz).abs() < SPAGHETTI_THRESHOLD
            && self.spaghetti_b.value_3d(sx, sy, sz).abs() < SPAGHETTI_THRESHOLD
    }

    /// Cheese: low-frequency blobs above a threshold.
    fn cheese_at(&self, x: Coord, y: Coord, z: Coord) -> bool {
        let scale = scale_m(CHEESE_WAVELENGTH_M);
        let value = self.cheese.fbm_3d(
            fixed::coord_scale(x, scale),
            fixed::coord_scale(y, scale),
            fixed::coord_scale(z, scale),
            2,
            2 * ONE,
            ONE / 2,
        );
        value > CHEESE_THRESHOLD
    }

    /// Gallery: a winding 2D centre line with a flat floor band.
    fn gallery_at(&self, x: Coord, y: Coord, z: Coord, context: &CaveContext) -> bool {
        let scale = scale_m(GALLERY_WAVELENGTH_M);
        let line = self
            .gallery
            .value_2d(fixed::coord_scale(x, scale), fixed::coord_scale(z, scale));
        if line.abs() >= GALLERY_WIDTH {
            return false;
        }

        let height_scale = scale_m(GALLERY_HEIGHT_WAVELENGTH_M);
        let height = self.gallery_height.value_2d(
            fixed::coord_scale(x, height_scale),
            fixed::coord_scale(z, height_scale),
        );
        // Map height [-ONE, ONE] to [0, GALLERY_SWING_M] and push the floor
        // down from the base depth. The floor is constant in y for a column,
        // so the corridor is walkable rather than a sloped tube.
        let swing =
            ((height as i64 + ONE as i64) * ((GALLERY_SWING_M as i64) << 16)) / (2 * ONE as i64);
        let floor = context.surface - ((GALLERY_BASE_DEPTH_M as i64) << 16) - swing;
        let top = floor + ((GALLERY_HEIGHT_M as i64) << 16);
        y >= floor && y < top
    }

    /// Shaft: a narrow vertical cylinder from a 24 m cell hash.
    fn shaft_at(&self, x: Coord, y: Coord, z: Coord, context: &CaveContext) -> bool {
        let top = context.surface - ((SHAFT_TOP_M as i64) << 16);
        let bottom = context.surface - ((SHAFT_BOTTOM_M as i64) << 16);
        if y > top || y < bottom {
            return false;
        }

        let xm = x >> 16;
        let zm = z >> 16;
        let cell_x = xm.div_euclid(SHAFT_CELL_M);
        let cell_z = zm.div_euclid(SHAFT_CELL_M);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let h = hash::hash2(self.seed ^ SHAFT_SALT, cell_x + dx, cell_z + dz);
                if h % 100 >= SHAFT_PERCENT {
                    continue;
                }
                let centre_x = (cell_x + dx) * SHAFT_CELL_M + 6 + ((h >> 8) % 20) as i64;
                let centre_z = (cell_z + dz) * SHAFT_CELL_M + 6 + ((h >> 16) % 20) as i64;
                let radius = 2 + ((h >> 24) % 2) as i64;
                let dxm = xm - centre_x;
                let dzm = zm - centre_z;
                if dxm * dxm + dzm * dzm <= radius * radius {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metres(value: i32) -> Coord {
        (value as i64) << 16
    }

    fn context(surface: i32) -> CaveContext {
        CaveContext::land(metres(surface), metres(surface - 20), metres(4))
    }

    #[test]
    fn caves_never_carve_above_the_surface() {
        let field = CaveField::new(1);
        let context = context(48);
        for x in (0..64).step_by(3) {
            for z in (0..64).step_by(3) {
                for y in 49..64 {
                    assert_eq!(
                        field.sample(metres(x), metres(y), metres(z), &context),
                        None,
                        "carved above the surface at ({x}, {y}, {z})"
                    );
                }
            }
        }
    }

    #[test]
    fn caves_keep_the_two_metre_water_seal() {
        let field = CaveField::new(2);
        let context = CaveContext::submerged(metres(48), metres(40), metres(4));
        for x in (0..64).step_by(2) {
            for z in (0..64).step_by(2) {
                for y in 47..=48 {
                    assert_eq!(
                        field.sample(metres(x), metres(y), metres(z), &context),
                        None,
                        "carved into the water seal at ({x}, {y}, {z})"
                    );
                }
            }
        }
        // The same column still carves below the seal.
        let mut carved_below = 0;
        for x in (0..64).step_by(2) {
            for z in (0..64).step_by(2) {
                for y in 0..46 {
                    if field
                        .sample(metres(x), metres(y), metres(z), &context)
                        .is_some()
                    {
                        carved_below += 1;
                    }
                }
            }
        }
        assert!(carved_below > 0, "the seal must not stop all carving");
    }

    #[test]
    fn caves_respect_the_depth_budget_and_floor() {
        let field = CaveField::new(3);
        let context = context(48);
        for x in (0..64).step_by(4) {
            for z in (0..64).step_by(4) {
                assert_eq!(
                    field.sample(metres(x), metres(-1), metres(z), &context),
                    None,
                    "carved below the floor"
                );
                assert_eq!(
                    field.sample(metres(x), metres(48 - 2_001), metres(z), &context),
                    None,
                    "carved below the 2,000 m budget"
                );
            }
        }
    }

    #[test]
    fn caves_are_deterministic() {
        let field = CaveField::new(4);
        let context = context(48);
        let first = field.sample(metres(11), metres(20), metres(13), &context);
        let second = field.sample(metres(11), metres(20), metres(13), &context);
        assert_eq!(first, second);
        assert_eq!(field.seed(), 4);
    }
}
