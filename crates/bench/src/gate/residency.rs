//! Loaded chunk count for the configured view/band (M1-36).
//!
//! The memory gate multiplies a measured per-chunk distribution by the number
//! of chunks the streaming policy holds. This module supplies that count, and
//! does it at equal *loaded volume* so the 64³ baseline and the pre-registered
//! 128³ target are compared for the same amount of world, not the same number
//! of chunks.
//!
//! # Model
//!
//! Pre-registration §2 defines
//!
//! ```text
//! N(edge, R, V) = ceil(pi * (R / edge)^2) * ceil(V / edge)
//! ```
//!
//! where `R` is the near voxel radius and `V` the full vertical shell. The
//! table frozen beside that formula is a prediction, not a result. This module
//! evaluates the formula as written and reports the delta against the table;
//! the table's Minimum/64³ row used nearest-integer chunk-area rounding and is
//! eight chunks lower than the formula. The report surfaces that difference
//! instead of silently changing the frozen model.
//!
//! The baseline count is evaluated at the M1 64³ edge. The 128³ count is the
//! equal-volume equivalent:
//!
//! ```text
//! N64 * 64³ = N128 * 128³  =>  N64 = 8 * N128
//! ```
//!
//! `N128` is therefore derived from `N64`, not independently rounded on the
//! 128³ grid; the report asserts both the chunk-count and the volume identity.
//!
//! # What is not measured
//!
//! `N` depends on the view/band geometry, not on block content. The frozen
//! fixture has 72 live chunks and cannot fill a 25,736-chunk view, so this
//! report is the residency target for the gate; per-chunk bytes are measured
//! separately over the fixture by [`super::memory`] (M1-35).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use voxel_core::constants::{
    BASELINE_CHUNK_BITS, BASELINE_CHUNK_DIM, BLOCK_BITS, TARGET_CHUNK_BITS, TARGET_CHUNK_DIM,
};

/// Schema version of the residency report.
pub const SCHEMA_VERSION: u32 = 1;

/// The profiles frozen in `docs/reference-machine.md` §3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// 64 m near radius, ±16 m shell. Gated.
    Minimum,
    /// 128 m near radius, ±16 m shell. Gated; the shipping target.
    #[default]
    Default,
    /// 256 m near radius, ±32 m shell. Not gated on the reference machine.
    High,
}

impl Profile {
    /// Every profile the CLI accepts, in the frozen table order.
    pub const ALL: [Self; 3] = [Self::Minimum, Self::Default, Self::High];

    /// Lower-case name used on the command line and in JSON.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minimum => "minimum",
            Self::Default => "default",
            Self::High => "high",
        }
    }

    /// Near voxel radius in metres (`docs/reference-machine.md` §3).
    pub const fn near_radius_m(self) -> u32 {
        match self {
            Self::Minimum => 64,
            Self::Default => 128,
            Self::High => 256,
        }
    }

    /// Full vertical shell in metres; the shell is `±V / 2`.
    pub const fn vertical_shell_m(self) -> u32 {
        match self {
            Self::Minimum | Self::Default => 32,
            Self::High => 64,
        }
    }

    /// True for the profiles the M1 gate measures.
    pub const fn gated(self) -> bool {
        matches!(self, Self::Minimum | Self::Default)
    }

    /// Parses a profile name, case-insensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|profile| profile.name().eq_ignore_ascii_case(name))
    }
}

/// The configured view and surface band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidencyConfig {
    /// Profile the values came from.
    pub profile: Profile,
    /// Near voxel radius in metres (horizontal disc around the camera).
    pub near_radius_m: u32,
    /// Full vertical shell in metres (`±V / 2` around the surface).
    pub vertical_shell_m: u32,
}

impl ResidencyConfig {
    /// Profile defaults from `docs/reference-machine.md` §3.
    pub const fn from_profile(profile: Profile) -> Self {
        Self {
            profile,
            near_radius_m: profile.near_radius_m(),
            vertical_shell_m: profile.vertical_shell_m(),
        }
    }

    /// True when the radius or band no longer matches the named profile.
    pub const fn is_customized(&self) -> bool {
        self.near_radius_m != self.profile.near_radius_m()
            || self.vertical_shell_m != self.profile.vertical_shell_m()
    }

    /// Rejects a view/band that cannot describe a loaded region.
    pub fn validate(&self) -> Result<(), String> {
        if self.near_radius_m == 0 {
            return Err("near radius must be greater than zero".to_owned());
        }
        if self.vertical_shell_m == 0 {
            return Err("vertical shell must be greater than zero".to_owned());
        }
        if !self.vertical_shell_m.is_multiple_of(2) {
            return Err(format!(
                "vertical shell must be even so ±V/2 is a whole metre (got {} m)",
                self.vertical_shell_m
            ));
        }
        Ok(())
    }
}

impl Default for ResidencyConfig {
    fn default() -> Self {
        Self::from_profile(Profile::Default)
    }
}

/// One edge's geometry and the baseline chunk count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineCount {
    /// `log2` of the chunk edge in voxels.
    pub chunk_bits: u32,
    /// Voxels per chunk edge.
    pub chunk_dim: u32,
    /// Chunk edge in metres.
    pub edge_m: u32,
    /// Cubic metres in one chunk.
    pub chunk_volume_m3: u64,
    /// Chunk columns in the near disc.
    pub horizontal_columns: u64,
    /// Chunk layers in the vertical shell.
    pub vertical_layers: u64,
    /// `horizontal_columns × vertical_layers`.
    pub loaded_chunks: u64,
}

/// The target edge and its equal-volume count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCount {
    /// `log2` of the chunk edge in voxels.
    pub chunk_bits: u32,
    /// Voxels per chunk edge.
    pub chunk_dim: u32,
    /// Chunk edge in metres.
    pub edge_m: u32,
    /// Cubic metres in one chunk.
    pub chunk_volume_m3: u64,
    /// Equal-volume chunk count.
    pub loaded_chunks: u64,
    /// How the count was obtained; always `"equal_volume"`.
    pub derivation: String,
}

/// The equal-loaded-volume check the gate asserts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqualVolume {
    /// `target chunk volume / baseline chunk volume`.
    pub factor: u64,
    /// Baseline loaded chunks.
    pub n64: u64,
    /// Target loaded chunks at equal volume.
    pub n128: u64,
    /// `factor × n128`, the value asserted against `n64`.
    pub product: u64,
    /// Loaded volume at the baseline edge.
    pub n64_volume_m3: u64,
    /// Loaded volume at the target edge.
    pub n128_volume_m3: u64,
    /// True when both the count and the volume identities hold.
    pub holds: bool,
}

/// The frozen pre-registration row this run reproduces, when one exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreRegistered {
    /// Pre-registered 64³ count (`pre-registration.md` §2).
    pub n64: u64,
    /// Pre-registered 128³ count (`pre-registration.md` §2).
    pub n128: u64,
    /// Measured `n64 - pre_registered.n64`.
    pub n64_delta: i64,
    /// Measured `n128 - pre_registered.n128`.
    pub n128_delta: i64,
}

/// The complete M1-36 report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidencyReport {
    /// Schema version of this report.
    pub schema_version: u32,
    /// Profile name and the view/band actually used.
    pub config: ConfigInfo,
    /// 64³ baseline count.
    pub baseline: BaselineCount,
    /// 128³ equal-volume count.
    pub target: TargetCount,
    /// The equality the gate asserts.
    pub equal_volume: EqualVolume,
    /// Frozen prediction, absent for High or an overridden profile.
    pub pre_registered: Option<PreRegistered>,
    /// Definitions and caveats.
    pub notes: Vec<String>,
}

/// The configured view/band carried by the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigInfo {
    /// Base profile.
    pub profile: Profile,
    /// Whether the M1 gate measures this profile.
    pub gated: bool,
    /// True when `near_radius_m` or `vertical_shell_m` overrides the profile.
    pub customized: bool,
    /// Near voxel radius in metres.
    pub near_radius_m: u32,
    /// Full vertical shell in metres.
    pub vertical_shell_m: u32,
    /// Half the shell, i.e. the ± bound around the surface.
    pub vertical_shell_half_m: u32,
}

impl ResidencyReport {
    /// Renders the Markdown gate row.
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        writeln!(out, "# M1 gate residency (M1-36)\n").unwrap();
        writeln!(
            out,
            "**Profile:** `{}` ({}{})\n",
            self.config.profile.name(),
            if self.config.gated {
                "gated"
            } else {
                "not gated"
            },
            if self.config.customized {
                ", customized"
            } else {
                ""
            },
        )
        .unwrap();
        writeln!(
            out,
            "**View:** near voxel radius {} m (horizontal disc)\n",
            self.config.near_radius_m
        )
        .unwrap();
        writeln!(
            out,
            "**Band:** ±{} m vertical shell ({} m total)\n",
            self.config.vertical_shell_half_m, self.config.vertical_shell_m
        )
        .unwrap();
        writeln!(
            out,
            "**Baseline edge:** {}³ ({} m, {} m³/chunk)\n",
            self.baseline.chunk_dim,
            self.baseline.edge_m,
            grouped(self.baseline.chunk_volume_m3),
        )
        .unwrap();
        writeln!(
            out,
            "**Target edge:** {}³ ({} m, {} m³/chunk)\n",
            self.target.chunk_dim,
            self.target.edge_m,
            grouped(self.target.chunk_volume_m3),
        )
        .unwrap();

        writeln!(
            out,
            "| Edge | Chunk edge | Chunk volume | Horizontal columns | Vertical layers | Loaded chunks |"
        )
        .unwrap();
        writeln!(out, "|---|---:|---:|---:|---:|---:|").unwrap();
        writeln!(
            out,
            "| {}³ baseline | {} m | {} m³ | {} | {} | **{}** |",
            self.baseline.chunk_dim,
            self.baseline.edge_m,
            grouped(self.baseline.chunk_volume_m3),
            grouped(self.baseline.horizontal_columns),
            grouped(self.baseline.vertical_layers),
            grouped(self.baseline.loaded_chunks),
        )
        .unwrap();
        writeln!(
            out,
            "| {}³ equal volume | {} m | {} m³ | — | — | **{}** |",
            self.target.chunk_dim,
            self.target.edge_m,
            grouped(self.target.chunk_volume_m3),
            grouped(self.target.loaded_chunks),
        )
        .unwrap();

        writeln!(
            out,
            "\n**N64:** **{}** loaded chunks at the {}³ baseline ({} m chunks)\n",
            grouped(self.baseline.loaded_chunks),
            self.baseline.chunk_dim,
            self.baseline.edge_m,
        )
        .unwrap();
        writeln!(
            out,
            "**N128:** **{}** loaded chunks at the {}³ target ({} m chunks), equal loaded volume\n",
            grouped(self.target.loaded_chunks),
            self.target.chunk_dim,
            self.target.edge_m,
        )
        .unwrap();
        writeln!(
            out,
            "**Equal loaded volume:** {} × {} m³ = {} × {} m³ = {} m³; \
             {} × {} = {} → N64 = {} × N128 ✓\n",
            grouped(self.equal_volume.n64),
            self.baseline.chunk_volume_m3,
            grouped(self.equal_volume.n128),
            self.target.chunk_volume_m3,
            grouped(self.equal_volume.n64_volume_m3),
            grouped(self.equal_volume.factor),
            grouped(self.equal_volume.n128),
            grouped(self.equal_volume.product),
            grouped(self.equal_volume.factor),
        )
        .unwrap();

        match &self.pre_registered {
            Some(row) => {
                writeln!(
                    out,
                    "**Pre-registered prediction (pre-registration §2):** N64 = {}, N128 = {} \
                     (delta {} / {}).\n",
                    grouped(row.n64),
                    grouped(row.n128),
                    signed(row.n64_delta),
                    signed(row.n128_delta),
                )
                .unwrap();
            }
            None => {
                let reason = if self.config.customized {
                    "customized view/band"
                } else {
                    "High is not gated"
                };
                writeln!(out, "**Pre-registered prediction:** no row ({reason}).\n").unwrap();
            }
        }

        for note in &self.notes {
            writeln!(out, "- {note}").unwrap();
        }
        out
    }
}

/// Measures the default (Default profile) view/band.
pub fn default_report() -> Result<ResidencyReport, String> {
    report(ResidencyConfig::default())
}

/// Measures `config` and derives the target edge at equal loaded volume.
pub fn report(config: ResidencyConfig) -> Result<ResidencyReport, String> {
    config.validate()?;

    let baseline = baseline_count(&config)?;
    let target_bits = TARGET_CHUNK_BITS;
    let target_dim = u32::try_from(TARGET_CHUNK_DIM)
        .map_err(|_| "target chunk dimension does not fit in u32".to_owned())?;
    let target_edge = chunk_edge_m(target_bits)?;
    let target_volume = chunk_volume_m3(target_edge)?;

    if target_volume <= baseline.chunk_volume_m3
        || !target_volume.is_multiple_of(baseline.chunk_volume_m3)
    {
        return Err(format!(
            "target chunk volume {target_volume} m³ is not a whole multiple of the baseline \
             chunk volume {} m³",
            baseline.chunk_volume_m3
        ));
    }
    let factor = target_volume / baseline.chunk_volume_m3;

    if !baseline.loaded_chunks.is_multiple_of(factor) {
        return Err(format!(
            "the configured band gives N64 = {} chunks, which is not divisible by {factor}; \
             an equal-volume target count must be whole chunks. The gated 32 m shell yields \
             {} layers at the baseline and always satisfies this.",
            baseline.loaded_chunks, factor,
        ));
    }
    let n128 = baseline.loaded_chunks / factor;

    let n64_volume = baseline
        .loaded_chunks
        .checked_mul(baseline.chunk_volume_m3)
        .ok_or_else(|| "baseline loaded volume overflows u64".to_owned())?;
    let n128_volume = n128
        .checked_mul(target_volume)
        .ok_or_else(|| "target loaded volume overflows u64".to_owned())?;
    let product = factor
        .checked_mul(n128)
        .ok_or_else(|| "equal-volume product overflows u64".to_owned())?;

    let equal_volume = EqualVolume {
        factor,
        n64: baseline.loaded_chunks,
        n128,
        product,
        n64_volume_m3: n64_volume,
        n128_volume_m3: n128_volume,
        holds: baseline.loaded_chunks == product && n64_volume == n128_volume,
    };
    if !equal_volume.holds {
        return Err(format!(
            "equal-volume assertion failed: N64 = {}, N128 = {}, factor = {}, \
             volume {} m³ vs {} m³",
            baseline.loaded_chunks, n128, factor, n64_volume, n128_volume,
        ));
    }

    let config_info = ConfigInfo {
        profile: config.profile,
        gated: config.profile.gated(),
        customized: config.is_customized(),
        near_radius_m: config.near_radius_m,
        vertical_shell_m: config.vertical_shell_m,
        vertical_shell_half_m: config.vertical_shell_m / 2,
    };

    // The 128³ delta compares the equal-volume count with the table's
    // independently rounded one; it is expected to be +1 at Default.
    let pre_registered = if config.is_customized() {
        None
    } else {
        pre_registered_row(config.profile).map(|(expected_n64, expected_n128)| PreRegistered {
            n64: expected_n64,
            n128: expected_n128,
            n64_delta: delta(equal_volume.n64, expected_n64),
            n128_delta: delta(equal_volume.n128, expected_n128),
        })
    };

    let target = TargetCount {
        chunk_bits: target_bits,
        chunk_dim: target_dim,
        edge_m: target_edge,
        chunk_volume_m3: target_volume,
        loaded_chunks: n128,
        derivation: "equal_volume".to_owned(),
    };

    let mut notes = vec![
        format!(
            "N64 is the frozen residency model evaluated at the configured view/band: \
             H = ceil(pi × (R / edge)²) = {} columns, L = ceil(V / edge) = {} layers, \
             N64 = {} × {} = {}. The pre-registered table is a prediction; its deltas are \
             reported below rather than folding a table rounding back into the model.",
            grouped(baseline.horizontal_columns),
            grouped(baseline.vertical_layers),
            grouped(baseline.horizontal_columns),
            grouped(baseline.vertical_layers),
            grouped(baseline.loaded_chunks),
        ),
        format!(
            "N128 is the equal-volume equivalent, N64 × ({}³ / {}³) = N64 / {} = {}. It is not \
             an independent view measurement on the 128³ grid; equal loaded volume is the \
             constraint, and the report asserts both N64 = {} × N128 and the volume identity.",
            baseline.chunk_dim,
            target_dim,
            grouped(equal_volume.factor),
            grouped(equal_volume.n128),
            grouped(equal_volume.factor),
        ),
        format!(
            "The count is geometric, not content-dependent. The frozen 72-chunk fixture cannot \
             fill a {}-chunk view; per-chunk bytes come from `gate memory` (M1-35) and this row \
             supplies N for the memory gate.",
            grouped(baseline.loaded_chunks),
        ),
    ];
    if let Some(row) = pre_registered.as_ref()
        && (row.n64_delta != 0 || row.n128_delta != 0)
    {
        notes.push(format!(
            "Pre-registration §2 predicted N64 = {} and N128 = {}; the frozen formula gives \
             {} and {}. The difference is the table's nearest-integer chunk-area rounding, not \
             a change in the configured view.",
            grouped(row.n64),
            grouped(row.n128),
            grouped(equal_volume.n64),
            grouped(equal_volume.n128),
        ));
    }

    Ok(ResidencyReport {
        schema_version: SCHEMA_VERSION,
        config: config_info,
        baseline,
        target,
        equal_volume,
        pre_registered,
        notes,
    })
}

fn baseline_count(config: &ResidencyConfig) -> Result<BaselineCount, String> {
    let chunk_bits = BASELINE_CHUNK_BITS;
    let chunk_dim = u32::try_from(BASELINE_CHUNK_DIM)
        .map_err(|_| "baseline chunk dimension does not fit in u32".to_owned())?;
    let edge_m = chunk_edge_m(chunk_bits)?;
    let chunk_volume_m3 = chunk_volume_m3(edge_m)?;

    let horizontal_columns = horizontal_columns(config.near_radius_m, edge_m);
    if horizontal_columns == 0 {
        return Err(format!(
            "the {} m near radius covers no {} m chunk columns",
            config.near_radius_m, edge_m
        ));
    }
    let vertical_layers = u64::from(config.vertical_shell_m.div_ceil(edge_m));
    let loaded_chunks = horizontal_columns
        .checked_mul(vertical_layers)
        .ok_or_else(|| "baseline loaded chunk count overflows u64".to_owned())?;

    Ok(BaselineCount {
        chunk_bits,
        chunk_dim,
        edge_m,
        chunk_volume_m3,
        horizontal_columns,
        vertical_layers,
        loaded_chunks,
    })
}

/// Chunk edge in metres for a `log2` edge in voxels.
fn chunk_edge_m(chunk_bits: u32) -> Result<u32, String> {
    let blocks = chunk_bits
        .checked_sub(BLOCK_BITS)
        .ok_or_else(|| format!("chunk bits {chunk_bits} is smaller than BLOCK_BITS"))?;
    1u32.checked_shl(blocks)
        .ok_or_else(|| format!("chunk edge for {chunk_bits} bits overflows u32"))
}

fn chunk_volume_m3(edge_m: u32) -> Result<u64, String> {
    let edge = u64::from(edge_m);
    edge.checked_mul(edge)
        .and_then(|square| square.checked_mul(edge))
        .ok_or_else(|| format!("chunk volume for a {edge_m} m edge overflows u64"))
}

/// Chunk columns in the near disc, following pre-registration §2:
/// `ceil(pi * (R / edge)^2)`.
///
/// The frozen table beside the formula rounded some rows to nearest instead
/// (Minimum/64³ is eight chunks lower). This function follows the frozen
/// formula; the report prints the table delta rather than adjusting the model
/// after the freeze.
fn horizontal_columns(radius_m: u32, edge_m: u32) -> u64 {
    let radius_in_chunks = f64::from(radius_m) / f64::from(edge_m);
    let area = std::f64::consts::PI * radius_in_chunks * radius_in_chunks;
    area.ceil() as u64
}

/// The frozen table values for the two gated profiles.
fn pre_registered_row(profile: Profile) -> Option<(u64, u64)> {
    match profile {
        Profile::Minimum => Some((6_432, 804)),
        Profile::Default => Some((25_736, 3_216)),
        Profile::High => None,
    }
}

fn delta(measured: u64, expected: u64) -> i64 {
    i64::try_from(measured).unwrap_or(i64::MAX) - i64::try_from(expected).unwrap_or(i64::MAX)
}

fn signed(value: i64) -> String {
    if value > 0 {
        format!("+{value}")
    } else {
        value.to_string()
    }
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_reproduces_the_frozen_64_count() {
        let report = default_report().expect("default residency report");
        assert_eq!(report.config.profile, Profile::Default);
        assert_eq!(report.config.near_radius_m, 128);
        assert_eq!(report.config.vertical_shell_m, 32);
        assert_eq!(report.config.vertical_shell_half_m, 16);
        assert_eq!(report.baseline.chunk_dim, 64);
        assert_eq!(report.baseline.edge_m, 4);
        assert_eq!(report.baseline.horizontal_columns, 3_217);
        assert_eq!(report.baseline.vertical_layers, 8);
        assert_eq!(report.baseline.loaded_chunks, 25_736);
        assert_eq!(report.target.chunk_dim, 128);
        assert_eq!(report.target.loaded_chunks, 3_217);
        assert_eq!(report.equal_volume.factor, 8);
        assert!(report.equal_volume.holds);
        assert_eq!(report.equal_volume.n64, 8 * report.equal_volume.n128);
        assert_eq!(
            report.equal_volume.n64_volume_m3,
            report.equal_volume.n128_volume_m3
        );
    }

    #[test]
    fn minimum_profile_follows_the_formula_and_reports_the_table_delta() {
        let report = report(ResidencyConfig::from_profile(Profile::Minimum))
            .expect("minimum residency report");
        assert_eq!(report.baseline.horizontal_columns, 805);
        assert_eq!(report.baseline.vertical_layers, 8);
        assert_eq!(report.baseline.loaded_chunks, 6_440);
        assert_eq!(report.target.loaded_chunks, 805);
        assert_eq!(report.equal_volume.n64, 8 * report.equal_volume.n128);
        let row = report.pre_registered.as_ref().unwrap();
        assert_eq!(
            row.n64_delta, 8,
            "the table rounded 805 columns down to 804"
        );
        assert_eq!(row.n128_delta, 1);
    }

    #[test]
    fn every_gated_profile_holds_the_equal_volume_identity() {
        for profile in Profile::ALL {
            let report = report(ResidencyConfig::from_profile(profile))
                .unwrap_or_else(|error| panic!("{}: {error}", profile.name()));
            assert_eq!(
                report.baseline.loaded_chunks,
                report.equal_volume.factor * report.equal_volume.n128,
                "{}",
                profile.name()
            );
            assert_eq!(
                report.equal_volume.n64_volume_m3,
                report.equal_volume.n128_volume_m3,
                "{}",
                profile.name()
            );
            assert!(report.equal_volume.holds, "{}", profile.name());
        }
    }

    #[test]
    fn overrides_are_measured_from_the_config_and_skip_the_table() {
        let config = ResidencyConfig {
            profile: Profile::Default,
            near_radius_m: 128,
            vertical_shell_m: 64,
        };
        let report = report(config).expect("overridden residency report");
        assert!(report.config.customized);
        assert_eq!(report.baseline.horizontal_columns, 3_217);
        assert_eq!(report.baseline.vertical_layers, 16);
        assert_eq!(report.baseline.loaded_chunks, 51_472);
        assert_eq!(report.target.loaded_chunks, 6_434);
        assert_eq!(report.pre_registered, None);
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let config = ResidencyConfig {
            near_radius_m: 0,
            ..ResidencyConfig::default()
        };
        assert!(report(config).is_err());

        let config = ResidencyConfig {
            vertical_shell_m: 0,
            ..ResidencyConfig::default()
        };
        assert!(report(config).is_err());

        let config = ResidencyConfig {
            vertical_shell_m: 31,
            ..ResidencyConfig::default()
        };
        assert!(report(config).is_err());

        let config = ResidencyConfig {
            vertical_shell_m: 16,
            ..ResidencyConfig::default()
        };
        let error = report(config).expect_err("16 m is not divisible into 8 equal-volume chunks");
        assert!(error.contains("not divisible by 8"), "{error}");
    }

    #[test]
    fn names_parse_case_insensitively() {
        assert_eq!(Profile::from_name("minimum"), Some(Profile::Minimum));
        assert_eq!(Profile::from_name("Default"), Some(Profile::Default));
        assert_eq!(Profile::from_name("HIGH"), Some(Profile::High));
        assert_eq!(Profile::from_name("ultra"), None);
    }

    #[test]
    fn grouping_is_readable_and_gate_numbers_keep_their_commas() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(804), "804");
        assert_eq!(grouped(25_736), "25,736");
        assert_eq!(grouped(1_647_104), "1,647,104");
    }
}
