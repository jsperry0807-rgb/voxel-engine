//! Cave depth relative to the local surface and underground biome assignment.
//!
//! The cave field ([`crate::caves`]) decides whether a voxel is carved; this
//! module answers two follow-up questions:
//!
//! 1. **How deep is it?** [`CaveDepth`] subtracts the *local* surface, not a
//!    world-wide datum, and normalises the result to `[0, ONE]` over the
//!    2,000 m cave budget. A cave under a mountain and a cave under a valley
//!    therefore have the same depth when they are the same distance below
//!    their own surface.
//! 2. **Which underground biome is it?** [`assign`] runs the M2.5-10 softmax
//!    with the depth axis filled in and picks the strongest underground or
//!    rare biome. Water raises the effective humidity, so a wet void at mid
//!    depth is an aquifer while a dry one at the same depth is cavern or fungal.
//!
//! # Why the weights, not thresholds
//!
//! The three underground centres differ mainly in depth and humidity:
//!
//! | Biome | Depth centre | Humidity centre |
//! |---|---:|---:|
//! | aquifer | 0.60 | 0.60 |
//! | cavern | 0.65 | 0.40 |
//! | fungal deep | 0.75 | 0.50 |
//! | geothermal (rare) | 0.90 | 0.30 |
//!
//! Reusing the softmax keeps the assignment consistent with the surface
//! biomes and with the RON content: tuning a centre changes the underground
//! distribution without a code edit. The assignment filters to
//! [`BiomeClass::Underground`] and [`BiomeClass::Rare`] so a trace surface
//! weight can never label a cave as a forest or an ocean.
//!
//! # Content
//!
//! The palettes come from `data/biomes.ron`; the keys are the M2.5-31
//! underground materials (`clay`, `deep_stone`, `fungal_cap`, `fungal_stem`,
//! `geothermal_rock`, `obsidian`, plus the shared stone/gravel/sand). The
//! integration test resolves every one through the committed block manifest.

use crate::biomes::{BiomeClass, BiomeDef, catalog};
use crate::caves::MAX_CAVE_DEPTH_M;
use crate::fixed::{Coord, ONE};
use crate::weights::{Axes, BiomeWeights};

/// Continentalness used for an underground sample.
///
/// Every underground centre sits at the middle of the axis; the surface
/// continentalness no longer distinguishes caves once the depth term is in.
pub const UNDERGROUND_CONTINENTALNESS: i32 = ONE / 2;

/// Effective humidity of a water-filled or water-adjacent cave, Q16.16.
pub const WATER_HUMIDITY: i32 = ONE * 80 / 100;

/// A cave voxel's depth below the local surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaveDepth {
    surface: Coord,
    y: Coord,
}

impl CaveDepth {
    /// Depth of `y` below `surface`, both Q16.16 metres.
    pub const fn new(surface: Coord, y: Coord) -> Self {
        Self { surface, y }
    }

    /// The local surface height this depth is measured from.
    pub const fn surface(self) -> Coord {
        self.surface
    }

    /// The voxel height.
    pub const fn y(self) -> Coord {
        self.y
    }

    /// Signed depth in whole metres; negative above the surface.
    pub fn metres(self) -> i32 {
        ((self.surface - self.y) >> 16) as i32
    }

    /// True when the voxel is at or below the surface and inside the 2,000 m
    /// cave budget.
    pub fn within_budget(self) -> bool {
        (0..=MAX_CAVE_DEPTH_M).contains(&self.metres())
    }

    /// Depth normalised to the cave budget, Q16.16 in `[0, ONE]`.
    ///
    /// Clamps above the surface and beyond 2,000 m, so callers can use it as
    /// the weight axis without a second range check.
    pub fn normalized(self) -> i32 {
        let depth = self.metres().clamp(0, MAX_CAVE_DEPTH_M) as i64;
        ((depth << 16) / MAX_CAVE_DEPTH_M as i64) as i32
    }
}

/// Climate inputs for an underground sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndergroundClimate {
    /// Surface temperature channel, 0 coldest / 255 hottest.
    pub temperature: u8,
    /// Surface humidity channel, 0 driest / 255 wettest.
    pub humidity: u8,
    /// Erosion channel, Q16.16 in `[0, ONE]`.
    pub erosion: i32,
    /// True for a water-filled cave or a column near the water table.
    pub water: bool,
}

impl UndergroundClimate {
    /// A climate sample with every input explicit.
    pub const fn new(temperature: u8, humidity: u8, erosion: i32, water: bool) -> Self {
        Self {
            temperature,
            humidity,
            erosion,
            water,
        }
    }
}

/// The five-axis weight input for a cave sample.
///
/// `temperature` and `humidity` are converted from the `u8` climate channels
/// the same way [`Axes::surface`] does; water raises humidity to at least
/// [`WATER_HUMIDITY`], which is what pulls the assignment toward the aquifer.
pub fn axes(depth: CaveDepth, climate: UndergroundClimate) -> Axes {
    let humidity = (climate.humidity as i32) * ONE / 255;
    Axes {
        continentalness: UNDERGROUND_CONTINENTALNESS,
        temperature: (climate.temperature as i32) * ONE / 255,
        humidity: if climate.water {
            humidity.max(WATER_HUMIDITY)
        } else {
            humidity
        },
        erosion: climate.erosion.clamp(0, ONE),
        depth: depth.normalized(),
    }
}

/// The strongest underground or rare biome for a weight input.
///
/// Returns the catalogue index, or `None` only if the catalogue has no
/// underground/rare entries. Ties keep the first such biome in catalogue
/// order, which is deterministic.
pub fn assign(axes: &Axes) -> Option<usize> {
    let weights = BiomeWeights::at(axes);
    let mut best: Option<usize> = None;
    for (index, biome) in catalog().biomes.iter().enumerate() {
        if !matches!(biome.class, BiomeClass::Underground | BiomeClass::Rare) {
            continue;
        }
        if best.is_none_or(|current| weights.weight(index) > weights.weight(current)) {
            best = Some(index);
        }
    }
    best
}

/// The assigned underground biome for a depth and climate.
pub fn assign_biome(depth: CaveDepth, climate: UndergroundClimate) -> Option<&'static BiomeDef> {
    assign(&axes(depth, climate)).map(|index| &catalog().biomes[index])
}

/// The assigned underground biome's key, for logging and tests.
pub fn assign_key(depth: CaveDepth, climate: UndergroundClimate) -> Option<&'static str> {
    assign_biome(depth, climate).map(|biome| biome.key.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metres(value: i32) -> Coord {
        (value as i64) << 16
    }

    fn climate(temperature: u8, humidity: u8, erosion: i32, water: bool) -> UndergroundClimate {
        UndergroundClimate::new(temperature, humidity, erosion, water)
    }

    #[test]
    fn cave_depth_is_relative_to_the_local_surface() {
        let mountain = CaveDepth::new(metres(9_000), metres(8_500));
        let valley = CaveDepth::new(metres(2_000), metres(1_500));
        assert_eq!(mountain.metres(), 500);
        assert_eq!(valley.metres(), 500);
        assert_eq!(mountain.normalized(), valley.normalized());
        assert_eq!(mountain.normalized(), (500 * ONE) / MAX_CAVE_DEPTH_M);
        assert!(mountain.within_budget());
        assert!(valley.within_budget());

        // The same absolute y is a different depth under a different surface.
        assert_ne!(
            CaveDepth::new(metres(1_000), metres(500)).normalized(),
            CaveDepth::new(metres(2_000), metres(500)).normalized()
        );
    }

    #[test]
    fn cave_depth_is_clamped_to_the_budget() {
        let at_surface = CaveDepth::new(metres(1_000), metres(1_000));
        assert_eq!(at_surface.metres(), 0);
        assert_eq!(at_surface.normalized(), 0);

        let full = CaveDepth::new(metres(1_000), metres(1_000 - MAX_CAVE_DEPTH_M));
        assert_eq!(full.metres(), MAX_CAVE_DEPTH_M);
        assert_eq!(full.normalized(), ONE);
        assert!(full.within_budget());

        let beyond = CaveDepth::new(metres(1_000), metres(1_000 - MAX_CAVE_DEPTH_M - 1));
        assert!(!beyond.within_budget());
        assert_eq!(beyond.normalized(), ONE);

        let above = CaveDepth::new(metres(1_000), metres(1_001));
        assert!(!above.within_budget());
        assert_eq!(above.normalized(), 0);
    }

    #[test]
    fn cave_depth_water_raises_humidity() {
        let depth = CaveDepth::new(metres(1_000), metres(400));
        let dry = axes(depth, climate(128, 20, ONE / 2, false));
        let wet = axes(depth, climate(128, 20, ONE / 2, true));
        assert!(wet.humidity >= WATER_HUMIDITY);
        assert!(wet.humidity > dry.humidity);
        assert_eq!(wet.depth, dry.depth);
    }

    #[test]
    fn cave_depth_assignment_is_always_underground() {
        for depth_m in [0, 200, 600, 1_200, 1_800, 2_000] {
            for water in [false, true] {
                let depth = CaveDepth::new(metres(5_000), metres(5_000 - depth_m));
                for temperature in [0u8, 128, 255] {
                    for humidity in [0u8, 128, 255] {
                        let biome =
                            assign_biome(depth, climate(temperature, humidity, ONE / 2, water))
                                .expect("an underground biome");
                        assert!(
                            matches!(biome.class, BiomeClass::Underground | BiomeClass::Rare),
                            "assigned {} at {depth_m} m",
                            biome.key
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cave_depth_assignment_follows_depth_and_water() {
        // Aquifer: mid depth and wet.
        let aquifer = assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_200)),
            climate(128, 102, ONE * 4 / 10, true),
        )
        .expect("an aquifer");
        assert_eq!(aquifer, "voxel:aquifer");

        // Fungal deep: deeper and dry, with the humid centre.
        let fungal = assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_500)),
            climate(128, 128, ONE * 3 / 10, false),
        )
        .expect("a fungal biome");
        assert_eq!(fungal, "voxel:fungal_deep");

        // Cavern: mid-depth, dry, eroded.
        let cavern = assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_300)),
            climate(128, 102, ONE * 6 / 10, false),
        )
        .expect("a cavern");
        assert_eq!(cavern, "voxel:cavern");

        // Geothermal: very deep and hot.
        let geothermal = assign_key(
            CaveDepth::new(metres(2_000), metres(2_000 - 1_800)),
            climate(230, 77, ONE * 9 / 10, false),
        )
        .expect("a geothermal biome");
        assert_eq!(geothermal, "voxel:geothermal");
    }
}
