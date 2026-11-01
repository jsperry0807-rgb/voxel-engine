//! M3-08: the seeded 256-chunk stress scene.
//!
//! One [`Scene`] is a pure function of a seed and a world position. It produces
//! five **terrain variants** (grass meadow, forest lowland, sand bar, snow
//! highland, rocky slope) and three **canopy variants** (none, scattered oaks,
//! forest oaks), all from the M1.5-14 material set. The same scene fills the
//! chunk volume and the one-voxel apron, so chunk borders and canopy edges
//! agree without any neighbour lookup.
//!
//! # Determinism
//!
//! Every value is derived from [`hash_to_unit`] over world coordinates and the
//! seed: there is no ordered state, no clock and no thread interaction. Two
//! runs with the same seed produce byte-identical volumes, and the seed is the
//! only input. [`Scene::from_env`] reads `VOXEL_SCENE_SEED` (decimal or `0x`
//! hex); the viewer logs the resolved seed at startup.
//!
//! # Materials
//!
//! A chunk volume stores **block ids**, not atlas slots. [`atlas_slot`] resolves
//! one block and one face to the M1.5-14 atlas slot (for example the grass top
//! and the grass side are different slots), so the mesher can carry per-face
//! materials. The block ids are scene-local and never leave the CPU side.
//!
//! # Canopy and the u16 index stream
//!
//! One chunk mesh is indexed by `u16`, so a chunk may emit at most 16,384
//! quads (`tests/scene.rs` enforces this over the whole ring for several
//! seeds). Trees therefore stand on flattened forest lowlands, where greedy
//! meshing merges the floor into few quads, and the tree grid is coarse enough
//! that a 128³ chunk stays inside the limit. Dense canopies need meshlets; that
//! is later M3 work.

use std::collections::HashMap;

use voxel_core::Face;
use voxel_core::constants::{CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME};
use voxel_storage::apron::{Face as ApronFace, SIDE};
use voxel_storage::{Apron, ChunkPos};

/// The seed used when `VOXEL_SCENE_SEED` is unset.
pub const DEFAULT_SEED: u32 = 0x5EED_1234;
/// The environment variable that overrides the scene seed.
pub const SEED_ENV: &str = "VOXEL_SCENE_SEED";

/// Air. Every other id is solid.
pub const AIR: u16 = 0;
/// The bottom layer, so a chunk never shows through to the void.
pub const BEDROCK: u16 = 1;
/// Deep filler.
pub const STONE: u16 = 2;
/// The layer under a grass or snow surface.
pub const DIRT: u16 = 3;
/// A grass surface block.
pub const GRASS: u16 = 4;
/// A sand surface or filler block.
pub const SAND: u16 = 5;
/// The layer under sand.
pub const SANDSTONE: u16 = 6;
/// A snow surface block.
pub const SNOW: u16 = 7;
/// Eroded ground on a rocky slope.
pub const GRAVEL: u16 = 8;
/// Eroded ground on a rocky slope.
pub const COBBLESTONE: u16 = 9;
/// An oak trunk.
pub const OAK_LOG: u16 = 10;
/// An oak canopy.
pub const OAK_LEAVES: u16 = 11;

/// One of the scene's terrain variants.
///
/// The variant changes the surface stack and the canopy density; the height
/// field stays the same so the variants meet without seams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    /// Grass with scattered oaks.
    Meadow,
    /// Flattened grass lowland with a canopy.
    Forest,
    /// Sand over sandstone on the low ground.
    Sandbar,
    /// Snow over stone on the high ground.
    Highland,
    /// Gravel and cobblestone on eroded slopes.
    Rocky,
}

impl Biome {
    /// Every variant, in a fixed order.
    pub const ALL: [Biome; 5] = [
        Biome::Meadow,
        Biome::Forest,
        Biome::Sandbar,
        Biome::Highland,
        Biome::Rocky,
    ];

    /// A short stable name for logs and tests.
    pub fn name(self) -> &'static str {
        match self {
            Biome::Meadow => "meadow",
            Biome::Forest => "forest",
            Biome::Sandbar => "sandbar",
            Biome::Highland => "highland",
            Biome::Rocky => "rocky",
        }
    }
}

/// One oak tree: a trunk at `(x, z)` and a canopy around its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tree {
    /// World column of the trunk.
    pub x: i32,
    /// World column of the trunk.
    pub z: i32,
    /// World height of the lowest trunk block (the ground surface).
    pub base: i32,
    /// World height of the highest trunk block.
    pub top: i32,
    /// Canopy radius in columns at its widest layer.
    pub radius: i32,
}

impl Tree {
    /// Bounds of the voxels this tree can occupy, inclusive.
    pub fn bounds(&self) -> ([i32; 3], [i32; 3]) {
        let r = self.radius;
        (
            [self.x - r, self.base, self.z - r],
            [self.x + r, self.top + 2, self.z + r],
        )
    }

    /// The block at one world voxel, or `None` when this tree does not cover it.
    ///
    /// The trunk wins over the canopy, and the canopy is a rounded blob: three
    /// layers at the widest radius and a narrower cap above the trunk.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        let dx = x - self.x;
        let dz = z - self.z;
        if dx.abs() > self.radius || dz.abs() > self.radius {
            return None;
        }
        if (self.base..=self.top).contains(&y) && dx == 0 && dz == 0 {
            return Some(OAK_LOG);
        }
        // Canopy: three full layers starting one below the top, then a cap.
        let layer = y - self.top;
        let radius = match layer {
            -1..=1 => self.radius,
            2 => self.radius - 1,
            _ => return None,
        };
        if dx.abs() == radius && dz.abs() == radius && radius > 1 {
            // Round the corners of the widest layers.
            return None;
        }
        Some(OAK_LEAVES)
    }
}

/// A deterministic, seeded scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    seed: u32,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new(DEFAULT_SEED)
    }
}

impl Scene {
    /// Worlds per tree candidate cell, on each horizontal axis.
    pub const TREE_CELL: i32 = 24;
    /// How far a trunk may sit from the centre of its cell.
    pub const TREE_JITTER: i32 = 6;
    /// The widest canopy radius [`Self::tree_at_cell`] can produce.
    pub const MAX_CANOPY: i32 = 4;

    /// Builds the scene for `seed`.
    pub const fn new(seed: u32) -> Self {
        Self { seed }
    }

    /// Reads `VOXEL_SCENE_SEED`, falling back to [`DEFAULT_SEED`].
    ///
    /// A malformed value is a startup error rather than a silent default: a
    /// scene that does not match its screenshot is worse than a failed launch.
    pub fn from_env() -> Self {
        match std::env::var(SEED_ENV) {
            Ok(text) => match parse_seed(&text) {
                Some(seed) => Self::new(seed),
                None => panic!("{SEED_ENV}={text:?} is not a u32 (decimal or 0x hex)"),
            },
            Err(_) => Self::default(),
        }
    }

    /// The seed this scene was built from.
    pub const fn seed(&self) -> u32 {
        self.seed
    }

    /// World-voxel surface height for a terrain column.
    ///
    /// A smooth, world-coherent surface: three noise octaves plus a broad
    /// continent mask. Forest lowlands are blended toward a flat floor so
    /// greedy meshing can merge them and leave index headroom for the canopy.
    /// The column under the default camera is flattened so the viewer does not
    /// start inside a hill.
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.column(x, z).height
    }

    /// The terrain variant at a world column.
    ///
    /// Altitude decides the extremes (low is sand, high is snow); moisture
    /// decides the forest; erodible noise decides the rocky slopes.
    pub fn biome(&self, x: i32, z: i32) -> Biome {
        let column = self.column(x, z);
        self.biome_of(x, z, &column)
    }

    /// Everything one column contributes, from one noise evaluation.
    ///
    /// Terrain fill, the apron and the public [`Self::height`]/[`Self::biome`]
    /// all read this, so a column cannot disagree with itself.
    fn column(&self, x: i32, z: i32) -> Column {
        let broad = value_noise(x as f32 / 96.0, z as f32 / 96.0, self.seed ^ 0x51D1_0001);
        let noise = value_noise(x as f32 / 48.0, z as f32 / 48.0, self.seed);
        let detail = value_noise(x as f32 / 19.0, z as f32 / 19.0, self.seed ^ 0xC0FF_EE01);
        let relief = 0.55 * broad + 0.30 * noise + 0.15 * detail;

        // The default camera starts at world voxel (72, 96): flatten the column
        // there so the view is not born inside a hill.
        let dx = (x - 72) as f32;
        let dz = (z - 96) as f32;
        let basin = 1.0 - (dx * dx + dz * dz) / (3.0 * CHUNK_DIM as f32).powi(2).max(1.0);
        let raw = 12.0 + (10.0 + relief * 32.0 - 12.0) * (1.0 - 0.55 * basin.max(0.0));

        let moisture = self.moisture(x, z);
        let forest = smoothstep(((moisture - 0.50) / 0.14).clamp(0.0, 1.0));
        let floor = 16.0 + (relief - 0.5) * 6.0;
        let height = raw + (floor - raw) * forest;

        let ceiling = (CHUNK_DIM as i32 - 9).max(2);
        Column {
            height: (height.round() as i32).clamp(1, ceiling),
            altitude: (raw.round() as i32).clamp(1, ceiling),
            moisture,
        }
    }

    /// The smooth moisture field, shared by the height blend and [`Self::biome`].
    fn moisture(&self, x: i32, z: i32) -> f32 {
        value_noise(x as f32 / 64.0, z as f32 / 64.0, self.seed ^ 0x00A1_B2C3)
    }

    fn biome_of(&self, x: i32, z: i32, column: &Column) -> Biome {
        if column.altitude <= 19 {
            return Biome::Sandbar;
        }
        if column.altitude >= 31 {
            return Biome::Highland;
        }
        if column.moisture > 0.62 {
            return Biome::Forest;
        }
        if value_noise(x as f32 / 23.0, z as f32 / 23.0, self.seed ^ 0x0BAD_5EED) > 0.78 {
            return Biome::Rocky;
        }
        Biome::Meadow
    }

    /// The block at one world voxel: terrain, then canopy.
    ///
    /// This is the reference answer; [`Self::fill_volume`] and [`Self::apron`]
    /// are built from the same pieces and tests compare them against it.
    pub fn block(&self, x: i32, y: i32, z: i32) -> u16 {
        self.sampler().block(x, y, z)
    }

    /// A reusable single-voxel sampler.
    ///
    /// It memoizes tree decisions per cell, so sampling many voxels (tests,
    /// tools) does not re-evaluate the same candidates.
    pub fn sampler(&self) -> Sampler<'_> {
        Sampler::new(self)
    }

    /// Fills `volume` with one chunk's blocks, clearing it first.
    pub fn fill_volume(&self, chunk: ChunkPos, volume: &mut [u16]) {
        assert_eq!(volume.len(), CHUNK_VOLUME, "a chunk volume is CHUNK_VOLUME");
        volume.fill(AIR);
        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_y = chunk.y * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;

        // Terrain first: one surface and one variant per column.
        for local_z in 0..CHUNK_DIM {
            for local_x in 0..CHUNK_DIM {
                let world_x = base_x + local_x as i32;
                let world_z = base_z + local_z as i32;
                let column = self.column(world_x, world_z);
                let surface = column.height;
                let biome = self.biome_of(world_x, world_z, &column);
                let top = (surface - base_y).clamp(0, CHUNK_DIM as i32) as usize;
                for local_y in 0..top {
                    let world_y = base_y + local_y as i32;
                    let index = local_x | (local_y << CHUNK_BITS) | (local_z << (2 * CHUNK_BITS));
                    volume[index] = surface_block(biome, world_y, surface, world_x, world_z);
                }
            }
        }

        // Canopy second, stamped per tree. Trees never overlap (the grid keeps
        // trunks ≥ 12 columns apart and canopies are ≤ 7 wide), so the order is
        // irrelevant; tests pin the non-overlap.
        for tree in self.trees_touching(chunk) {
            let ([min_x, min_y, min_z], [max_x, max_y, max_z]) = tree.bounds();
            let start = [min_x.max(base_x), min_y.max(base_y), min_z.max(base_z)];
            let end = [
                max_x.min(base_x + CHUNK_DIM as i32 - 1),
                max_y.min(base_y + CHUNK_DIM as i32 - 1),
                max_z.min(base_z + CHUNK_DIM as i32 - 1),
            ];
            for world_z in start[2]..=end[2] {
                for world_y in start[1]..=end[1] {
                    for world_x in start[0]..=end[0] {
                        let Some(block) = tree.block_at(world_x, world_y, world_z) else {
                            continue;
                        };
                        let index = (world_x - base_x) as usize
                            | ((world_y - base_y) as usize) << CHUNK_BITS
                            | ((world_z - base_z) as usize) << (2 * CHUNK_BITS);
                        if volume[index] == AIR {
                            volume[index] = block;
                        }
                    }
                }
            }
        }
    }

    /// The chunk volume for `chunk`.
    pub fn volume(&self, chunk: ChunkPos) -> Vec<u16> {
        let mut volume = vec![AIR; CHUNK_VOLUME];
        self.fill_volume(chunk, &mut volume);
        volume
    }

    /// The six face planes around `chunk`, sampled from the same scene.
    ///
    /// The planes carry terrain and canopy, so a chunk meshed with this apron
    /// culls its border faces against the real neighbour and the border AO sees
    /// the real canopy.
    pub fn apron(&self, chunk: ChunkPos) -> Apron {
        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_y = chunk.y * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;
        let mut apron = Apron::new();
        let edge = CHUNK_DIM as i32;

        // Terrain on every plane.
        for face in ApronFace::ALL {
            for v in 0..SIDE {
                for u in 0..SIDE {
                    let (dx, dy, dz) = face.offset(u, v);
                    let block = self.terrain_block(base_x + dx, base_y + dy, base_z + dz);
                    apron.set(face, u, v, block);
                }
            }
        }

        // Canopy: stamp each nearby tree onto any plane it touches.
        for tree in self.trees_touching(chunk) {
            let ([min_x, min_y, min_z], [max_x, max_y, max_z]) = tree.bounds();
            for world_z in min_z..=max_z {
                for world_y in min_y..=max_y {
                    for world_x in min_x..=max_x {
                        let Some(block) = tree.block_at(world_x, world_y, world_z) else {
                            continue;
                        };
                        let local = [world_x - base_x, world_y - base_y, world_z - base_z];
                        write_shell(&mut apron, edge, local, block);
                    }
                }
            }
        }
        apron
    }

    /// The terrain block at one world voxel.
    fn terrain_block(&self, x: i32, y: i32, z: i32) -> u16 {
        if y < 0 {
            return AIR;
        }
        let column = self.column(x, z);
        if y >= column.height {
            return AIR;
        }
        surface_block(self.biome_of(x, z, &column), y, column.height, x, z)
    }

    /// The tree whose candidate cell is `(gx, gz)`, if any.
    ///
    /// Pure in `(seed, gx, gz)`: the same cell always produces the same tree,
    /// which is what lets neighbouring chunks and their aprons agree.
    pub fn tree_at_cell(&self, gx: i32, gz: i32) -> Option<Tree> {
        let cell = Self::TREE_CELL;
        let hash = cell_hash(self.seed, gx, gz);
        let jitter = |shift: u32| {
            let span = 2 * Self::TREE_JITTER + 1;
            ((hash >> shift) % span as u32) as i32 - Self::TREE_JITTER
        };
        let x = gx * cell + cell / 2 + jitter(0);
        let z = gz * cell + cell / 2 + jitter(9);
        let density = match self.biome(x, z) {
            Biome::Forest => 0.85,
            Biome::Meadow => 0.10,
            Biome::Sandbar | Biome::Highland | Biome::Rocky => 0.0,
        };
        if unit(hash >> 18) >= density {
            return None;
        }
        let base = self.height(x, z);
        let trunk = 6 + ((hash >> 28) % 4) as i32;
        let radius = 3 + (((hash >> 31) & 1) as i32);
        Some(Tree {
            x,
            z,
            base,
            top: base + trunk - 1,
            radius,
        })
    }

    /// Every tree whose voxels can land in `chunk` or on its apron.
    pub fn trees_touching(&self, chunk: ChunkPos) -> Vec<Tree> {
        let cell = Self::TREE_CELL;
        // A tree can cover a column up to `MAX_CANOPY` from its trunk, and a
        // trunk sits up to `TREE_JITTER` from its cell centre. Enumerating cells
        // by the chunk's footprint expanded by that reach finds every tree that
        // can touch the volume or its apron, including trees rooted in the next
        // cell over.
        let margin = cell / 2 + Self::TREE_JITTER + Self::MAX_CANOPY + 1;
        let base_x = chunk.x * CHUNK_DIM as i32;
        let base_z = chunk.z * CHUNK_DIM as i32;
        let min_gx = (base_x - margin).div_euclid(cell);
        let max_gx = (base_x + CHUNK_DIM as i32 + margin).div_euclid(cell);
        let min_gz = (base_z - margin).div_euclid(cell);
        let max_gz = (base_z + CHUNK_DIM as i32 + margin).div_euclid(cell);
        let mut trees = Vec::new();
        for gz in min_gz..=max_gz {
            for gx in min_gx..=max_gx {
                if let Some(tree) = self.tree_at_cell(gx, gz) {
                    trees.push(tree);
                }
            }
        }
        trees
    }
}

/// One column's derived terrain fields.
struct Column {
    /// The surface height the terrain is built to.
    height: i32,
    /// The unflattened height the biome classifier reads.
    altitude: i32,
    /// The moisture field, in `0.0..=1.0`.
    moisture: f32,
}

/// A memoized single-voxel scene sampler.
#[derive(Debug)]
pub struct Sampler<'a> {
    scene: &'a Scene,
    cells: HashMap<(i32, i32), Option<Tree>>,
}

impl<'a> Sampler<'a> {
    fn new(scene: &'a Scene) -> Self {
        Self {
            scene,
            cells: HashMap::new(),
        }
    }

    /// The block at one world voxel: terrain, then canopy.
    pub fn block(&mut self, x: i32, y: i32, z: i32) -> u16 {
        let terrain = self.scene.terrain_block(x, y, z);
        if terrain != AIR {
            return terrain;
        }
        self.canopy_at(x, y, z)
    }

    fn canopy_at(&mut self, x: i32, y: i32, z: i32) -> u16 {
        let cell = Scene::TREE_CELL;
        let margin = Scene::TREE_JITTER + Scene::MAX_CANOPY;
        for gz in (z - margin).div_euclid(cell)..=(z + margin).div_euclid(cell) {
            for gx in (x - margin).div_euclid(cell)..=(x + margin).div_euclid(cell) {
                let tree = *self
                    .cells
                    .entry((gx, gz))
                    .or_insert_with(|| self.scene.tree_at_cell(gx, gz));
                if let Some(tree) = tree
                    && let Some(block) = tree.block_at(x, y, z)
                {
                    return block;
                }
            }
        }
        AIR
    }
}

/// The atlas slot for one block face, using only the M1.5-14 material set.
///
/// Slots follow `crates/engine/block/data/faces.ron`; `tests/scene.rs` pins the
/// whole map against the committed face table.
pub fn atlas_slot(block: u16, face: Face) -> u16 {
    match (block, face) {
        (AIR, _) => 0,
        (BEDROCK, _) => 12,
        (STONE, _) => 1,
        (DIRT, _) => 2,
        (GRAVEL, _) => 7,
        (COBBLESTONE, _) => 5,
        (SAND, _) => 6,
        (SNOW, _) => 19,
        (OAK_LEAVES, _) => 11,
        (GRASS, Face::Top) => 3,
        (GRASS, Face::Bottom) => 2,
        (GRASS, _) => 4,
        (OAK_LOG, Face::Top | Face::Bottom) => 9,
        (OAK_LOG, _) => 10,
        (SANDSTONE, Face::Top) => 21,
        (SANDSTONE, Face::Bottom) => 22,
        (SANDSTONE, _) => 23,
        // The volume only ever contains the blocks above; a new id must extend
        // this map and the material tests rather than fall through silently.
        _ => unreachable!("no M1.5-14 slot for block id {block}"),
    }
}

/// The surface stack at one solid voxel below `surface`.
fn surface_block(biome: Biome, y: i32, surface: i32, x: i32, z: i32) -> u16 {
    if y <= 0 {
        return BEDROCK;
    }
    let depth = surface - 1 - y;
    match biome {
        Biome::Meadow | Biome::Forest => match depth {
            0 => GRASS,
            1..=3 => DIRT,
            _ => STONE,
        },
        Biome::Sandbar => match depth {
            0..=2 => SAND,
            3..=6 => SANDSTONE,
            _ => STONE,
        },
        Biome::Highland => match depth {
            0 => SNOW,
            _ => STONE,
        },
        Biome::Rocky => match depth {
            0..=2 => {
                if hash_to_unit(x, z, 0x600D_5EED) > 0.5 {
                    GRAVEL
                } else {
                    COBBLESTONE
                }
            }
            _ => STONE,
        },
    }
}

/// Writes `block` into every apron plane that contains the chunk-local voxel
/// `local`.
///
/// A voxel on a chunk edge can belong to two planes (an edge or a corner
/// sample), so each plane is checked independently, exactly like the per-plane
/// sampling loop in [`Scene::apron`].
fn write_shell(apron: &mut Apron, edge: i32, local: [i32; 3], block: u16) {
    let [x, y, z] = local;
    let side = SIDE as i32;
    let inside = |u: i32, v: i32| (0..side).contains(&u) && (0..side).contains(&v);
    if x == -1 && inside(z, y) {
        apron.set(ApronFace::NegX, z as u32, y as u32, block);
    }
    if x == edge && inside(z, y) {
        apron.set(ApronFace::PosX, z as u32, y as u32, block);
    }
    if y == -1 && inside(z, x) {
        apron.set(ApronFace::NegY, z as u32, x as u32, block);
    }
    if y == edge && inside(z, x) {
        apron.set(ApronFace::PosY, z as u32, x as u32, block);
    }
    if z == -1 && inside(x, y) {
        apron.set(ApronFace::NegZ, x as u32, y as u32, block);
    }
    if z == edge && inside(x, y) {
        apron.set(ApronFace::PosZ, x as u32, y as u32, block);
    }
}

/// Parses a decimal or `0x`-prefixed seed.
pub fn parse_seed(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// A stable hash of `(seed, gx, gz)` for tree decisions.
fn cell_hash(seed: u32, gx: i32, gz: i32) -> u32 {
    let mut hash = (gx as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((gz as u32).wrapping_mul(0x85EB_CA77))
        .wrapping_add(seed);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x2974_C1E7);
    hash ^ (hash >> 15)
}

/// Maps the top bits of `hash` to `0.0..1.0`.
fn unit(hash: u32) -> f32 {
    (hash & 0xFFFF) as f32 / 65_535.0
}

/// Smooth 2D value noise in `0.0..=1.0`.
pub fn value_noise(x: f32, z: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let tx = smoothstep(x - x0);
    let tz = smoothstep(z - z0);
    let x0 = x0 as i32;
    let z0 = z0 as i32;
    let a = hash_to_unit(x0, z0, seed);
    let b = hash_to_unit(x0 + 1, z0, seed);
    let c = hash_to_unit(x0, z0 + 1, seed);
    let d = hash_to_unit(x0 + 1, z0 + 1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * tz
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn hash_to_unit(x: i32, z: i32, seed: u32) -> f32 {
    let mut hash = (x as u32)
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add((z as u32).wrapping_mul(0x85EB_CA77))
        .wrapping_add(seed);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    hash ^= hash >> 12;
    unit(hash)
}
