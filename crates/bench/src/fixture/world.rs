//! Deterministic generator for the M1-28 fixture world.
//!
//! The generator is deliberately not the production worldgen in
//! `docs/worldgen-design.md`: it is a small, dependency-free stand-in that
//! exercises the shapes a save must carry before real saves exist. It builds
//! terrain with sea level, beaches and snowy peaks; chambers, tunnels, shafts
//! and water-filled caves; a rock canyon with natural roof shelves; a tower, a
//! ruin, a bridge, trees and plants; and most built-in materials, including
//! ores and cutout vegetation.
//!
//! Every choice is a hash of integer world coordinates or of the recipe seed.
//! No floating point, `rand`, clock or platform state enters the result, so a
//! chunk generated today is byte-identical to one generated in the verifier.

use super::{Feature, Recipe};
use voxel_storage::{CHUNK_DIM, CHUNK_VOLUME, ChunkPos, local_index};

/// Material ids in built-in registration order. `air` is 0 by the storage
/// contract; the rest follow [`voxel_block::BlockKind::ALL`].
///
/// The manifest records the same ids with namespaced keys. A save has no
/// palette of its own, so this table *is* the meaning of the fixture's bytes.
pub mod mat {
    pub const AIR: u16 = 0;
    pub const STONE: u16 = 1;
    pub const DIRT: u16 = 2;
    pub const GRASS: u16 = 3;
    pub const COBBLESTONE: u16 = 4;
    pub const SAND: u16 = 5;
    pub const GRAVEL: u16 = 6;
    pub const WATER: u16 = 7;
    pub const OAK_LOG: u16 = 8;
    pub const OAK_LEAVES: u16 = 9;
    pub const BEDROCK: u16 = 10;
    pub const COAL_ORE: u16 = 11;
    pub const IRON_ORE: u16 = 12;
    pub const GOLD_ORE: u16 = 13;
    pub const DIAMOND_ORE: u16 = 14;
    pub const TALL_GRASS: u16 = 15;
    pub const RED_FLOWER: u16 = 16;
    pub const SNOW: u16 = 17;
    pub const ICE: u16 = 18;
    pub const SANDSTONE: u16 = 19;
}

/// A named built-in material, in the order the fixture's raw ids refer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Material {
    pub id: u16,
    pub key: &'static str,
    pub name: &'static str,
    pub role: &'static str,
}

pub const MATERIALS: [Material; 20] = [
    Material {
        id: mat::AIR,
        key: "voxel:air",
        name: "air",
        role: "empty space",
    },
    Material {
        id: mat::STONE,
        key: "voxel:stone",
        name: "stone",
        role: "terrain and underground",
    },
    Material {
        id: mat::DIRT,
        key: "voxel:dirt",
        name: "dirt",
        role: "soil",
    },
    Material {
        id: mat::GRASS,
        key: "voxel:grass_block",
        name: "grass_block",
        role: "surface",
    },
    Material {
        id: mat::COBBLESTONE,
        key: "voxel:cobblestone",
        name: "cobblestone",
        role: "structures",
    },
    Material {
        id: mat::SAND,
        key: "voxel:sand",
        name: "sand",
        role: "beaches and canyon floor",
    },
    Material {
        id: mat::GRAVEL,
        key: "voxel:gravel",
        name: "gravel",
        role: "underground pockets and canyon floor",
    },
    Material {
        id: mat::WATER,
        key: "voxel:water",
        name: "water",
        role: "oceans and water-filled caves",
    },
    Material {
        id: mat::OAK_LOG,
        key: "voxel:oak_log",
        name: "oak_log",
        role: "trees and bridge deck",
    },
    Material {
        id: mat::OAK_LEAVES,
        key: "voxel:oak_leaves",
        name: "oak_leaves",
        role: "tree canopy",
    },
    Material {
        id: mat::BEDROCK,
        key: "voxel:bedrock",
        name: "bedrock",
        role: "world floor",
    },
    Material {
        id: mat::COAL_ORE,
        key: "voxel:coal_ore",
        name: "coal_ore",
        role: "shallow ore",
    },
    Material {
        id: mat::IRON_ORE,
        key: "voxel:iron_ore",
        name: "iron_ore",
        role: "mid-depth ore",
    },
    Material {
        id: mat::GOLD_ORE,
        key: "voxel:gold_ore",
        name: "gold_ore",
        role: "deep ore",
    },
    Material {
        id: mat::DIAMOND_ORE,
        key: "voxel:diamond_ore",
        name: "diamond_ore",
        role: "deepest ore",
    },
    Material {
        id: mat::TALL_GRASS,
        key: "voxel:tall_grass",
        name: "tall_grass",
        role: "cutout ground cover",
    },
    Material {
        id: mat::RED_FLOWER,
        key: "voxel:red_flower",
        name: "red_flower",
        role: "cutout ground cover",
    },
    Material {
        id: mat::SNOW,
        key: "voxel:snow",
        name: "snow",
        role: "peaks and cold surfaces",
    },
    Material {
        id: mat::ICE,
        key: "voxel:ice",
        name: "ice",
        role: "frozen water surface",
    },
    Material {
        id: mat::SANDSTONE,
        key: "voxel:sandstone",
        name: "sandstone",
        role: "beach subsoil",
    },
];

/// Sea level used by the fixture recipe.
pub const SEA_LEVEL: i32 = 44;

/// Depth band whose air must be underground: any air below this is cave,
/// shaft or structure void, never sky.
pub const DEEP_AIR_CEILING: i32 = 38;

/// One carved cave volume. `a`/`b` are the endpoints of a capsule and the
/// radius tapers from `ra` at `a` to `rb` at `b`.
#[derive(Clone, Copy, Debug)]
struct Cave {
    a: [i32; 3],
    b: [i32; 3],
    ra: i32,
    rb: i32,
    shaft: bool,
    water: bool,
}

impl Cave {
    fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let r = self.ra.max(self.rb) + 1;
        (
            [
                self.a[0].min(self.b[0]) - r,
                self.a[1].min(self.b[1]) - r,
                self.a[2].min(self.b[2]) - r,
            ],
            [
                self.a[0].max(self.b[0]) + r,
                self.a[1].max(self.b[1]) + r,
                self.a[2].max(self.b[2]) + r,
            ],
        )
    }

    /// Integer capsule test. `t` is 0..=1024 along `a` to `b`.
    fn contains(&self, point: [i32; 3]) -> bool {
        let ab = [
            self.b[0] - self.a[0],
            self.b[1] - self.a[1],
            self.b[2] - self.a[2],
        ];
        let ap = [
            point[0] - self.a[0],
            point[1] - self.a[1],
            point[2] - self.a[2],
        ];
        let length = ab
            .iter()
            .map(|&v| (v as i64) * (v as i64))
            .sum::<i64>()
            .max(1);
        let dot = ab
            .iter()
            .zip(ap.iter())
            .map(|(&u, &v)| (u as i64) * (v as i64))
            .sum::<i64>();
        let t = (dot * 1024 / length).clamp(0, 1024);
        let closest = [
            self.a[0] + ((ab[0] as i64 * t) / 1024) as i32,
            self.a[1] + ((ab[1] as i64 * t) / 1024) as i32,
            self.a[2] + ((ab[2] as i64 * t) / 1024) as i32,
        ];
        let radius = (self.ra as i64 * (1024 - t) + self.rb as i64 * t) / 1024;
        let distance = closest
            .iter()
            .zip(point.iter())
            .map(|(&a, &b)| {
                let d = (a as i64) - (b as i64);
                d * d
            })
            .sum::<i64>();
        distance <= radius * radius
    }
}

/// One non-terrain voxel: structures, trees, plants, and the air that clears a
/// tower's interior. `material == AIR` is an overwrite to empty.
#[derive(Clone, Copy, Debug)]
struct Extra {
    x: i32,
    y: i32,
    z: i32,
    material: u16,
}

/// A deterministic generator for one frozen fixture recipe.
pub struct WorldGenerator {
    recipe: Recipe,
    caves: Vec<Cave>,
    extras: Vec<Extra>,
}

impl WorldGenerator {
    /// Builds the feature list for `recipe`.
    pub fn new(recipe: &Recipe) -> Result<Self, String> {
        recipe.validate()?;
        let mut generator = Self {
            recipe: recipe.clone(),
            caves: Vec::new(),
            extras: Vec::new(),
        };
        generator.build_caves();
        generator.build_plants();
        generator.build_trees();
        generator.build_structures();
        Ok(generator)
    }

    /// The recipe this generator was built from.
    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// Number of cave volumes and non-terrain voxels, for the verifier's report.
    pub fn feature_counts(&self) -> (usize, usize) {
        (self.caves.len(), self.extras.len())
    }

    /// Fills one chunk with `CHUNK_VOLUME` raw material ids.
    pub fn fill(&self, pos: ChunkPos, out: &mut [u16]) {
        assert_eq!(
            out.len(),
            CHUNK_VOLUME,
            "chunk buffer has CHUNK_VOLUME cells"
        );
        out.fill(mat::AIR);
        let edge = CHUNK_DIM as i32;
        let ox = pos.x * edge;
        let oy = pos.y * edge;
        let oz = pos.z * edge;
        if oy >= CHUNK_DIM as i32 * 2 || oy + edge <= 0 {
            return;
        }

        let mut base_map = vec![0i32; CHUNK_DIM * CHUNK_DIM];
        for lz in 0..CHUNK_DIM {
            for lx in 0..CHUNK_DIM {
                let x = ox + lx as i32;
                let z = oz + lz as i32;
                let column = self.column(x, z);
                base_map[lz * CHUNK_DIM + lx] = column.base;
                self.fill_column(out, lx as u32, lz as u32, oy, x, z, column);
            }
        }

        self.carve_caves(out, ox, oy, oz, &base_map);
        self.stamp_extras(out, ox, oy, oz);
        self.plant_surface(out, ox, oy, oz, &base_map);
    }

    /// Terrain height before the canyon cut.
    fn surface_height(&self, x: i32, z: i32) -> i32 {
        let seed = self.recipe.seed;
        let continent = noise2(x, z, 256, seed);
        let hills = noise2(x, z, 64, seed ^ 0x51ED_2701);
        let detail = noise2(x, z, 16, seed ^ 0x9E37_79B9);
        let mut height = 30 + continent * 18 / 1000 + hills * 16 / 1000 + detail * 6 / 1000;
        let mountain = noise2(x, z, 128, seed ^ 0xC2B2_AE35);
        if mountain > 600 {
            height += (mountain - 600) * 44 / 400;
        }
        // One guaranteed massif gives the fixture a snow/ice peak even when the
        // hash field would otherwise land low.
        let dx = x + 96;
        let dz = z - 96;
        let distance = dx * dx + dz * dz;
        if distance < 48 * 48 {
            height += (48 * 48 - distance) * 26 / (48 * 48);
        }
        height.clamp(8, 116)
    }

    /// The ground and optional overhang shelf for one world column.
    fn column(&self, x: i32, z: i32) -> Column {
        let base = self.surface_height(x, z);
        let seed = self.recipe.seed;
        let centre = 8 + (noise1(z, 96, seed ^ 0x2545_F491) - 500) * 20 / 1000;
        let distance = (x - centre).abs();
        if distance >= 9 {
            return Column {
                base,
                ground: base,
                shelf: None,
            };
        }
        let depth = if distance <= 4 {
            16
        } else {
            16 - (distance - 4) * 2
        };
        let band = z.rem_euclid(96) < 40;
        let hole = hash2(x.div_euclid(4), z.div_euclid(4), seed ^ 0x7F4A_7C15) % 100 < 18;
        let shelf = (band && !hole && base > SEA_LEVEL + 8).then_some((base - 7, base - 4));
        Column {
            base,
            ground: base - depth,
            shelf,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_column(
        &self,
        out: &mut [u16],
        lx: u32,
        lz: u32,
        oy: i32,
        x: i32,
        z: i32,
        column: Column,
    ) {
        let cold = !(-80..=120).contains(&z);
        for ly in 0..CHUNK_DIM {
            let y = oy + ly as i32;
            let material = if y == 0 {
                mat::BEDROCK
            } else if y <= column.ground {
                self.ground_material(x, y, z, column)
            } else if y <= SEA_LEVEL {
                if cold && y == SEA_LEVEL {
                    mat::ICE
                } else {
                    mat::WATER
                }
            } else {
                mat::AIR
            };
            out[local_index(lx, ly as u32, lz) as usize] = material;
        }

        if let Some((low, high)) = column.shelf {
            for y in low..=high {
                if y < oy || y >= oy + CHUNK_DIM as i32 {
                    continue;
                }
                let ly = (y - oy) as u32;
                let index = local_index(lx, ly, lz) as usize;
                let material = if column.base > 76 && y == high {
                    mat::SNOW
                } else if hash3(x, y, z, self.recipe.seed ^ 0x39B4_4C2D) % 100 < 8 {
                    mat::COBBLESTONE
                } else {
                    mat::STONE
                };
                out[index] = material;
            }
        }
    }

    fn ground_material(&self, x: i32, y: i32, z: i32, column: Column) -> u16 {
        let base = column.base;
        let ground = column.ground;
        let cut = ground < base - 1;
        if cut {
            if y == ground {
                if ground <= SEA_LEVEL + 1 {
                    mat::SAND
                } else if hash2(x.div_euclid(2), z.div_euclid(2), self.recipe.seed) % 100 < 35 {
                    mat::GRAVEL
                } else {
                    mat::STONE
                }
            } else {
                self.stone_at(x, y, z, base)
            }
        } else if base <= SEA_LEVEL + 2 {
            if y == ground {
                mat::SAND
            } else if y > ground - 4 {
                mat::SANDSTONE
            } else {
                self.stone_at(x, y, z, base)
            }
        } else if base > 76 || (base > 70 && z < -80) {
            if y >= ground - 1 {
                mat::SNOW
            } else {
                self.stone_at(x, y, z, base)
            }
        } else if y == ground {
            mat::GRASS
        } else if y > ground - 4 {
            mat::DIRT
        } else {
            self.stone_at(x, y, z, base)
        }
    }

    /// Stone, with depth-banded ore clusters and gravel pockets.
    fn stone_at(&self, x: i32, y: i32, z: i32, base: i32) -> u16 {
        let h = hash3(x >> 1, y >> 1, z >> 1, self.recipe.seed ^ 0x1F83_D9AB) % 1000;
        if y < 16 && h < 14 {
            mat::DIAMOND_ORE
        } else if y < 32 && h < 28 {
            mat::GOLD_ORE
        } else if y < 56 && h < 52 {
            mat::IRON_ORE
        } else if y < base - 6 && h < 92 {
            mat::COAL_ORE
        } else if hash3(x, y, z, self.recipe.seed ^ 0x85EB_CA6B) % 100 < 12 {
            mat::GRAVEL
        } else {
            mat::STONE
        }
    }

    fn carve_caves(&self, out: &mut [u16], ox: i32, oy: i32, oz: i32, base_map: &[i32]) {
        let edge = CHUNK_DIM as i32;
        let (min, max) = ([ox, oy, oz], [ox + edge - 1, oy + edge - 1, oz + edge - 1]);
        for cave in &self.caves {
            let (low, high) = cave.bounds();
            if low[0] > max[0]
                || low[1] > max[1]
                || low[2] > max[2]
                || high[0] < min[0]
                || high[1] < min[1]
                || high[2] < min[2]
            {
                continue;
            }
            for y in low[1].max(min[1])..=high[1].min(max[1]) {
                for z in low[2].max(min[2])..=high[2].min(max[2]) {
                    for x in low[0].max(min[0])..=high[0].min(max[0]) {
                        if !cave.contains([x, y, z]) {
                            continue;
                        }
                        let lx = (x - ox) as u32;
                        let ly = (y - oy) as u32;
                        let lz = (z - oz) as u32;
                        let base = base_map[lz as usize * CHUNK_DIM + lx as usize];
                        if y > base - 8 && !cave.shaft {
                            continue;
                        }
                        if y > base {
                            continue;
                        }
                        let index = local_index(lx, ly, lz) as usize;
                        let current = out[index];
                        if current == mat::AIR
                            || current == mat::BEDROCK
                            || current == mat::WATER
                            || current == mat::ICE
                        {
                            continue;
                        }
                        out[index] = if cave.water && y <= 18 {
                            mat::WATER
                        } else {
                            mat::AIR
                        };
                    }
                }
            }
        }
    }

    fn stamp_extras(&self, out: &mut [u16], ox: i32, oy: i32, oz: i32) {
        let edge = CHUNK_DIM as i32;
        for extra in &self.extras {
            if extra.x < ox
                || extra.x >= ox + edge
                || extra.y < oy
                || extra.y >= oy + edge
                || extra.z < oz
                || extra.z >= oz + edge
            {
                continue;
            }
            let lx = (extra.x - ox) as u32;
            let ly = (extra.y - oy) as u32;
            let lz = (extra.z - oz) as u32;
            out[local_index(lx, ly, lz) as usize] = extra.material;
        }
    }

    fn plant_surface(&self, out: &mut [u16], ox: i32, oy: i32, oz: i32, base_map: &[i32]) {
        let edge = CHUNK_DIM as i32;
        for lz in 0..CHUNK_DIM {
            for lx in 0..CHUNK_DIM {
                let x = ox + lx as i32;
                let z = oz + lz as i32;
                let top = base_map[lz * CHUNK_DIM + lx];
                if top < oy || top + 1 >= oy + edge {
                    continue;
                }
                let top_index = local_index(lx as u32, (top - oy) as u32, lz as u32) as usize;
                let above_index = local_index(lx as u32, (top + 1 - oy) as u32, lz as u32) as usize;
                if out[top_index] != mat::GRASS || out[above_index] != mat::AIR {
                    continue;
                }
                let h = hash2(x, z, self.recipe.seed ^ 0xA24B_AED4);
                let material = match h % 1000 {
                    0..=14 => Some(mat::TALL_GRASS),
                    15..=19 => Some(mat::RED_FLOWER),
                    _ => None,
                };
                if let Some(material) = material {
                    out[above_index] = material;
                }
            }
        }
    }

    fn build_caves(&mut self) {
        let seed = self.recipe.seed;
        let (min, max) = self.recipe.world_bounds();
        let cell = 32;
        let gx0 = (min[0] - 48).div_euclid(cell);
        let gx1 = (max[0] + 48).div_euclid(cell);
        let gz0 = (min[2] - 48).div_euclid(cell);
        let gz1 = (max[2] + 48).div_euclid(cell);
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                for layer in 0..5 {
                    let h = hash3(gx, layer, gz, seed ^ 0x5DEE_CE66);
                    let x = gx * cell + 4 + (h % 24) as i32;
                    let z = gz * cell + 4 + ((h >> 8) % 24) as i32;
                    let ground = self.surface_height(x, z);
                    let depth = [8, 20, 32, 46, 62][layer as usize] + ((h >> 16) % 8) as i32;
                    let y = ground - depth;
                    if y < 6 || y > ground - 4 {
                        continue;
                    }
                    let radius = 3 + ((h >> 24) % 4) as i32;
                    let kind = (h >> 32) % 10;
                    let water = y < 18 && (h >> 40).is_multiple_of(4);
                    let cave = match kind {
                        0..=4 => Cave {
                            a: [x, y, z],
                            b: [x, y, z],
                            ra: radius,
                            rb: radius,
                            shaft: false,
                            water,
                        },
                        5..=6 => Cave {
                            a: [x - 7, y, z],
                            b: [x + 7, y + ((h >> 44) % 4) as i32 - 2, z],
                            ra: radius,
                            rb: radius + 1,
                            shaft: false,
                            water,
                        },
                        7..=8 => {
                            let direction = (h >> 44) % 4;
                            let (dx, dz) = match direction {
                                0 => (12, 0),
                                1 => (-12, 0),
                                2 => (0, 12),
                                _ => (0, -12),
                            };
                            Cave {
                                a: [x, y, z],
                                b: [x + dx, (y + 3).min(ground - 5), z + dz],
                                ra: 2,
                                rb: 2,
                                shaft: false,
                                water,
                            }
                        }
                        _ if layer <= 1 => Cave {
                            a: [x, ground - 1, z],
                            b: [x, y, z],
                            ra: 1 + ((h >> 48) % 2) as i32,
                            rb: 2,
                            shaft: true,
                            water,
                        },
                        _ => continue,
                    };
                    self.caves.push(cave);
                }
            }
        }
    }

    fn build_plants(&mut self) {
        let (min, max) = self.recipe.world_bounds();
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                let column = self.column(x, z);
                if column.ground != column.base || column.base <= SEA_LEVEL + 2 || column.base > 76
                {
                    continue;
                }
                let h = hash2(x, z, self.recipe.seed ^ 0xA24B_AED4);
                let material = match h % 1000 {
                    0..=14 => Some(mat::TALL_GRASS),
                    15..=19 => Some(mat::RED_FLOWER),
                    _ => None,
                };
                if let Some(material) = material
                    && column.base < max[1]
                {
                    self.extras.push(Extra {
                        x,
                        y: column.base + 1,
                        z,
                        material,
                    });
                }
            }
        }
    }

    fn build_trees(&mut self) {
        let seed = self.recipe.seed;
        let (min, max) = self.recipe.world_bounds();
        let cell = 20;
        let gx0 = min[0].div_euclid(cell) - 1;
        let gx1 = max[0].div_euclid(cell) + 1;
        let gz0 = min[2].div_euclid(cell) - 1;
        let gz1 = max[2].div_euclid(cell) + 1;
        for gz in gz0..=gz1 {
            for gx in gx0..=gx1 {
                let h = hash3(gx, 1, gz, seed ^ 0x68E3_1DA4);
                if h % 100 >= 42 {
                    continue;
                }
                let x = gx * cell + 2 + (h % 16) as i32;
                let z = gz * cell + 2 + ((h >> 8) % 16) as i32;
                let column = self.column(x, z);
                if column.ground != column.base || column.base <= SEA_LEVEL + 3 || column.base > 72
                {
                    continue;
                }
                let trunk = 4 + ((h >> 16) % 4) as i32;
                let canopy = 2 + ((h >> 20) % 2) as i32;
                self.add_tree(x, column.base, z, trunk, canopy);
            }
        }
        // One landmark tree, so the fixture also carries a large canopy.
        let x = 128;
        let z = 128;
        let column = self.column(x, z);
        if column.ground == column.base {
            self.add_tree(x, column.base, z, 13, 5);
        }
    }

    fn add_tree(&mut self, x: i32, base: i32, z: i32, trunk: i32, canopy: i32) {
        for y in base + 1..=base + trunk {
            self.extras.push(Extra {
                x,
                y,
                z,
                material: mat::OAK_LOG,
            });
        }
        let centre = base + trunk;
        for dy in -1..=canopy {
            for dz in -canopy..=canopy {
                for dx in -canopy..=canopy {
                    let distance = dx * dx + dy * dy + dz * dz;
                    if distance > canopy * canopy + 2 {
                        continue;
                    }
                    if dx == 0 && dz == 0 && dy <= 0 {
                        continue;
                    }
                    self.extras.push(Extra {
                        x: x + dx,
                        y: centre + dy,
                        z: z + dz,
                        material: mat::OAK_LEAVES,
                    });
                }
            }
        }
    }

    fn build_structures(&mut self) {
        self.build_tower(-72, -32);
        self.build_ruin(96, -96);
        self.build_bridge(72);
    }

    fn build_tower(&mut self, x: i32, z: i32) {
        let column = self.column(x, z);
        if column.ground != column.base {
            return;
        }
        let base = column.base;
        let half = 4;
        for y in base - 2..=base {
            for dz in -half - 1..=half + 1 {
                for dx in -half - 1..=half + 1 {
                    self.extras.push(Extra {
                        x: x + dx,
                        y,
                        z: z + dz,
                        material: mat::COBBLESTONE,
                    });
                }
            }
        }
        for y in base + 1..=base + 21 {
            for dz in -half..=half {
                for dx in -half..=half {
                    let edge = dx.abs() == half || dz.abs() == half;
                    if !edge {
                        self.extras.push(Extra {
                            x: x + dx,
                            y,
                            z: z + dz,
                            material: mat::AIR,
                        });
                        continue;
                    }
                    let door = dz == half && dx.abs() <= 1 && y <= base + 3;
                    let window = y >= base + 8
                        && y <= base + 9
                        && (dx.abs() == 1 || dz.abs() == 1)
                        && (dx == 0 || dz == 0);
                    let material = if door || window {
                        mat::AIR
                    } else if dx.abs() == half && dz.abs() == half {
                        mat::OAK_LOG
                    } else if y % 7 == 0 {
                        mat::SANDSTONE
                    } else {
                        mat::COBBLESTONE
                    };
                    self.extras.push(Extra {
                        x: x + dx,
                        y,
                        z: z + dz,
                        material,
                    });
                }
            }
        }
        for dz in -half..=half {
            for dx in -half..=half {
                if (dx.abs() != half && dz.abs() != half) || (dx + dz).rem_euclid(2) == 0 {
                    continue;
                }
                self.extras.push(Extra {
                    x: x + dx,
                    y: base + 22,
                    z: z + dz,
                    material: mat::COBBLESTONE,
                });
            }
        }
    }

    fn build_ruin(&mut self, x: i32, z: i32) {
        let column = self.column(x, z);
        if column.ground != column.base {
            return;
        }
        let base = column.base;
        let seed = self.recipe.seed;
        for dz in -4i32..=4 {
            for dx in -4i32..=4 {
                if dx.abs() != 4 && dz.abs() != 4 {
                    if hash2(x + dx, z + dz, seed ^ 0x147A_9E37) % 100 < 30 {
                        self.extras.push(Extra {
                            x: x + dx,
                            y: base,
                            z: z + dz,
                            material: mat::SANDSTONE,
                        });
                    }
                    continue;
                }
                let height = 1 + hash2(x + dx, z + dz, seed ^ 0xB529_7A4D) % 5;
                for y in base..base + height as i32 {
                    if hash3(x + dx, y, z + dz, seed) % 100 >= 78 {
                        continue;
                    }
                    self.extras.push(Extra {
                        x: x + dx,
                        y,
                        z: z + dz,
                        material: if y % 3 == 0 {
                            mat::SANDSTONE
                        } else {
                            mat::COBBLESTONE
                        },
                    });
                }
            }
        }
        for i in 0..18 {
            let h = hash3(i, 3, 9, seed ^ 0x2545_F491);
            let dx = (h % 13) as i32 - 6;
            let dz = ((h >> 8) % 13) as i32 - 6;
            self.extras.push(Extra {
                x: x + dx,
                y: base,
                z: z + dz,
                material: if i % 3 == 0 {
                    mat::COBBLESTONE
                } else {
                    mat::SANDSTONE
                },
            });
        }
    }

    fn build_bridge(&mut self, z: i32) {
        let centre = 8 + (noise1(z, 96, self.recipe.seed ^ 0x2545_F491) - 500) * 20 / 1000;
        let base = self.surface_height(centre, z);
        let deck = base - 1;
        for dx in -14..=14 {
            for dz in -1..=1 {
                self.extras.push(Extra {
                    x: centre + dx,
                    y: deck,
                    z: z + dz,
                    material: if dz == 0 {
                        mat::OAK_LOG
                    } else {
                        mat::COBBLESTONE
                    },
                });
            }
        }
        for dx in (-14i32..=14).step_by(3) {
            for dz in [-1, 1] {
                for y in deck + 1..=deck + 2 {
                    self.extras.push(Extra {
                        x: centre + dx,
                        y,
                        z: z + dz,
                        material: mat::OAK_LOG,
                    });
                }
            }
        }
        for dx in [-14i32, -7, 7, 14] {
            for y in base - 16..deck {
                for dz in -1..=1 {
                    self.extras.push(Extra {
                        x: centre + dx,
                        y,
                        z: z + dz,
                        material: mat::COBBLESTONE,
                    });
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Column {
    base: i32,
    ground: i32,
    shelf: Option<(i32, i32)>,
}

/// True when the recipe contains the five feature families the task asks for.
pub fn recipe_features() -> [Feature; 5] {
    [
        Feature::Terrain,
        Feature::Caves,
        Feature::Overhangs,
        Feature::Structures,
        Feature::MixedMaterials,
    ]
}

fn hash2(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (z as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ (z as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn noise1(z: i32, cell: i32, seed: u64) -> i32 {
    noise2(0, z, cell, seed)
}

/// Integer value noise in `0..=1000` with smoothstep interpolation.
fn noise2(x: i32, z: i32, cell: i32, seed: u64) -> i32 {
    let cell = cell.max(1);
    let gx = x.div_euclid(cell);
    let gz = z.div_euclid(cell);
    let fx = (x.rem_euclid(cell) as i64 * 1024 / cell as i64) as i32;
    let fz = (z.rem_euclid(cell) as i64 * 1024 / cell as i64) as i32;
    let v00 = (hash2(gx, gz, seed) % 1001) as i32;
    let v10 = (hash2(gx + 1, gz, seed) % 1001) as i32;
    let v01 = (hash2(gx, gz + 1, seed) % 1001) as i32;
    let v11 = (hash2(gx + 1, gz + 1, seed) % 1001) as i32;
    let sx = smooth(fx);
    let sz = smooth(fz);
    let low = lerp(v00, v10, sx);
    let high = lerp(v01, v11, sx);
    lerp(low, high, sz)
}

fn smooth(t: i32) -> i32 {
    let t = t as i64;
    ((t * t * (3 * 1024 - 2 * t)) / (1024 * 1024)) as i32
}

fn lerp(a: i32, b: i32, t: i32) -> i32 {
    a + ((b - a) as i64 * t as i64 / 1024) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_block::BlockKind;

    #[test]
    fn palette_matches_the_builtin_registration_order() {
        assert_eq!(MATERIALS.len(), BlockKind::ALL.len());
        for (material, kind) in MATERIALS.iter().zip(BlockKind::ALL) {
            assert_eq!(material.key, kind.key().as_str());
        }
        for (index, material) in MATERIALS.iter().enumerate() {
            assert_eq!(material.id as usize, index);
        }
    }

    #[test]
    fn noise_and_surface_are_deterministic() {
        let recipe = Recipe::default();
        let generator = WorldGenerator::new(&recipe).unwrap();
        for (x, z) in [(0, 0), (-96, 96), (128, 128), (-192, 191)] {
            assert_eq!(
                generator.surface_height(x, z),
                generator.surface_height(x, z)
            );
            assert!((8..=116).contains(&generator.surface_height(x, z)));
        }
        assert_eq!(noise2(3, 7, 16, 42), noise2(3, 7, 16, 42));
        assert_ne!(noise2(3, 7, 16, 42), noise2(3, 8, 16, 42));
    }

    #[test]
    fn one_chunk_is_byte_stable_and_has_air_and_solid() {
        let recipe = Recipe::default();
        let generator = WorldGenerator::new(&recipe).unwrap();
        let pos = ChunkPos::new(-1, 0, -1);
        let mut first = vec![0; CHUNK_VOLUME];
        let mut second = vec![0; CHUNK_VOLUME];
        generator.fill(pos, &mut first);
        generator.fill(pos, &mut second);
        assert_eq!(first, second);
        assert!(first.contains(&mat::AIR));
        assert!(first.iter().any(|&m| m != mat::AIR));
    }
}
