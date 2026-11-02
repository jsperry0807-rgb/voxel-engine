//! Ore placement by depth tier and vein.
//!
//! Ores are deterministic **veins**, not per-voxel speckle. A 16 m cell hash
//! decides whether a cell contains a vein, which ore it carries, where its
//! centre is and which axis the capsule runs along. A query checks the
//! 3 × 3 × 3 neighbouring cells, so a vein that crosses a chunk border is the
//! same vein from both sides.
//!
//! # Depth tiers
//!
//! Tiers are measured from sea level (6,600 m), which is the one datum that
//! does not move with the local terrain. The bands overlap so a deep vein can
//! be any of the four:
//!
//! | Ore | Height band | Equivalent depth |
//! |---|---|---|
//! | coal | 5,400–6,570 m | 30–1,200 m below sea |
//! | iron | 4,800–6,540 m | 60–1,800 m below sea |
//! | gold | 4,400–6,300 m | 300–2,200 m below sea |
//! | diamond | 4,000–5,700 m | 900–2,600 m below sea |
//!
//! The top of the ore field is 30 m below sea level, so no ore is exposed on a
//! normal land surface. Diamond stops 900 m below sea level, so a **gem never
//! occurs anywhere near the surface** even at the lowest coastline; the test
//! sweeps the whole band above 800 m below sea.
//!
//! # Veins
//!
//! A vein is a capsule of radius 1–2 m and half-length 4–8 m along one axis.
//! The capsule is a connected set by construction; the integration test
//! flood-fills the ore mask and checks that no interior ore block is an
//! isolated fragment.
//!
//! All arithmetic is integer and the field is a pure function of the seed and
//! position, so two workers and two machines produce the same veins.

use crate::fixed::Coord;
use crate::hash;
use crate::splines::SEA_LEVEL;

/// Horizontal and vertical edge of an ore cell, in metres.
pub const ORE_CELL_M: i64 = 16;

/// Share of ore cells that contain a vein, in percent.
pub const VEIN_PERCENT: u64 = 35;

/// Coal is present from 1,200 m below sea to 30 m below sea.
pub const COAL_MIN_Y: i32 = SEA_LEVEL - (1_200 << 16);
/// Highest coal, 30 m below sea.
pub const COAL_MAX_Y: i32 = SEA_LEVEL - (30 << 16);

/// Iron is present from 1,800 m below sea to 60 m below sea.
pub const IRON_MIN_Y: i32 = SEA_LEVEL - (1_800 << 16);
/// Highest iron, 60 m below sea.
pub const IRON_MAX_Y: i32 = SEA_LEVEL - (60 << 16);

/// Gold is present from 2,200 m below sea to 300 m below sea.
pub const GOLD_MIN_Y: i32 = SEA_LEVEL - (2_200 << 16);
/// Highest gold, 300 m below sea.
pub const GOLD_MAX_Y: i32 = SEA_LEVEL - (300 << 16);

/// Diamond is present from 2,600 m below sea to 900 m below sea.
pub const DIAMOND_MIN_Y: i32 = SEA_LEVEL - (2_600 << 16);
/// Highest diamond, 900 m below sea.
pub const DIAMOND_MAX_Y: i32 = SEA_LEVEL - (900 << 16);

/// Highest point the ore field can reach, Q16.16 metres.
pub const ORE_TOP_Y: i32 = COAL_MAX_Y;

/// Lowest point the ore field can reach, Q16.16 metres.
pub const ORE_BOTTOM_Y: i32 = DIAMOND_MIN_Y;

/// Minimum connected size expected of a vein that is wholly inside a scan.
pub const MIN_VEIN_BLOCKS: usize = 4;

const ORE_SALT: u64 = 0x4F52_4553_0001;

/// The four ore kinds the manifest provides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OreKind {
    /// Coal, the shallow fuel seam.
    Coal,
    /// Iron, a common mid-depth metal.
    Iron,
    /// Gold, a deeper metal.
    Gold,
    /// Diamond, the rare deep gem.
    Diamond,
}

impl OreKind {
    /// Every ore, in tier order.
    pub const ALL: [Self; 4] = [Self::Coal, Self::Iron, Self::Gold, Self::Diamond];

    /// The committed block key.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Coal => "voxel:coal_ore",
            Self::Iron => "voxel:iron_ore",
            Self::Gold => "voxel:gold_ore",
            Self::Diamond => "voxel:diamond_ore",
        }
    }

    /// True for the metals (iron and gold).
    pub const fn is_metal(self) -> bool {
        matches!(self, Self::Iron | Self::Gold)
    }

    /// True for the gems (diamond).
    pub const fn is_gem(self) -> bool {
        matches!(self, Self::Diamond)
    }

    /// Lowest height this ore can occupy, Q16.16 metres.
    pub const fn min_y(self) -> i32 {
        match self {
            Self::Coal => COAL_MIN_Y,
            Self::Iron => IRON_MIN_Y,
            Self::Gold => GOLD_MIN_Y,
            Self::Diamond => DIAMOND_MIN_Y,
        }
    }

    /// Highest height this ore can occupy, Q16.16 metres.
    pub const fn max_y(self) -> i32 {
        match self {
            Self::Coal => COAL_MAX_Y,
            Self::Iron => IRON_MAX_Y,
            Self::Gold => GOLD_MAX_Y,
            Self::Diamond => DIAMOND_MAX_Y,
        }
    }
}

/// The seeded ore field.
#[derive(Clone, Copy, Debug)]
pub struct OreField {
    seed: u64,
}

impl OreField {
    /// Create an ore field from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The seed this field was created with.
    pub const fn seed(self) -> u64 {
        self.seed
    }

    /// The ore at a voxel, or `None` for plain stone.
    ///
    /// `x`, `y` and `z` are Q16.16 metres. The caller writes the returned
    /// material only into a solid voxel; the field itself does not know the
    /// cave or surface state.
    pub fn at(&self, x: Coord, y: Coord, z: Coord) -> Option<OreKind> {
        if y > ORE_TOP_Y as Coord || y < ORE_BOTTOM_Y as Coord || y < 0 {
            return None;
        }

        let xm = x >> 16;
        let ym = y >> 16;
        let zm = z >> 16;
        let cell_x = xm.div_euclid(ORE_CELL_M);
        let cell_y = ym.div_euclid(ORE_CELL_M);
        let cell_z = zm.div_euclid(ORE_CELL_M);

        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let h =
                        hash::hash3(self.seed ^ ORE_SALT, cell_x + dx, cell_y + dy, cell_z + dz);
                    if h % 100 >= VEIN_PERCENT {
                        continue;
                    }

                    let centre_x = (cell_x + dx) * ORE_CELL_M + 4 + ((h >> 8) % 8) as i64;
                    let centre_y = (cell_y + dy) * ORE_CELL_M + 4 + ((h >> 16) % 8) as i64;
                    let centre_z = (cell_z + dz) * ORE_CELL_M + 4 + ((h >> 24) % 8) as i64;
                    let Some(kind) = kind_for((centre_y << 16) as Coord, h) else {
                        continue;
                    };

                    let half = 4 + ((h >> 32) % 5) as i64;
                    let axis = (h >> 35) % 3;
                    let radius = 1 + ((h >> 37) % 2) as i64;
                    let radius2 = radius * radius;
                    let dxm = xm - centre_x;
                    let dym = ym - centre_y;
                    let dzm = zm - centre_z;
                    let inside = match axis {
                        0 => dxm.abs() <= half && dym * dym + dzm * dzm <= radius2,
                        1 => dym.abs() <= half && dxm * dxm + dzm * dzm <= radius2,
                        _ => dzm.abs() <= half && dxm * dxm + dym * dym <= radius2,
                    };
                    if inside {
                        return Some(kind);
                    }
                }
            }
        }
        None
    }
}

/// Pick the ore for a vein centre at `y`, using the cell hash for the choice.
///
/// The bands overlap; a centre in the deep all-four band chooses uniformly
/// among the four, while a shallow centre has fewer candidates.
pub fn kind_for(y: Coord, h: u64) -> Option<OreKind> {
    let mut kinds = [OreKind::Coal; 4];
    let mut count = 0;
    for kind in OreKind::ALL {
        if y >= kind.min_y() as Coord && y <= kind.max_y() as Coord {
            kinds[count] = kind;
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(kinds[(h >> 39) as usize % count])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::ONE;

    fn metres(value: i32) -> Coord {
        (value as i64) << 16
    }

    #[test]
    fn ores_are_deterministic() {
        let field = OreField::new(7);
        let at = |x, y, z| field.at(metres(x), metres(y), metres(z));
        assert_eq!(at(11, 5_000, 13), at(11, 5_000, 13));
        assert_eq!(field.seed(), 7);
    }

    #[test]
    fn ores_stop_above_the_top() {
        let field = OreField::new(1);
        for x in (0..64).step_by(3) {
            for z in (0..64).step_by(5) {
                assert_eq!(
                    field.at(metres(x), ORE_TOP_Y as Coord + ONE as Coord, metres(z)),
                    None
                );
                assert_eq!(field.at(metres(x), metres(6_600), metres(z)), None);
            }
        }
    }

    #[test]
    fn ores_never_place_a_gem_above_the_ceiling() {
        let field = OreField::new(1);
        // A diamond centre can sit at most at DIAMOND_MAX_Y and a capsule adds
        // about 10 m; 32 m above the ceiling is safely clear of every block.
        let ceiling = DIAMOND_MAX_Y as Coord + (32 << 16);
        for x in (0..64).step_by(8) {
            for z in (0..64).step_by(8) {
                for y in (ceiling..metres(6_600)).step_by(1 << 16) {
                    if let Some(kind) = field.at(metres(x), y, metres(z)) {
                        assert_ne!(kind, OreKind::Diamond, "diamond at {y}");
                    }
                }
            }
        }
    }

    #[test]
    fn ores_have_the_expected_bands() {
        // A mid-depth centre can be any of the four.
        let all = kind_for(metres(5_000), 0);
        assert!(all.is_some());
        let shallow = kind_for(metres(6_500), 0);
        assert!(matches!(shallow, Some(OreKind::Coal) | Some(OreKind::Iron)));
        let deep = kind_for(metres(4_200), 0);
        assert!(matches!(deep, Some(OreKind::Gold) | Some(OreKind::Diamond)));
        assert_eq!(kind_for(metres(6_700), 0), None);
        assert_eq!(kind_for(metres(3_000), 0), None);
    }

    #[test]
    fn ores_below_the_floor_are_empty() {
        let field = OreField::new(2);
        assert_eq!(field.at(metres(0), -ONE as Coord, metres(0)), None);
    }
}
