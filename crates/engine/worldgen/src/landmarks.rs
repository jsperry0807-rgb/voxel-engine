//! Landmark anchors on a jittered 20–50 km grid.
//!
//! The world is divided into **32 × 32 cells of 32 km**, and every cell gets
//! exactly one landmark. The anchor is jittered by up to 4 km around the cell
//! centre, so neighbouring landmarks sit 24–40 km apart — inside the design's
//! 20–50 km band.
//!
//! # Types
//!
//! | Type | Fits where |
//! |---|---|
//! | massif | high, uneroded continental interior |
//! | canyon/falls | high erosion and a steep gradient |
//! | volcano | hot, high, uneroded ground |
//! | giant grove | humid, gentle, low-to-mid land |
//! | monolith/arch | dry, flat lowland, or an ocean stack |
//!
//! The classification is a scored choice over the macro fields at the anchor
//! (continentalness, erosion, temperature, humidity, height and a local
//! 1 km slope). It is deterministic; the cell hash only chooses the jitter.
//! Ocean cells are always monolith/arch (a sea stack or sea arch), so the grid
//! is never empty even though most ocean cells have no other landmark type.
//!
//! # Why a fixed cell size
//!
//! The design says "jittered 20–50 km grid". A fixed 32 km cell with a ±4 km
//! anchor jitter gives a guaranteed 24–40 km spacing and a simple, globally
//! indexable grid; a variable cell size would need a second data structure to
//! answer "which landmark is near this chunk?" The anchor and type are pure
//! functions of the seed and cell, so any worker can rebuild the whole grid.

use crate::biomes::{BiomeClass, catalog};
use crate::fixed::{Coord, ONE};
use crate::generator::MacroGenerator;
use crate::hash;
use crate::macro_grid::WORLD_EXTENT_M;
use crate::splines::SEA_LEVEL;
use crate::weights::{Axes, BiomeWeights};

/// Landmark cell edge, in metres.
pub const LANDMARK_CELL_M: i64 = 32_000;

/// Anchor jitter around the cell centre, in metres.
pub const LANDMARK_JITTER_M: i64 = 4_000;

/// Cells per world axis: 1,024 km / 32 km.
pub const LANDMARK_CELLS: i32 = (WORLD_EXTENT_M as i64 / LANDMARK_CELL_M) as i32;

/// Landmarks in the world: one per cell.
pub const LANDMARK_COUNT: usize = (LANDMARK_CELLS * LANDMARK_CELLS) as usize;

/// Metres between the anchor samples used for the local slope.
const SLOPE_BASELINE_M: i64 = 512;

const LANDMARK_SALT: u64 = 0x4C41_4E44_0001;

/// The five landmark types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LandmarkKind {
    /// A high mountain mass.
    Massif,
    /// A steep canyon or waterfall setting.
    CanyonFalls,
    /// A hot volcanic peak.
    Volcano,
    /// A giant-tree grove.
    GiantGrove,
    /// A dry monolith, arch, or ocean stack.
    MonolithArch,
}

impl LandmarkKind {
    /// Every type, in classification order.
    pub const ALL: [Self; 5] = [
        Self::Massif,
        Self::CanyonFalls,
        Self::Volcano,
        Self::GiantGrove,
        Self::MonolithArch,
    ];

    /// One-letter map label.
    pub const fn letter(self) -> char {
        match self {
            Self::Massif => 'M',
            Self::CanyonFalls => 'C',
            Self::Volcano => 'V',
            Self::GiantGrove => 'G',
            Self::MonolithArch => 'A',
        }
    }

    /// Lower-case name for the CLI.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Massif => "massif",
            Self::CanyonFalls => "canyon",
            Self::Volcano => "volcano",
            Self::GiantGrove => "grove",
            Self::MonolithArch => "monolith",
        }
    }
}

/// One landmark anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Landmark {
    /// Cell column and row.
    pub cell: [i32; 2],
    /// Anchor world x, Q16.16 metres.
    pub x: Coord,
    /// Anchor world z, Q16.16 metres.
    pub z: Coord,
    /// Surface height at the anchor, Q16.16 metres.
    pub surface: i32,
    /// Chosen type.
    pub kind: LandmarkKind,
}

/// The macro fields sampled at an anchor, kept for classification and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LandmarkSample {
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

impl LandmarkSample {
    /// True when the anchor is at or below sea level.
    pub fn ocean(self) -> bool {
        self.surface <= SEA_LEVEL
    }

    /// Dominant biome key at this sample, for diagnostics.
    pub fn biome_key(self) -> &'static str {
        let axes = Axes::surface(
            self.continentalness,
            self.temperature,
            self.humidity,
            self.erosion,
        );
        let weights = BiomeWeights::at(&axes);
        catalog().biomes[weights.dominant()].key.as_str()
    }

    /// Dominant biome class at this sample.
    pub fn biome_class(self) -> BiomeClass {
        let axes = Axes::surface(
            self.continentalness,
            self.temperature,
            self.humidity,
            self.erosion,
        );
        let weights = BiomeWeights::at(&axes);
        catalog().biomes[weights.dominant()].class
    }
}

/// The seeded landmark grid.
#[derive(Clone, Copy, Debug)]
pub struct LandmarkGrid {
    seed: u64,
    generator: MacroGenerator,
}

impl LandmarkGrid {
    /// Create the grid for a world seed.
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

    /// The macro generator used for anchor sampling.
    pub const fn generator(&self) -> &MacroGenerator {
        &self.generator
    }

    /// The anchor of a cell, in Q16.16 metres.
    ///
    /// # Panics
    ///
    /// If the cell is outside `0..LANDMARK_CELLS`.
    pub fn anchor(&self, cell_x: i32, cell_z: i32) -> (Coord, Coord) {
        assert!(
            (0..LANDMARK_CELLS).contains(&cell_x) && (0..LANDMARK_CELLS).contains(&cell_z),
            "landmark cell {cell_x}, {cell_z} is outside the world"
        );
        let h = hash::hash2(self.seed ^ LANDMARK_SALT, cell_x as i64, cell_z as i64);
        let span = (2 * LANDMARK_JITTER_M + 1) as u64;
        let jitter_x = ((h % span) as i64 - LANDMARK_JITTER_M) << 16;
        let jitter_z = (((h >> 16) % span) as i64 - LANDMARK_JITTER_M) << 16;
        let centre_x = (cell_x as i64 * LANDMARK_CELL_M + LANDMARK_CELL_M / 2) << 16;
        let centre_z = (cell_z as i64 * LANDMARK_CELL_M + LANDMARK_CELL_M / 2) << 16;
        (centre_x + jitter_x, centre_z + jitter_z)
    }

    /// Sample the macro fields at an anchor.
    pub fn sample(&self, cell_x: i32, cell_z: i32) -> LandmarkSample {
        let (x, z) = self.anchor(cell_x, cell_z);
        let surface = self.generator.surface(x, z);
        let continentalness = self.generator.continentalness(x, z);
        let erosion = self.generator.erosion(x, z);
        let temperature = self.generator.temperature(x, z, surface);
        let humidity = self.generator.humidity(x, z, continentalness);

        let step = SLOPE_BASELINE_M << 16;
        let east = self.generator.surface(x + step, z);
        let west = self.generator.surface(x - step, z);
        let south = self.generator.surface(x, z + step);
        let north = self.generator.surface(x, z - step);
        let dx = (east - west).abs();
        let dz = (south - north).abs();
        // Q16.16 metres / whole metres = Q16.16 rise over run.
        let slope = (dx.max(dz) / SLOPE_BASELINE_M as i32).clamp(0, ONE);

        LandmarkSample {
            surface,
            continentalness,
            erosion,
            temperature,
            humidity,
            slope,
        }
    }

    /// The landmark in a cell.
    pub fn landmark(&self, cell_x: i32, cell_z: i32) -> Landmark {
        let (x, z) = self.anchor(cell_x, cell_z);
        let sample = self.sample(cell_x, cell_z);
        Landmark {
            cell: [cell_x, cell_z],
            x,
            z,
            surface: sample.surface,
            kind: classify(&sample),
        }
    }

    /// Every landmark in row-major cell order.
    pub fn iter(&self) -> impl Iterator<Item = Landmark> + '_ {
        let cells = LANDMARK_CELLS;
        (0..cells).flat_map(move |z| (0..cells).map(move |x| self.landmark(x, z)))
    }
}

/// Steepness that reads as a canyon or falls, Q16.16 rise over run.
pub const CANYON_SLOPE: i32 = ONE * 12 / 100;

/// Erosion that reads as a canyon or falls, Q16.16.
pub const CANYON_EROSION: i32 = ONE * 65 / 100;

/// Temperature at or above which a mountain reads as volcanic.
pub const VOLCANO_TEMPERATURE: u8 = 100;

/// Classify an anchor by its biome and local terrain.
///
/// The rules are intentionally legible: the ocean gets a stack/arch; steep or
/// heavily eroded land gets the canyon/falls; mountains and hills are massifs
/// unless they are warm, in which case they are volcanic; forest, taiga, swamp
/// and plains get the giant grove; and the remaining dry or cold lowlands get
/// the monolith/arch.
pub fn classify(sample: &LandmarkSample) -> LandmarkKind {
    if sample.ocean() {
        return LandmarkKind::MonolithArch;
    }
    if sample.slope >= CANYON_SLOPE || sample.erosion >= CANYON_EROSION {
        return LandmarkKind::CanyonFalls;
    }
    match sample.biome_key() {
        "voxel:mountains" | "voxel:alpine" | "voxel:hills" => {
            if sample.temperature >= VOLCANO_TEMPERATURE {
                LandmarkKind::Volcano
            } else {
                LandmarkKind::Massif
            }
        }
        "voxel:forest" | "voxel:taiga" | "voxel:swamp" | "voxel:plains" => LandmarkKind::GiantGrove,
        _ => LandmarkKind::MonolithArch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmarks_grid_is_exactly_one_per_cell() {
        let grid = LandmarkGrid::new(1);
        assert_eq!(LANDMARK_CELLS, 32);
        assert_eq!(LANDMARK_COUNT, 1_024);
        let landmarks: Vec<_> = grid.iter().collect();
        assert_eq!(landmarks.len(), LANDMARK_COUNT);
        let mut cells: Vec<[i32; 2]> = landmarks.iter().map(|landmark| landmark.cell).collect();
        cells.sort_unstable();
        cells.dedup();
        assert_eq!(cells.len(), LANDMARK_COUNT);
    }

    #[test]
    fn landmarks_anchor_inside_its_cell() {
        let grid = LandmarkGrid::new(7);
        for cell_z in 0..LANDMARK_CELLS {
            for cell_x in 0..LANDMARK_CELLS {
                let (x, z) = grid.anchor(cell_x, cell_z);
                let left = (cell_x as i64 * LANDMARK_CELL_M) << 16;
                let right = left + (LANDMARK_CELL_M << 16);
                let top = (cell_z as i64 * LANDMARK_CELL_M) << 16;
                let bottom = top + (LANDMARK_CELL_M << 16);
                assert!(x >= left && x < right, "x {cell_x},{cell_z}");
                assert!(z >= top && z < bottom, "z {cell_x},{cell_z}");
            }
        }
    }

    #[test]
    fn landmarks_spacing_is_twenty_to_fifty_km() {
        let grid = LandmarkGrid::new(3);
        for cell_z in 0..LANDMARK_CELLS {
            for cell_x in 0..LANDMARK_CELLS {
                let (x, _) = grid.anchor(cell_x, cell_z);
                if cell_x + 1 < LANDMARK_CELLS {
                    let (next, _) = grid.anchor(cell_x + 1, cell_z);
                    let gap = (next - x) >> 16;
                    assert!((20_000..=50_000).contains(&gap), "x gap {gap}");
                }
                let (_, z) = grid.anchor(cell_x, cell_z);
                if cell_z + 1 < LANDMARK_CELLS {
                    let (_, next) = grid.anchor(cell_x, cell_z + 1);
                    let gap = (next - z) >> 16;
                    assert!((20_000..=50_000).contains(&gap), "z gap {gap}");
                }
            }
        }
    }

    #[test]
    fn landmarks_are_deterministic_and_seeded() {
        let first = LandmarkGrid::new(11);
        let second = LandmarkGrid::new(11);
        let other = LandmarkGrid::new(12);
        for cell_z in (0..LANDMARK_CELLS).step_by(5) {
            for cell_x in (0..LANDMARK_CELLS).step_by(5) {
                assert_eq!(
                    first.landmark(cell_x, cell_z),
                    second.landmark(cell_x, cell_z)
                );
            }
        }
        assert_ne!(first.landmark(8, 8), other.landmark(8, 8));
    }

    #[test]
    fn landmarks_cover_all_five_types() {
        for seed in 1..=5 {
            let grid = LandmarkGrid::new(seed);
            let mut seen = [false; 5];
            for landmark in grid.iter() {
                seen[landmark.kind as usize] = true;
            }
            assert!(
                seen.iter().all(|seen| *seen),
                "seed {seed} is missing a landmark type"
            );
        }
    }

    #[test]
    fn landmarks_types_fit_terrain() {
        let grid = LandmarkGrid::new(1);
        for landmark in grid.iter() {
            let sample = grid.sample(landmark.cell[0], landmark.cell[1]);
            let biome = sample.biome_key();
            if sample.ocean() {
                assert_eq!(landmark.kind, LandmarkKind::MonolithArch);
                continue;
            }
            match landmark.kind {
                LandmarkKind::Volcano => {
                    assert!(sample.temperature >= VOLCANO_TEMPERATURE);
                }
                LandmarkKind::Massif => {
                    assert!(matches!(
                        biome,
                        "voxel:mountains" | "voxel:alpine" | "voxel:hills"
                    ));
                    assert!(sample.temperature < VOLCANO_TEMPERATURE);
                }
                LandmarkKind::GiantGrove => {
                    assert!(matches!(
                        biome,
                        "voxel:forest" | "voxel:taiga" | "voxel:swamp" | "voxel:plains"
                    ));
                }
                LandmarkKind::CanyonFalls => {
                    assert!(sample.slope >= CANYON_SLOPE || sample.erosion >= CANYON_EROSION);
                }
                LandmarkKind::MonolithArch => {}
            }
        }
    }
}
