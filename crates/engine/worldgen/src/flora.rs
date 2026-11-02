//! Flora placement: biome species, ecotone species and rare giants.
//!
//! Flora is content-driven: every biome's `flora` entries in `data/biomes.ron`
//! carry a species key and a density, and its `ecotone` list names the species
//! that may appear only at a seam.
//!
//! # Scales
//!
//! - **Plants** (`tall_grass`, `red_flower`, `fern`, `cactus`, `dead_bush`) are
//!   rolled per 1 m column at the biome density.
//! - **Trees** (`oak_log`, `spruce_log`, `birch_log`, ...) are rolled once per
//!   **8 m tree cell**, so a 0.08 forest density is about one tree per 800 m²
//!   rather than one per 12.5 m².
//! - **Giants** use a **32 km cell**, inside the design's 20–50 km spacing. A
//!   tree biome gets at most one giant per cell, with height 30–60 m.
//!
//! # Seams and ecotones
//!
//! The dominant biome is the M2.5-10 softmax winner. A column is a **seam**
//! when the runner-up weight is at least [`SEAM_WEIGHT`] (0.2); that is the
//! middle of the 300–500 m blend band. Only a seam may draw from the dominant
//! biome's `ecotone` list. Birch is therefore only at a forest/taiga seam, and
//! the ecotone dead bush only at a seam; the normal desert/savanna dead-bush
//! entries are separate, non-ecotone placements.
//!
//! A tree ecotone still has to land on a tree cell, so birch does not carpet
//! every seam column.
//!
//! # Keys
//!
//! Every species and ecotone key in the catalogue is resolved through the
//! committed block manifest by the integration test. Tree log keys map to their
//! leaf keys (`oak_log` → `oak_leaves`, and so on).

use crate::biomes::{BiomeClass, BiomeDef, FloraEntry, catalog};
use crate::fixed::{Coord, ONE};
use crate::generator::MacroGenerator;
use crate::hash;
use crate::splines::SEA_LEVEL;
use crate::weights::{Axes, BiomeWeights};

/// Tree cell edge, in metres.
pub const TREE_CELL_M: i64 = 8;

/// Giant-tree cell edge, in metres. Inside the design's 20–50 km spacing.
pub const GIANT_CELL_M: i64 = 32_000;

/// Runner-up weight at or above which a column is an ecotone seam, Q16.16.
pub const SEAM_WEIGHT: i32 = ONE / 5;

/// Ecotone-species chance per eligible column, Q16.16.
pub const ECOTONE_DENSITY: i32 = ONE * 8 / 100;

/// Steepest surface that can carry flora, Q16.16 rise over run.
pub const FLORA_MAX_SLOPE: i32 = ONE * 45 / 100;

/// Shortest giant, in blocks.
pub const GIANT_MIN_HEIGHT: u8 = 30;

/// Tallest giant, in blocks.
pub const GIANT_MAX_HEIGHT: u8 = 60;

const FLORA_SALT: u64 = 0x464C_4F52_0001;
const TREE_SALT: u64 = 0x464C_4F52_0002;
const GIANT_SALT: u64 = 0x464C_4F52_0003;

/// Ground plant or tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloraKind {
    /// A one-block plant.
    Plant,
    /// A trunk-and-canopy tree.
    Tree,
}

/// One flora placement at a surface column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloraPlacement {
    /// World block x.
    pub x: Coord,
    /// World block z.
    pub z: Coord,
    /// Surface height, Q16.16 metres.
    pub surface: i32,
    /// Species key from the biome catalogue.
    pub species: &'static str,
    /// Plant or tree.
    pub kind: FloraKind,
    /// Trunk/plant height in blocks.
    pub height: u8,
    /// True for a rare giant tree.
    pub giant: bool,
    /// True when the species came from the ecotone list at a seam.
    pub ecotone: bool,
    /// Dominant biome index.
    pub biome: usize,
}

impl FloraPlacement {
    /// The canopy key for a tree, or `None` for a plant.
    pub fn leaves(self) -> Option<&'static str> {
        if self.kind == FloraKind::Tree {
            leaves_for(self.species)
        } else {
            None
        }
    }
}

/// The macro context for one flora column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloraContext {
    /// Surface height, Q16.16 metres.
    pub surface: i32,
    /// Continentalness in `[-ONE, ONE]`.
    pub continentalness: i32,
    /// Erosion in `[0, ONE]`.
    pub erosion: i32,
    /// Temperature channel.
    pub temperature: u8,
    /// Humidity channel.
    pub humidity: u8,
    /// Local slope, Q16.16 rise over run.
    pub slope: i32,
}

impl FloraContext {
    /// Sample the macro fields at a world position.
    pub fn from_generator(generator: &MacroGenerator, x: Coord, z: Coord) -> Self {
        let surface = generator.surface(x, z);
        let continentalness = generator.continentalness(x, z);
        let erosion = generator.erosion(x, z);
        let temperature = generator.temperature(x, z, surface);
        let humidity = generator.humidity(x, z, continentalness);
        let step = 4 << 16;
        let dx = (generator.surface(x + step, z) - generator.surface(x - step, z)).abs();
        let dz = (generator.surface(x, z + step) - generator.surface(x, z - step)).abs();
        // The samples are ±4 m apart, so the span is 8 m: Q16.16 metres / 8 m
        // is Q16.16 rise over run. Dividing by 4 would double every slope.
        let slope = (dx.max(dz) / 8).clamp(0, ONE);
        Self {
            surface,
            continentalness,
            erosion,
            temperature,
            humidity,
            slope,
        }
    }
}

/// The seeded flora field.
#[derive(Clone, Copy, Debug)]
pub struct FloraField {
    seed: u64,
}

impl FloraField {
    /// Create a field from a world seed.
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The seed this field was created with.
    pub const fn seed(self) -> u64 {
        self.seed
    }

    /// The anchor of a normal tree cell, in Q16.16 metres.
    pub fn tree_anchor(&self, cell_x: i32, cell_z: i32) -> (Coord, Coord) {
        let h = hash::hash2(self.seed ^ TREE_SALT, cell_x as i64, cell_z as i64);
        (
            ((cell_x as i64 * TREE_CELL_M) + (h % TREE_CELL_M as u64) as i64) << 16,
            ((cell_z as i64 * TREE_CELL_M) + ((h >> 16) % TREE_CELL_M as u64) as i64) << 16,
        )
    }

    /// The anchor of a giant-tree cell, in Q16.16 metres.
    ///
    /// The anchor sits 8–24 km inside its 32 km cell, so neighbouring giant
    /// cells are still 32 km apart.
    pub fn giant_anchor(&self, cell_x: i32, cell_z: i32) -> (Coord, Coord) {
        let h = hash::hash2(self.seed ^ GIANT_SALT, cell_x as i64, cell_z as i64);
        (
            ((cell_x as i64 * GIANT_CELL_M) + 8_000 + (h % 16_000) as i64) << 16,
            ((cell_z as i64 * GIANT_CELL_M) + 8_000 + ((h >> 32) % 16_000) as i64) << 16,
        )
    }

    /// Place one flora feature at a surface column, if any.
    pub fn sample(&self, x: Coord, z: Coord, context: &FloraContext) -> Option<FloraPlacement> {
        if context.surface <= SEA_LEVEL || context.slope > FLORA_MAX_SLOPE {
            return None;
        }

        let axes = Axes::surface(
            context.continentalness,
            context.temperature,
            context.humidity,
            context.erosion,
        );
        let weights = BiomeWeights::at(&axes);
        let dominant = weights.dominant();
        let biome: &'static BiomeDef = &catalog().biomes[dominant];
        if biome.class != BiomeClass::Land {
            return None;
        }

        let second = runner_up(&weights);
        let seam = second >= SEAM_WEIGHT;
        let xm = x >> 16;
        let zm = z >> 16;
        let h = hash::hash2(self.seed ^ FLORA_SALT, xm, zm);

        // A giant is checked first and overrides the normal rolls at its anchor.
        let giant_cell_x = xm.div_euclid(GIANT_CELL_M);
        let giant_cell_z = zm.div_euclid(GIANT_CELL_M);
        let (giant_x, giant_z) = self.giant_anchor(giant_cell_x as i32, giant_cell_z as i32);
        if x == giant_x
            && z == giant_z
            && let Some(entry) = pick_tree_any(biome, h)
        {
            let height = GIANT_MIN_HEIGHT
                + ((h >> 40) % (GIANT_MAX_HEIGHT - GIANT_MIN_HEIGHT + 1) as u64) as u8;
            return Some(place(
                x,
                z,
                context.surface,
                entry.species.as_str(),
                height,
                true,
                false,
                dominant,
            ));
        }

        // Ecotones only exist on a seam. A tree ecotone still needs a tree cell.
        if seam && let Some(species) = pick_ecotone(biome, h) {
            let tree = is_tree_key(species);
            let on_tree_cell = self.is_tree_anchor(x, z);
            if !tree || on_tree_cell {
                let height = if tree { normal_tree_height(h) } else { 1 };
                return Some(place(
                    x,
                    z,
                    context.surface,
                    species,
                    height,
                    false,
                    true,
                    dominant,
                ));
            }
        }

        if self.is_tree_anchor(x, z)
            && let Some(entry) = pick_tree(biome, h ^ 0x5452_4545_0001)
        {
            let height = normal_tree_height(h);
            return Some(place(
                x,
                z,
                context.surface,
                entry.species.as_str(),
                height,
                false,
                false,
                dominant,
            ));
        }

        if let Some(entry) = pick_plant(biome, h) {
            return Some(place(
                x,
                z,
                context.surface,
                entry.species.as_str(),
                1,
                false,
                false,
                dominant,
            ));
        }
        None
    }

    fn is_tree_anchor(&self, x: Coord, z: Coord) -> bool {
        let xm = x >> 16;
        let zm = z >> 16;
        let cell_x = xm.div_euclid(TREE_CELL_M);
        let cell_z = zm.div_euclid(TREE_CELL_M);
        let (ax, az) = self.tree_anchor(cell_x as i32, cell_z as i32);
        x == ax && z == az
    }
}

/// Map a tree log key to its leaf key.
pub fn leaves_for(species: &str) -> Option<&'static str> {
    match species {
        "voxel:oak_log" => Some("voxel:oak_leaves"),
        "voxel:birch_log" => Some("voxel:birch_leaves"),
        "voxel:spruce_log" => Some("voxel:spruce_leaves"),
        "voxel:dark_oak_log" => Some("voxel:dark_oak_leaves"),
        _ => None,
    }
}

/// True for a tree log key.
pub fn is_tree_key(key: &str) -> bool {
    key.ends_with("_log")
}

#[allow(clippy::too_many_arguments)]
fn place(
    x: Coord,
    z: Coord,
    surface: i32,
    species: &'static str,
    height: u8,
    giant: bool,
    ecotone: bool,
    biome: usize,
) -> FloraPlacement {
    FloraPlacement {
        x,
        z,
        surface,
        species,
        kind: if is_tree_key(species) {
            FloraKind::Tree
        } else {
            FloraKind::Plant
        },
        height,
        giant,
        ecotone,
        biome,
    }
}

fn runner_up(weights: &BiomeWeights) -> i32 {
    let mut first = 0;
    let mut second = 0;
    for weight in weights.weights() {
        if *weight >= first {
            second = first;
            first = *weight;
        } else if *weight > second {
            second = *weight;
        }
    }
    second
}

fn pick_tree_any(biome: &'static BiomeDef, h: u64) -> Option<&'static FloraEntry> {
    let entries: Vec<&'static FloraEntry> = biome
        .flora
        .iter()
        .filter(|entry| is_tree_key(&entry.species))
        .collect();
    if entries.is_empty() {
        None
    } else {
        Some(entries[(h as usize) % entries.len()])
    }
}

fn pick_tree(biome: &'static BiomeDef, h: u64) -> Option<&'static FloraEntry> {
    let entries: Vec<&'static FloraEntry> = biome
        .flora
        .iter()
        .filter(|entry| is_tree_key(&entry.species))
        .collect();
    pick_density(&entries, h)
}

fn pick_plant(biome: &'static BiomeDef, h: u64) -> Option<&'static FloraEntry> {
    let entries: Vec<&'static FloraEntry> = biome
        .flora
        .iter()
        .filter(|entry| !is_tree_key(&entry.species))
        .collect();
    pick_density(&entries, h)
}

fn pick_density<'a>(entries: &[&'a FloraEntry], h: u64) -> Option<&'a FloraEntry> {
    let total: i32 = entries
        .iter()
        .map(|entry| entry.density_q16)
        .sum::<i32>()
        .min(ONE);
    let roll = hash::unit(h ^ 0x4445_4E53_4954);
    if roll >= total {
        return None;
    }
    let mut cumulative = 0;
    for entry in entries {
        cumulative += entry.density_q16;
        if roll < cumulative {
            return Some(*entry);
        }
    }
    None
}

fn pick_ecotone(biome: &'static BiomeDef, h: u64) -> Option<&'static str> {
    if biome.ecotone.is_empty() {
        return None;
    }
    if hash::unit(h ^ 0x4543_4F54_4F4E) >= ECOTONE_DENSITY {
        return None;
    }
    let index = ((h >> 24) as usize) % biome.ecotone.len();
    Some(biome.ecotone[index].as_str())
}

fn normal_tree_height(h: u64) -> u8 {
    4 + ((h >> 48) % 9) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forest_context() -> FloraContext {
        let centre = &catalog().get("voxel:forest").expect("forest").centre;
        FloraContext {
            surface: 7_500 << 16,
            continentalness: ((centre.continentalness * 2.0 - 1.0) * ONE as f64) as i32,
            erosion: (centre.erosion * ONE as f64) as i32,
            temperature: (centre.temperature * 255.0) as u8,
            humidity: (centre.humidity * 255.0) as u8,
            slope: 0,
        }
    }

    #[test]
    fn flora_keys_have_canopies() {
        for (log, leaves) in [
            ("voxel:oak_log", "voxel:oak_leaves"),
            ("voxel:birch_log", "voxel:birch_leaves"),
            ("voxel:spruce_log", "voxel:spruce_leaves"),
            ("voxel:dark_oak_log", "voxel:dark_oak_leaves"),
        ] {
            assert_eq!(leaves_for(log), Some(leaves));
        }
        assert!(is_tree_key("voxel:oak_log"));
        assert!(!is_tree_key("voxel:fern"));
    }

    #[test]
    fn flora_tree_anchor_is_eight_metre_cell() {
        let field = FloraField::new(1);
        let (x, z) = field.tree_anchor(10, 20);
        assert!((x >> 16) >= 80 && (x >> 16) < 88);
        assert!((z >> 16) >= 160 && (z >> 16) < 168);
        assert_eq!(field.tree_anchor(10, 20), field.tree_anchor(10, 20));
    }

    #[test]
    fn flora_giant_anchor_is_inside_its_cell() {
        let field = FloraField::new(1);
        let (x, z) = field.giant_anchor(3, 5);
        assert!((x >> 16) >= 104_000 && (x >> 16) < 120_000);
        assert!((z >> 16) >= 168_000 && (z >> 16) < 184_000);
        assert_eq!(field.giant_anchor(3, 5), field.giant_anchor(3, 5));
    }

    #[test]
    fn flora_context_slope_uses_the_full_sample_span() {
        let generator = MacroGenerator::new(1);
        let x = 12_000 << 16;
        let z = 20_000 << 16;
        let step = 4 << 16;
        let dx = (generator.surface(x + step, z) - generator.surface(x - step, z)).abs();
        let dz = (generator.surface(x, z + step) - generator.surface(x, z - step)).abs();
        let expected = (dx.max(dz) / 8).clamp(0, ONE);
        let context = FloraContext::from_generator(&generator, x, z);
        assert_eq!(context.slope, expected);
    }

    #[test]
    fn flora_tree_heights_are_bounded() {
        let field = FloraField::new(1);
        let context = forest_context();
        let mut normal = 0;
        let mut giant = 0;
        for cell_x in 0..8 {
            let (x, z) = field.tree_anchor(cell_x, 0);
            if let Some(placement) = field.sample(x, z, &context)
                && placement.kind == FloraKind::Tree
            {
                assert!((4..=12).contains(&placement.height));
                normal += 1;
            }
            let (gx, gz) = field.giant_anchor(cell_x, 0);
            if let Some(placement) = field.sample(gx, gz, &context)
                && placement.giant
            {
                assert!((GIANT_MIN_HEIGHT..=GIANT_MAX_HEIGHT).contains(&placement.height));
                giant += 1;
            }
        }
        assert!(normal + giant > 0);
    }
}
