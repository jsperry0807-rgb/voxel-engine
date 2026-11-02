//! Structure anchors and prefab stamps.
//!
//! The world is divided into **2 km cells**. Each cell has a 15% chance of a
//! structure; the rest are empty. The anchor is jittered by up to 500 m around
//! the cell centre, so neighbouring structures sit 1–3 km apart, inside the
//! design's 1–4 km band. Anchors on ocean cells are skipped.
//!
//! # Prefabs
//!
//! Four small prefabs are built from the M2.5-31 structure materials:
//!
//! | Kind | Shape |
//! |---|---|
//! | tower | 7 × 7 hollow stone tower with plank floors and crenellations |
//! | ruin | 10 × 8 broken wall ring with plank beams and moss |
//! | dungeon | 9 × 9 buried stone room with a plank floor |
//! | bridge/road | 15 × 5 plank deck on stone supports |
//!
//! `Structure::stamp` places a prefab on a heightfield. The base sits one block
//! above the highest foundation column, and every foundation column is filled
//! from that base down to its own ground. The result is **no floating
//! foundations**: every support column touches the terrain. A bridge only has
//! foundations at its two ends, so its middle span is intentionally above the
//! valley but is connected to both supported ends. A dungeon is buried below
//! the surface instead of standing on a foundation.
//!
//! Every prefab uses at least one M2.5-31 key; the integration test resolves
//! every block key through the committed block manifest.
//!
//! # Determinism
//!
//! The grid, anchor, presence and type are pure functions of `(seed, cell)`.
//! The stamp is a pure function of the prefab and the supplied height closure,
//! so two workers produce the same blocks.

use crate::fixed::Coord;
use crate::generator::MacroGenerator;
use crate::hash;
use crate::macro_grid::WORLD_EXTENT_M;
use crate::splines::SEA_LEVEL;

/// Structure cell edge, in metres.
pub const STRUCTURE_CELL_M: i64 = 2_000;

/// Anchor jitter around the cell centre, in metres.
pub const STRUCTURE_JITTER_M: i64 = 500;

/// Cells per world axis: 1,024 km / 2 km.
pub const STRUCTURE_CELLS: i32 = (WORLD_EXTENT_M as i64 / STRUCTURE_CELL_M) as i32;

/// Share of cells that contain a structure, in percent.
pub const STRUCTURE_PERCENT: u64 = 15;

const STRUCTURE_SALT: u64 = 0x5354_5255_0001;

/// The four structure families.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StructureKind {
    /// A ruined wall ring.
    Ruin,
    /// A hollow stone tower.
    Tower,
    /// A buried stone room.
    Dungeon,
    /// A plank bridge with stone supports.
    BridgeRoad,
}

impl StructureKind {
    /// Every kind, in hash order.
    pub const ALL: [Self; 4] = [Self::Ruin, Self::Tower, Self::Dungeon, Self::BridgeRoad];

    /// Lower-case name for logs and tests.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ruin => "ruin",
            Self::Tower => "tower",
            Self::Dungeon => "dungeon",
            Self::BridgeRoad => "bridge",
        }
    }
}

/// One anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Structure {
    /// Cell column and row.
    pub cell: [i32; 2],
    /// Anchor block x.
    pub x: i32,
    /// Anchor block z.
    pub z: i32,
    /// Surface height at the anchor, in block metres.
    pub surface: i32,
    /// Chosen prefab.
    pub kind: StructureKind,
}

/// A block in a prefab's local frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefabBlock {
    /// Local x offset.
    pub dx: i32,
    /// Local y offset from the base.
    pub dy: i32,
    /// Local z offset.
    pub dz: i32,
    /// Material key.
    pub key: &'static str,
}

/// A prefab and the columns that must reach the ground.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefab {
    /// Kind this prefab builds.
    pub kind: StructureKind,
    /// Footprint width along x.
    pub width: i32,
    /// Footprint depth along z.
    pub depth: i32,
    /// Height above the base.
    pub height: i32,
    /// Local blocks.
    pub blocks: Vec<PrefabBlock>,
    /// Local `(x, z)` columns that need a foundation to the terrain.
    pub foundation: Vec<(i32, i32)>,
    /// True when the prefab is placed below the surface, not on foundations.
    pub buried: bool,
}

impl Prefab {
    /// Build the prefab for a kind.
    pub fn new(kind: StructureKind) -> Self {
        match kind {
            StructureKind::Ruin => ruin(),
            StructureKind::Tower => tower(),
            StructureKind::Dungeon => dungeon(),
            StructureKind::BridgeRoad => bridge(),
        }
    }
}

/// The stamped result: world block coordinates and materials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// Placed blocks in world block coordinates.
    pub blocks: Vec<PlacedBlock>,
    /// Number of foundation blocks added below the prefab.
    pub foundation_blocks: usize,
    /// True when the prefab was buried rather than founded.
    pub buried: bool,
}

/// One placed block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacedBlock {
    /// World block x.
    pub x: i32,
    /// World block y.
    pub y: i32,
    /// World block z.
    pub z: i32,
    /// Material key.
    pub key: &'static str,
}

impl Structure {
    /// Place the prefab on a heightfield.
    ///
    /// `height(x, z)` returns the solid surface height in block metres. The
    /// prefab base is one block above the highest foundation column; each
    /// foundation column is filled from the base down to its own surface.
    pub fn stamp(&self, height: impl Fn(i32, i32) -> i32) -> Stamp {
        let prefab = Prefab::new(self.kind);
        if prefab.buried {
            return self.stamp_buried(&prefab);
        }
        self.stamp_founded(&prefab, height)
    }

    fn stamp_founded(&self, prefab: &Prefab, height: impl Fn(i32, i32) -> i32) -> Stamp {
        // Sit above every column the prefab touches, not just the supports, so
        // no wall or deck block can end up inside a rise in the terrain.
        let mut base = i32::MIN;
        for block in &prefab.blocks {
            base = base.max(height(self.x + block.dx, self.z + block.dz));
        }
        let base = base.saturating_add(1);

        let mut blocks = Vec::with_capacity(prefab.blocks.len() + prefab.foundation.len() * 4);
        for block in &prefab.blocks {
            blocks.push(PlacedBlock {
                x: self.x + block.dx,
                y: base + block.dy,
                z: self.z + block.dz,
                key: block.key,
            });
        }

        let mut foundation_blocks = 0;
        for &(dx, dz) in &prefab.foundation {
            let ground = height(self.x + dx, self.z + dz);
            let mut y = base - 1;
            while y > ground {
                blocks.push(PlacedBlock {
                    x: self.x + dx,
                    y,
                    z: self.z + dz,
                    key: "voxel:cobblestone",
                });
                foundation_blocks += 1;
                y -= 1;
            }
        }

        Stamp {
            blocks,
            foundation_blocks,
            buried: false,
        }
    }

    fn stamp_buried(&self, prefab: &Prefab) -> Stamp {
        // The room's ceiling must stay below the surface.
        let base = self.surface - prefab.height - 2;
        if base <= 0 {
            return Stamp {
                blocks: Vec::new(),
                foundation_blocks: 0,
                buried: true,
            };
        }
        let blocks = prefab
            .blocks
            .iter()
            .map(|block| PlacedBlock {
                x: self.x + block.dx,
                y: base + block.dy,
                z: self.z + block.dz,
                key: block.key,
            })
            .collect();
        Stamp {
            blocks,
            foundation_blocks: 0,
            buried: true,
        }
    }
}

/// The seeded structure grid.
#[derive(Clone, Copy, Debug)]
pub struct StructureGrid {
    seed: u64,
    generator: MacroGenerator,
}

impl StructureGrid {
    /// Create a grid for a world seed.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            generator: MacroGenerator::new(seed),
        }
    }

    /// The seed this grid was created with.
    pub const fn seed(self) -> u64 {
        self.seed
    }

    /// The anchor of a cell, in Q16.16 metres.
    ///
    /// # Panics
    ///
    /// If the cell is outside `0..STRUCTURE_CELLS`.
    pub fn anchor(&self, cell_x: i32, cell_z: i32) -> (Coord, Coord) {
        assert!(
            (0..STRUCTURE_CELLS).contains(&cell_x) && (0..STRUCTURE_CELLS).contains(&cell_z),
            "structure cell {cell_x}, {cell_z} is outside the world"
        );
        let h = hash::hash2(self.seed ^ STRUCTURE_SALT, cell_x as i64, cell_z as i64);
        let span = (2 * STRUCTURE_JITTER_M + 1) as u64;
        let jitter_x = ((h % span) as i64 - STRUCTURE_JITTER_M) << 16;
        let jitter_z = (((h >> 16) % span) as i64 - STRUCTURE_JITTER_M) << 16;
        let centre_x = (cell_x as i64 * STRUCTURE_CELL_M + STRUCTURE_CELL_M / 2) << 16;
        let centre_z = (cell_z as i64 * STRUCTURE_CELL_M + STRUCTURE_CELL_M / 2) << 16;
        (centre_x + jitter_x, centre_z + jitter_z)
    }

    /// The structure in a cell, if the cell is non-empty and on land.
    pub fn structure(&self, cell_x: i32, cell_z: i32) -> Option<Structure> {
        let h = hash::hash2(self.seed ^ STRUCTURE_SALT, cell_x as i64, cell_z as i64);
        if h % 100 >= STRUCTURE_PERCENT {
            return None;
        }
        let (x, z) = self.anchor(cell_x, cell_z);
        let surface = self.generator.surface(x, z);
        if surface <= SEA_LEVEL {
            return None;
        }
        let kind = StructureKind::ALL[((h >> 8) % 4) as usize];
        Some(Structure {
            cell: [cell_x, cell_z],
            x: (x >> 16) as i32,
            z: (z >> 16) as i32,
            surface: surface >> 16,
            kind,
        })
    }

    /// Every structure in row-major cell order.
    pub fn iter(&self) -> impl Iterator<Item = Structure> + '_ {
        let cells = STRUCTURE_CELLS;
        (0..cells).flat_map(move |z| (0..cells).filter_map(move |x| self.structure(x, z)))
    }
}

/// M2.5-31 structure keys used by the prefabs.
pub const PREFAB_KEYS: [&str; 4] = [
    "voxel:stone_bricks",
    "voxel:cracked_stone_bricks",
    "voxel:mossy_cobblestone",
    "voxel:oak_planks",
];

fn push(blocks: &mut Vec<PrefabBlock>, dx: i32, dy: i32, dz: i32, key: &'static str) {
    blocks.push(PrefabBlock { dx, dy, dz, key });
}

fn tower() -> Prefab {
    let mut blocks = Vec::new();
    let half: i32 = 3;
    let height: i32 = 17;
    for y in 0..16 {
        for x in -half..=half {
            for z in -half..=half {
                let edge = x == -half || x == half || z == -half || z == half;
                if !edge {
                    continue;
                }
                // A door on the west face, two blocks tall.
                if x == -half && z == 0 && (y == 1 || y == 2) {
                    continue;
                }
                let key = if y == 0 {
                    "voxel:mossy_cobblestone"
                } else if (x + y + z).rem_euclid(7) == 0 {
                    "voxel:cracked_stone_bricks"
                } else if (x + z).rem_euclid(5) == 0 {
                    "voxel:mossy_cobblestone"
                } else {
                    "voxel:stone_bricks"
                };
                push(&mut blocks, x, y, z, key);
            }
        }
    }
    for y in [0i32, 5, 10, 15] {
        for x in -half + 1..=half - 1 {
            for z in -half + 1..=half - 1 {
                push(&mut blocks, x, y, z, "voxel:oak_planks");
            }
        }
    }
    for x in -half..=half {
        for z in -half..=half {
            let edge = x == -half || x == half || z == -half || z == half;
            if edge && (x + z).rem_euclid(2) == 0 {
                push(&mut blocks, x, 16, z, "voxel:stone_bricks");
            }
        }
    }
    let foundation = (0..=half * 2)
        .flat_map(|x| (0..=half * 2).map(move |z| (x - half, z - half)))
        .collect();
    Prefab {
        kind: StructureKind::Tower,
        width: 7,
        depth: 7,
        height,
        blocks,
        foundation,
        buried: false,
    }
}

fn ruin() -> Prefab {
    let mut blocks = Vec::new();
    let half_x: i32 = 5;
    let half_z: i32 = 4;
    for y in 0i32..3 {
        for x in -half_x..=half_x {
            for z in -half_z..=half_z {
                let edge = x == -half_x || x == half_x || z == -half_z || z == half_z;
                if !edge {
                    continue;
                }
                let gate = (x == -half_x && z == 0) || (x == half_x && z == 2);
                if gate && (y == 1 || y == 2) {
                    continue;
                }
                let hole = (x * 5 + z * 7 + y * 11).rem_euclid(13) == 0;
                if hole {
                    continue;
                }
                let key = match (x * 3 + z * 5 + y).rem_euclid(6) {
                    0 => "voxel:mossy_cobblestone",
                    1 => "voxel:cracked_stone_bricks",
                    _ => "voxel:stone_bricks",
                };
                push(&mut blocks, x, y, z, key);
            }
        }
    }
    // Beams sit on the corners and wall runs, never in open air.
    for x in [-half_x, 0, half_x] {
        push(&mut blocks, x, 3, -half_z, "voxel:oak_planks");
        push(&mut blocks, x, 3, half_z, "voxel:oak_planks");
    }
    for z in [-half_z + 1, 0, half_z - 1] {
        if !(z == 0) {
            push(&mut blocks, -half_x, 3, z, "voxel:oak_planks");
        }
        push(&mut blocks, half_x, 3, z, "voxel:oak_planks");
    }
    // A full interior floor, so every foundation column has a base block.
    for x in -half_x + 1..=half_x - 1 {
        for z in -half_z + 1..=half_z - 1 {
            let key = if (x + z).rem_euclid(5) == 0 {
                "voxel:mossy_cobblestone"
            } else {
                "voxel:stone_bricks"
            };
            push(&mut blocks, x, 0, z, key);
        }
    }
    let foundation = (0..=half_x * 2)
        .flat_map(|x| (0..=half_z * 2).map(move |z| (x - half_x, z - half_z)))
        .collect();
    Prefab {
        kind: StructureKind::Ruin,
        width: 11,
        depth: 9,
        height: 4,
        blocks,
        foundation,
        buried: false,
    }
}

fn dungeon() -> Prefab {
    let mut blocks = Vec::new();
    let half: i32 = 4;
    for y in 0i32..5 {
        for x in -half..=half {
            for z in -half..=half {
                let edge = x == -half || x == half || z == -half || z == half;
                if y == 0 || y == 4 || edge {
                    let key = if y == 0 && x.abs() <= 2 && z.abs() <= 2 {
                        "voxel:oak_planks"
                    } else {
                        match (x + y + z).rem_euclid(5) {
                            0 => "voxel:mossy_cobblestone",
                            1 => "voxel:cracked_stone_bricks",
                            _ => "voxel:stone_bricks",
                        }
                    };
                    push(&mut blocks, x, y, z, key);
                }
            }
        }
    }
    Prefab {
        kind: StructureKind::Dungeon,
        width: 9,
        depth: 9,
        height: 5,
        blocks,
        foundation: Vec::new(),
        buried: true,
    }
}

fn bridge() -> Prefab {
    let mut blocks = Vec::new();
    let half: i32 = 7;
    let half_z: i32 = 2;
    for x in -half..=half {
        for z in -half_z..=half_z {
            let key = if x == -half || x == half {
                "voxel:stone_bricks"
            } else {
                "voxel:oak_planks"
            };
            push(&mut blocks, x, 0, z, key);
        }
    }
    for x in -half..=half {
        for z in [-half_z, half_z] {
            if x.rem_euclid(4) == 0 {
                push(&mut blocks, x, 1, z, "voxel:stone_bricks");
            } else {
                push(&mut blocks, x, 1, z, "voxel:oak_planks");
            }
        }
    }
    // End caps get the stone supports; the middle span intentionally does not.
    let foundation = (-half..=-half + 2)
        .chain(half - 2..=half)
        .flat_map(|x| (-half_z..=half_z).map(move |z| (x, z)))
        .collect();
    Prefab {
        kind: StructureKind::BridgeRoad,
        width: 15,
        depth: 5,
        height: 2,
        blocks,
        foundation,
        buried: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_grid_anchors_inside_cells() {
        let grid = StructureGrid::new(1);
        assert_eq!(STRUCTURE_CELLS, 512);
        for cell_z in (0..STRUCTURE_CELLS).step_by(17) {
            for cell_x in (0..STRUCTURE_CELLS).step_by(19) {
                let (x, z) = grid.anchor(cell_x, cell_z);
                let left = (cell_x as i64 * STRUCTURE_CELL_M) << 16;
                let top = (cell_z as i64 * STRUCTURE_CELL_M) << 16;
                assert!(x >= left && x < left + (STRUCTURE_CELL_M << 16));
                assert!(z >= top && z < top + (STRUCTURE_CELL_M << 16));
            }
        }
    }

    #[test]
    fn structures_prefabs_have_the_m25_31_keys() {
        for kind in StructureKind::ALL {
            let prefab = Prefab::new(kind);
            assert!(
                prefab
                    .blocks
                    .iter()
                    .any(|block| PREFAB_KEYS.contains(&block.key)),
                "{kind:?} has no M2.5-31 key"
            );
        }
    }

    #[test]
    fn structures_stamp_keeps_foundations_on_the_ground() {
        let structure = Structure {
            cell: [0, 0],
            x: 0,
            z: 0,
            surface: 40,
            kind: StructureKind::Tower,
        };
        let height = |x: i32, z: i32| 40 + x / 3 + z / 5;
        let stamp = structure.stamp(height);
        assert!(!stamp.blocks.is_empty());
        assert!(stamp.foundation_blocks > 0);
        let prefab = Prefab::new(StructureKind::Tower);
        for &(dx, dz) in &prefab.foundation {
            let ground = height(dx, dz);
            let lowest = stamp
                .blocks
                .iter()
                .filter(|block| block.x == structure.x + dx && block.z == structure.z + dz)
                .map(|block| block.y)
                .min()
                .expect("foundation column");
            assert_eq!(lowest, ground + 1, "floating foundation at ({dx}, {dz})");
        }
    }

    #[test]
    fn structures_dungeon_is_buried() {
        let structure = Structure {
            cell: [0, 0],
            x: 0,
            z: 0,
            surface: 80,
            kind: StructureKind::Dungeon,
        };
        let stamp = structure.stamp(|_, _| 80);
        assert!(stamp.buried);
        assert_eq!(stamp.foundation_blocks, 0);
        assert!(stamp.blocks.iter().all(|block| block.y < 80));
        assert!(stamp.blocks.iter().all(|block| block.y > 0));
    }
}
