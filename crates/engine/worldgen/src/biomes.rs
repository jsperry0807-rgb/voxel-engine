//! Biome content: the RON catalogue and its validation.
//!
//! The 18 definitions live in `data/biomes.ron`; code owns only the schema and
//! the checks. Parsing is not validation: [`BiomeCatalog::parse`] enforces the
//! structural rules (18 entries, the class counts, key uniqueness, ranges), and
//! [`BiomeCatalog::validate`] takes a resolver for the block registry so every
//! material, flora and ecotone key is verified against the committed manifest.
//!
//! The weight function (`crate::weights`) reads only the centres, converted
//! once to Q16.16 by [`BiomeCatalog::centres_q16`]. Everything else is content
//! for the surface rules (M2.5-12), flora (M2.5-21) and the map.
//!
//! # Data-only
//!
//! Adding a biome is a change to the RON file, the block manifest and the slot
//! table; no Rust change is needed. The count is fixed at
//! [`BIOME_COUNT`](crate::weights::BIOME_COUNT) because the weight vector is a
//! fixed-size array in the hot path, and a definition set that does not match
//! is rejected rather than silently truncated.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::fixed::ONE;
use crate::weights::{BIOME_COUNT, BIOME_COUNT_AXES, Centres};

/// The committed biome file, embedded at build time.
pub const EMBEDDED_BIOMES: &str = include_str!("../data/biomes.ron");

/// Broad class of a biome. The counts are part of the design: 11 land, 3
/// ocean, 3 underground, 1 rare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
pub enum BiomeClass {
    /// Land surface biome.
    Land,
    /// Ocean depth band.
    Ocean,
    /// Underground biome; only reachable with a depth term.
    Underground,
    /// Rare geothermal biome.
    Rare,
}

impl BiomeClass {
    /// How many definitions of this class the design requires.
    pub const fn required_count(self) -> usize {
        match self {
            Self::Land => 11,
            Self::Ocean => 3,
            Self::Underground => 3,
            Self::Rare => 1,
        }
    }
}

/// The softmax centre in the five-axis space, normalised to `[0, 1]`.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct BiomeCentre {
    pub continentalness: f64,
    pub temperature: f64,
    pub humidity: f64,
    pub erosion: f64,
    pub depth: f64,
}

impl BiomeCentre {
    /// The centre in Q16.16, `[continentalness, temperature, humidity, erosion,
    /// depth]`.
    pub fn q16(self) -> [i32; BIOME_COUNT_AXES] {
        [
            unit(self.continentalness),
            unit(self.temperature),
            unit(self.humidity),
            unit(self.erosion),
            unit(self.depth),
        ]
    }
}

/// The climate envelope of a biome, in normalised units.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ClimateRange {
    pub temperature: (f64, f64),
    pub humidity: (f64, f64),
    pub erosion: (f64, f64),
}

/// The four surface materials of a biome.
#[derive(Clone, Debug, Deserialize)]
pub struct SurfacePalette {
    pub top: String,
    pub filler: String,
    pub stone: String,
    pub beach: String,
}

impl SurfacePalette {
    /// Every material key in this palette.
    pub fn keys(&self) -> [&str; 4] {
        [&self.top, &self.filler, &self.stone, &self.beach]
    }
}

/// One flora species and its density.
#[derive(Clone, Debug, Deserialize)]
pub struct FloraEntry {
    pub species: String,
    /// Probability per eligible surface block, `[0, 1]`.
    pub density: f64,
}

/// One biome definition, as stored in `data/biomes.ron`.
#[derive(Clone, Debug, Deserialize)]
pub struct BiomeDef {
    pub key: String,
    pub class: BiomeClass,
    pub centre: BiomeCentre,
    pub climate: ClimateRange,
    pub surface: SurfacePalette,
    pub soil_depth: (u8, u8),
    pub flora: Vec<FloraEntry>,
    pub ecotone: Vec<String>,
    pub structures: Vec<String>,
    pub map_color: (u8, u8, u8),
    pub fog: (f64, f64, f64),
}

impl BiomeDef {
    /// The weight centre in Q16.16.
    pub fn centre_q16(&self) -> [i32; BIOME_COUNT_AXES] {
        self.centre.q16()
    }
}

/// The parsed biome catalogue.
#[derive(Clone, Debug, Deserialize)]
pub struct BiomeCatalog {
    pub biomes: Vec<BiomeDef>,
}

/// Why a biome file was rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum BiomeError {
    /// The RON could not be parsed.
    Parse(String),
    /// The file does not define exactly [`BIOME_COUNT`] biomes.
    WrongCount { found: usize },
    /// A key appears twice.
    DuplicateKey { key: String },
    /// A key is not namespaced (`namespace:name`).
    BadKey { key: String },
    /// A class has the wrong number of definitions.
    WrongClassCount {
        class: BiomeClass,
        found: usize,
        expected: usize,
    },
    /// A climate or fog range is inverted or outside `[0, 1]`.
    BadRange {
        biome: String,
        field: &'static str,
        low: f64,
        high: f64,
    },
    /// A unit value is outside `[0, 1]`.
    BadUnit {
        biome: String,
        field: &'static str,
        value: f64,
    },
    /// Soil depth is not a sensible `1..=8` range.
    BadSoilDepth { biome: String, low: u8, high: u8 },
    /// A flora density is outside `[0, 1]`.
    BadDensity {
        biome: String,
        species: String,
        density: f64,
    },
    /// A material, flora or ecotone key is not in the block manifest.
    UnknownKey { biome: String, key: String },
}

impl std::fmt::Display for BiomeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "biomes.ron: {message}"),
            Self::WrongCount { found } => {
                write!(f, "expected {BIOME_COUNT} biomes, found {found}")
            }
            Self::DuplicateKey { key } => write!(f, "duplicate biome key {key:?}"),
            Self::BadKey { key } => write!(f, "bad biome key {key:?}"),
            Self::WrongClassCount {
                class,
                found,
                expected,
            } => write!(f, "{class:?} has {found} biomes, expected {expected}"),
            Self::BadRange {
                biome,
                field,
                low,
                high,
            } => write!(f, "{biome}: {field} range ({low}, {high}) is invalid"),
            Self::BadUnit {
                biome,
                field,
                value,
            } => write!(f, "{biome}: {field} {value} is outside [0, 1]"),
            Self::BadSoilDepth { biome, low, high } => {
                write!(f, "{biome}: soil depth ({low}, {high}) is invalid")
            }
            Self::BadDensity {
                biome,
                species,
                density,
            } => write!(f, "{biome}: {species} density {density} is outside [0, 1]"),
            Self::UnknownKey { biome, key } => {
                write!(f, "{biome}: {key:?} is not in the block manifest")
            }
        }
    }
}

impl std::error::Error for BiomeError {}

impl BiomeCatalog {
    /// Parse and structurally validate a biome file.
    pub fn parse(text: &str) -> Result<Self, BiomeError> {
        let catalog: Self =
            ron::from_str(text).map_err(|error| BiomeError::Parse(error.to_string()))?;
        catalog.check_structure()?;
        Ok(catalog)
    }

    /// Check every material, flora and ecotone key against a resolver.
    ///
    /// `resolve` answers whether a namespaced key exists in the block registry.
    pub fn validate(&self, resolve: impl Fn(&str) -> bool) -> Result<(), BiomeError> {
        for biome in &self.biomes {
            for key in biome.surface.keys() {
                if !resolve(key) {
                    return Err(BiomeError::UnknownKey {
                        biome: biome.key.clone(),
                        key: key.to_owned(),
                    });
                }
            }
            for flora in &biome.flora {
                if !resolve(&flora.species) {
                    return Err(BiomeError::UnknownKey {
                        biome: biome.key.clone(),
                        key: flora.species.clone(),
                    });
                }
            }
            for key in &biome.ecotone {
                if !resolve(key) {
                    return Err(BiomeError::UnknownKey {
                        biome: biome.key.clone(),
                        key: key.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Structural rules: count, classes, keys, ranges and unit values.
    pub fn check_structure(&self) -> Result<(), BiomeError> {
        if self.biomes.len() != BIOME_COUNT {
            return Err(BiomeError::WrongCount {
                found: self.biomes.len(),
            });
        }

        let mut seen: Vec<&str> = Vec::with_capacity(self.biomes.len());
        for biome in &self.biomes {
            if !biome.key.contains(':') || biome.key.starts_with(':') || biome.key.ends_with(':') {
                return Err(BiomeError::BadKey {
                    key: biome.key.clone(),
                });
            }
            if seen.contains(&biome.key.as_str()) {
                return Err(BiomeError::DuplicateKey {
                    key: biome.key.clone(),
                });
            }
            seen.push(&biome.key);

            check_range(&biome.key, "temperature", biome.climate.temperature)?;
            check_range(&biome.key, "humidity", biome.climate.humidity)?;
            check_range(&biome.key, "erosion", biome.climate.erosion)?;
            for (field, value) in [
                ("centre.continentalness", biome.centre.continentalness),
                ("centre.temperature", biome.centre.temperature),
                ("centre.humidity", biome.centre.humidity),
                ("centre.erosion", biome.centre.erosion),
                ("centre.depth", biome.centre.depth),
                ("fog.red", biome.fog.0),
                ("fog.green", biome.fog.1),
                ("fog.blue", biome.fog.2),
            ] {
                check_unit(&biome.key, field, value)?;
            }
            let (low, high) = biome.soil_depth;
            if low == 0 || low > high || high > 8 {
                return Err(BiomeError::BadSoilDepth {
                    biome: biome.key.clone(),
                    low,
                    high,
                });
            }
            for flora in &biome.flora {
                if !(0.0..=1.0).contains(&flora.density) {
                    return Err(BiomeError::BadDensity {
                        biome: biome.key.clone(),
                        species: flora.species.clone(),
                        density: flora.density,
                    });
                }
            }
        }

        for class in [
            BiomeClass::Land,
            BiomeClass::Ocean,
            BiomeClass::Underground,
            BiomeClass::Rare,
        ] {
            let found = self
                .biomes
                .iter()
                .filter(|biome| biome.class == class)
                .count();
            let expected = class.required_count();
            if found != expected {
                return Err(BiomeError::WrongClassCount {
                    class,
                    found,
                    expected,
                });
            }
        }
        Ok(())
    }

    /// Look up a biome by key.
    pub fn get(&self, key: &str) -> Option<&BiomeDef> {
        self.biomes.iter().find(|biome| biome.key == key)
    }

    /// Number of definitions.
    pub fn len(&self) -> usize {
        self.biomes.len()
    }

    /// True when the catalogue is empty (never for a parsed file).
    pub fn is_empty(&self) -> bool {
        self.biomes.is_empty()
    }

    /// Every centre in Q16.16, in file order.
    pub fn centres_q16(&self) -> Centres {
        let mut centres = [[0i32; BIOME_COUNT_AXES]; BIOME_COUNT];
        for (index, biome) in self.biomes.iter().enumerate() {
            centres[index] = biome.centre_q16();
        }
        centres
    }
}

/// The embedded catalogue, parsed once.
///
/// # Panics
///
/// If the committed file is invalid. The test suite parses it on every run, so
/// this only fires on a broken build.
pub fn catalog() -> &'static BiomeCatalog {
    static CATALOG: OnceLock<BiomeCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        BiomeCatalog::parse(EMBEDDED_BIOMES).expect("committed data/biomes.ron must parse")
    })
}

/// The embedded centres, converted once for the weight function.
pub fn embedded_centres() -> &'static Centres {
    static CENTRES: OnceLock<Centres> = OnceLock::new();
    CENTRES.get_or_init(|| catalog().centres_q16())
}

fn unit(value: f64) -> i32 {
    ((value.clamp(0.0, 1.0)) * ONE as f64).round() as i32
}

fn check_range(
    biome: &str,
    field: &'static str,
    (low, high): (f64, f64),
) -> Result<(), BiomeError> {
    if !(0.0..=1.0).contains(&low) || !(0.0..=1.0).contains(&high) || low > high {
        return Err(BiomeError::BadRange {
            biome: biome.to_owned(),
            field,
            low,
            high,
        });
    }
    Ok(())
}

fn check_unit(biome: &str, field: &'static str, value: f64) -> Result<(), BiomeError> {
    if !(0.0..=1.0).contains(&value) {
        return Err(BiomeError::BadUnit {
            biome: biome.to_owned(),
            field,
            value,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biome_ron_embedded_file_parses() {
        let catalog = catalog();
        assert_eq!(catalog.len(), BIOME_COUNT);
        assert_eq!(
            catalog
                .biomes
                .iter()
                .filter(|biome| biome.class == BiomeClass::Land)
                .count(),
            11
        );
    }

    #[test]
    fn biome_ron_centres_convert_to_q16() {
        let catalog = catalog();
        let centres = catalog.centres_q16();
        assert_eq!(centres.len(), BIOME_COUNT);
        // Beach: continentalness 0.52.
        assert_eq!(centres[0][0], (0.52 * ONE as f64).round() as i32);
        for centre in centres {
            for axis in centre {
                assert!((0..=ONE).contains(&axis));
            }
        }
    }
}
