//! Frozen, representative fixture world (M1-28).
//!
//! A save-scan report answers "what is in this save?", but without a known
//! save there is nothing stable to scan. This module freezes one:
//!
//! * [`WorldGenerator`] builds deterministic terrain, caves, overhangs,
//!   structures and vegetation (see [`world`]);
//! * [`build`] writes the save through [`voxel_storage::world::World`], records
//!   its file hashes and a manifest stating where it came from;
//! * [`verify`] checks the checked-in bytes and rebuilds the save from the
//!   manifest recipe, so "reproducible" is tested rather than asserted.
//!
//! The fixture is **provisional until the first real save**. It is synthetic,
//! it has no palette record of its own, and its numbers are illustrative until
//! a gameplay save is captured and this fixture is re-frozen. The manifest
//! carries that limitation next to the provenance so no report can quote the
//! fixture without also having the caveat available.
//!
//! The save uses raw material ids, not namespaced keys: the region log format
//! stores ids and a save-level palette does not exist yet. [`MATERIALS`] is the
//! id-to-key mapping for this frozen file and is checked against
//! [`voxel_block::BlockKind::ALL`] at build and verify time.

pub mod world;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::save_scan::{Distribution, SaveScanReport, scan};
use voxel_storage::gpu::MemSink;
use voxel_storage::scan::scan_save;
use voxel_storage::serialize::VERSION as CHUNK_VERSION;
use voxel_storage::world::World;
use voxel_storage::{CHUNK_BITS, CHUNK_DIM, CHUNK_VOLUME, ChunkPos, local_index};

pub use world::{
    DEEP_AIR_CEILING, MATERIALS, Material, SEA_LEVEL, WorldGenerator, mat, recipe_features,
};

/// Schema version of the manifest itself.
pub const SCHEMA_VERSION: u32 = 1;
/// Generator identity written into the recipe.
pub const GENERATOR: &str = "m1-representative";
/// Generator version; bump when the bytes of a recipe change.
pub const GENERATOR_VERSION: u32 = 1;
/// Default fixture directory, relative to the repository root.
pub const DEFAULT_DIR: &str = "benchmarks/fixtures/m1-representative";

/// The distinctive limitation, kept verbatim in the manifest.
pub const LIMITATION: &str = "Provisional until the first real save. This world is synthetic: it exercises the shape of a save (terrain, caves, overhangs, structures and mixed materials) but was not produced by gameplay, by docs/worldgen-design.md, or by the save path on the reference machine. Its byte sizes, material histograms and distributions are illustrative only and must not be quoted as gate results (M1-35, M1-37) until a real save is captured and this fixture is re-frozen.";

/// The five feature families the fixture is required to contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    Terrain,
    Caves,
    Overhangs,
    Structures,
    MixedMaterials,
}

/// Fixture lifecycle. Only `provisional` ships today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureStatus {
    Provisional,
}

/// How to regenerate the bytes, independent of the machine that froze them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipe {
    pub generator: String,
    pub generator_version: u32,
    pub seed: u64,
    /// Inclusive chunk coordinates.
    pub chunk_min: [i32; 3],
    pub chunk_max: [i32; 3],
    pub sea_level: i32,
    pub features: Vec<Feature>,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            generator: GENERATOR.to_owned(),
            generator_version: GENERATOR_VERSION,
            seed: 0x4D31_2F28_2026_1026,
            chunk_min: [-3, 0, -3],
            chunk_max: [2, 1, 2],
            sea_level: SEA_LEVEL,
            features: recipe_features().to_vec(),
        }
    }
}

impl Recipe {
    /// Rejects recipes this build cannot generate or reproduce.
    pub fn validate(&self) -> Result<(), String> {
        if self.generator != GENERATOR {
            return Err(format!(
                "unknown fixture generator `{}` (this build knows `{GENERATOR}`)",
                self.generator
            ));
        }
        if self.generator_version != GENERATOR_VERSION {
            return Err(format!(
                "fixture generator version {} is not supported (this build is {GENERATOR_VERSION})",
                self.generator_version
            ));
        }
        for axis in 0..3 {
            if self.chunk_min[axis] > self.chunk_max[axis] {
                return Err(format!(
                    "chunk_min {:?} is greater than chunk_max {:?}",
                    self.chunk_min, self.chunk_max
                ));
            }
            if !(-4_096..=4_096).contains(&self.chunk_min[axis])
                || !(-4_096..=4_096).contains(&self.chunk_max[axis])
            {
                return Err("fixture chunk bounds must stay within ±4096 chunks".to_owned());
            }
        }
        if self.chunk_min[1] < 0 || self.chunk_max[1] > 8 {
            return Err("fixture chunk y must stay in 0..=8".to_owned());
        }
        if !(0..=120).contains(&self.sea_level) {
            return Err("fixture sea level must stay in 0..=120".to_owned());
        }
        let volume = self.chunk_volume();
        if volume == 0 || volume > 1_024 {
            return Err(format!(
                "fixture recipe covers {volume} chunks; the supported range is 1..=1024"
            ));
        }
        let features: BTreeSet<Feature> = self.features.iter().copied().collect();
        for feature in recipe_features() {
            if !features.contains(&feature) {
                return Err(format!("fixture recipe is missing feature {feature:?}"));
            }
        }
        Ok(())
    }

    /// Number of chunks in the rectangular recipe.
    pub fn chunk_volume(&self) -> usize {
        let axis = |min: i32, max: i32| (max - min + 1) as usize;
        axis(self.chunk_min[0], self.chunk_max[0])
            * axis(self.chunk_min[1], self.chunk_max[1])
            * axis(self.chunk_min[2], self.chunk_max[2])
    }

    /// Inclusive world-coordinate bounds covered by the recipe.
    pub fn world_bounds(&self) -> ([i32; 3], [i32; 3]) {
        let edge = CHUNK_DIM as i32;
        (
            [
                self.chunk_min[0] * edge,
                self.chunk_min[1] * edge,
                self.chunk_min[2] * edge,
            ],
            [
                (self.chunk_max[0] + 1) * edge - 1,
                (self.chunk_max[1] + 1) * edge - 1,
                (self.chunk_max[2] + 1) * edge - 1,
            ],
        )
    }

    fn positions(&self) -> Vec<ChunkPos> {
        let mut positions = Vec::with_capacity(self.chunk_volume());
        for y in self.chunk_min[1]..=self.chunk_max[1] {
            for z in self.chunk_min[2]..=self.chunk_max[2] {
                for x in self.chunk_min[0]..=self.chunk_max[0] {
                    positions.push(ChunkPos::new(x, y, z));
                }
            }
        }
        positions
    }
}

/// Where the fixture came from, in the form a later report can cite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub kind: String,
    pub generator: String,
    pub generator_source: String,
    pub captured_from_gameplay: bool,
    pub external_data: bool,
    pub generated_by: String,
    pub generated_at: String,
    pub source_commit: Option<String>,
    pub source_worktree_dirty: Option<bool>,
    pub worldgen: String,
}

/// The on-disk formats the hashes cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub chunk_format: String,
    pub chunk_version: u16,
    pub chunk_bits: u32,
    pub chunk_dim: u32,
    pub region_format: String,
}

impl Format {
    fn current() -> Self {
        Self {
            chunk_format: "VXV2".to_owned(),
            chunk_version: CHUNK_VERSION,
            chunk_bits: CHUNK_BITS,
            chunk_dim: CHUNK_DIM as u32,
            region_format: "vxr-1".to_owned(),
        }
    }
}

/// One raw id and the key it meant when the fixture was frozen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteEntry {
    pub id: u16,
    pub key: String,
    pub name: String,
    pub role: String,
}

fn palette() -> Vec<PaletteEntry> {
    MATERIALS
        .iter()
        .map(|material| PaletteEntry {
            id: material.id,
            key: material.key.to_owned(),
            name: material.name.to_owned(),
            role: material.role.to_owned(),
        })
        .collect()
}

/// Content hash of one region file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDigest {
    pub path: String,
    pub bytes: u64,
    pub blake3: String,
}

/// Hash record for the frozen save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integrity {
    pub algorithm: String,
    pub world_hash: String,
    pub total_bytes: u64,
    pub files: Vec<FileDigest>,
}

/// One material's non-air voxel count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialCount {
    pub id: u16,
    pub name: String,
    pub count: u64,
}

/// What the save-scan reference reader observed when the fixture was frozen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observed {
    pub region_files: usize,
    pub file_bytes: u64,
    pub record_bytes: u64,
    pub live_bytes: u64,
    pub superseded_bytes: u64,
    pub live_chunks: usize,
    pub total_voxels: u64,
    pub total_overrides: u64,
    pub materials: Vec<MaterialCount>,
    pub node_bytes: Distribution,
    pub voxels: Distribution,
    pub warnings: Vec<String>,
}

impl Observed {
    fn from_report(report: SaveScanReport) -> Self {
        let materials = report
            .materials
            .into_iter()
            .map(|(id, count)| MaterialCount {
                id,
                name: material_name(id),
                count,
            })
            .collect();
        Self {
            region_files: report.region_files,
            file_bytes: report.file_bytes,
            record_bytes: report.record_bytes,
            live_bytes: report.live_bytes,
            superseded_bytes: report.superseded_bytes,
            live_chunks: report.live_chunks,
            total_voxels: report.total_voxels,
            total_overrides: report.total_overrides,
            materials,
            node_bytes: report.node_bytes,
            voxels: report.voxels,
            warnings: report.warnings,
        }
    }
}

/// Coarse evidence that each required shape is actually in the save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    /// Air below [`DEEP_AIR_CEILING`], which cannot be sky.
    pub cave_air_voxels: u64,
    /// Solid blocks with two air cells below and air within four above: a roof
    /// over walkable space. The canyon shelves dominate this count.
    pub overhang_roof_blocks: u64,
    /// Cobblestone voxels, which only the tower, ruin, bridge and shelf edges
    /// produce.
    pub structure_blocks: u64,
    /// Log and leaf voxels, which only trees produce.
    pub tree_blocks: u64,
}

/// The fixture manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub status: FixtureStatus,
    pub provisional_until_first_real_save: bool,
    pub limitation: String,
    pub save_root: String,
    pub provenance: Provenance,
    pub format: Format,
    pub recipe: Recipe,
    pub palette: Vec<PaletteEntry>,
    pub integrity: Integrity,
    pub observed: Observed,
    pub checks: Checks,
}

/// Result of building a fixture.
#[derive(Debug)]
pub struct BuildOutcome {
    pub manifest_path: PathBuf,
    pub save_dir: PathBuf,
    pub manifest: Manifest,
}

/// Result of verifying a fixture.
#[derive(Debug)]
pub struct VerifyOutcome {
    pub manifest_path: PathBuf,
    pub save_dir: PathBuf,
    pub manifest: Manifest,
    pub rebuilt: bool,
}

struct Generated {
    files: BTreeMap<String, Vec<u8>>,
    integrity: Integrity,
    observed: Observed,
    checks: Checks,
}

/// The default manifest path, relative to the current directory.
pub fn default_manifest() -> PathBuf {
    PathBuf::from(DEFAULT_DIR).join("manifest.json")
}

/// Builds the default recipe into `out` and freezes its manifest.
///
/// Refuses to overwrite an existing fixture: a freeze is a deliberate event.
pub fn build(out: &Path) -> Result<BuildOutcome, String> {
    if out.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite a frozen fixture (remove it to re-freeze)",
            out.display()
        ));
    }
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no usable directory name", out.display()))?;
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create fixture parent {}: {error}",
            parent.display()
        )
    })?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let tmp = parent.join(format!(".{name}.tmp-{}-{stamp}", std::process::id()));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)
            .map_err(|error| format!("failed to clear {}: {error}", tmp.display()))?;
    }
    fs::create_dir_all(&tmp)
        .map_err(|error| format!("failed to create {}: {error}", tmp.display()))?;

    let result = (|| {
        let recipe = Recipe::default();
        let save_dir = tmp.join("save");
        let generated = generate(&recipe, &save_dir)?;
        validate_evidence(&recipe, &generated.observed, &generated.checks)?;
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            id: name.to_owned(),
            status: FixtureStatus::Provisional,
            provisional_until_first_real_save: true,
            limitation: LIMITATION.to_owned(),
            save_root: "save".to_owned(),
            provenance: provenance(parent),
            format: Format::current(),
            recipe,
            palette: palette(),
            integrity: generated.integrity,
            observed: generated.observed,
            checks: generated.checks,
        };
        let manifest_path = tmp.join("manifest.json");
        write_manifest(&manifest_path, &manifest)?;
        fs::rename(&tmp, out).map_err(|error| {
            format!(
                "failed to publish {} -> {}: {error}",
                tmp.display(),
                out.display()
            )
        })?;
        Ok(BuildOutcome {
            manifest_path: out.join("manifest.json"),
            save_dir: out.join("save"),
            manifest,
        })
    })();

    if result.is_err() && tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result
}

/// Verifies the checked-in bytes, the manifest mapping, and a fresh rebuild.
///
/// The rebuild is the contract: if the recipe cannot reproduce the checked-in
/// save byte-for-byte, the fixture is not frozen, it is just committed.
pub fn verify(manifest_path: &Path) -> Result<VerifyOutcome, String> {
    let manifest = read_manifest(manifest_path)?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "manifest schema {} is not supported (this build is {SCHEMA_VERSION})",
            manifest.schema_version
        ));
    }
    manifest.recipe.validate()?;
    if manifest.palette != palette() {
        return Err(
            "the manifest palette does not match this build's built-in material order; \
             the save's raw ids would mean different blocks"
                .to_owned(),
        );
    }
    let format = Format::current();
    if manifest.format.chunk_bits != CHUNK_BITS {
        return Err(format!(
            "fixture was frozen at {}-bit chunks ({}³); this build uses {}-bit chunks ({}³). \
             The 64³ fixture is intentionally not valid at another edge; re-freeze at the matching edge.",
            manifest.format.chunk_bits, manifest.format.chunk_dim, CHUNK_BITS, CHUNK_DIM
        ));
    }
    if manifest.format != format {
        return Err(format!(
            "fixture format {:?} does not match this build {:?}; re-freeze the fixture",
            manifest.format, format
        ));
    }

    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let save_dir = fixture_path(root, &manifest.save_root)?;
    let files = read_files(&save_dir)?;
    let integrity = integrity(&files);
    if integrity != manifest.integrity {
        return Err(describe_integrity_mismatch(&manifest.integrity, &integrity));
    }

    let report = scan(&save_dir).map_err(|error| {
        format!(
            "failed to scan checked-in save {}: {error}",
            save_dir.display()
        )
    })?;
    let observed = Observed::from_report(report);
    let checks = analyze(&save_dir)?;
    if observed != manifest.observed {
        return Err(
            "checked-in save observations differ from the manifest; the fixture was edited"
                .to_owned(),
        );
    }
    if checks != manifest.checks {
        return Err(
            "checked-in save feature evidence differs from the manifest; the fixture was edited"
                .to_owned(),
        );
    }
    validate_evidence(&manifest.recipe, &observed, &checks)?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let rebuild_root = std::env::temp_dir().join(format!(
        "voxel-fixture-verify-{}-{stamp}",
        std::process::id()
    ));
    if rebuild_root.exists() {
        fs::remove_dir_all(&rebuild_root)
            .map_err(|error| format!("failed to clear {}: {error}", rebuild_root.display()))?;
    }
    let regenerate = (|| {
        let rebuild_save = rebuild_root.join("save");
        let rebuilt = generate(&manifest.recipe, &rebuild_save)?;
        if rebuilt.files != files {
            return Err(first_difference(&files, &rebuilt.files));
        }
        if rebuilt.observed != observed || rebuilt.checks != checks {
            return Err(
                "the recipe rebuilt the same files but different observations; this is a verifier bug"
                    .to_owned(),
            );
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&rebuild_root);
    regenerate?;

    Ok(VerifyOutcome {
        manifest_path: manifest_path.to_path_buf(),
        save_dir,
        manifest,
        rebuilt: true,
    })
}

fn fixture_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "manifest save_root `{}` must be a relative path inside the fixture",
            relative.display()
        ));
    }
    Ok(root.join(relative))
}

fn provenance(fixture_root: &Path) -> Provenance {
    Provenance {
        kind: "synthetic-engine-generated".to_owned(),
        generator: format!("voxel-bench fixture world ({GENERATOR} v{GENERATOR_VERSION})"),
        generator_source: "crates/bench/src/fixture/world.rs".to_owned(),
        captured_from_gameplay: false,
        external_data: false,
        generated_by: "cargo run -p voxel-bench -- fixture build".to_owned(),
        generated_at: utc_now(),
        source_commit: git(&["rev-parse", "--short=12", "HEAD"]),
        source_worktree_dirty: git_dirty(fixture_root),
        worldgen: "Deterministic stand-in, not docs/worldgen-design.md; frozen for M1 measurement."
            .to_owned(),
    }
}

fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    utc_stamp(seconds)
}

/// `20261026T223346Z` from a Unix timestamp.
fn utc_stamp(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// True when the worktree has changes outside the fixture directory.
fn git_dirty(fixture_root: &Path) -> Option<bool> {
    let status = git(&["status", "--porcelain"])?;
    let root = git(&["rev-parse", "--show-toplevel"])?;
    let fixture_root = std::path::absolute(fixture_root).ok()?;
    let prefix = fixture_root
        .strip_prefix(Path::new(&root))
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"));

    for line in status.lines() {
        let path = line.get(3..).unwrap_or(line);
        let path = path.split(" -> ").last().unwrap_or(path).trim_matches('"');
        let inside_fixture = prefix
            .as_deref()
            .is_some_and(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")));
        if !inside_fixture {
            return Some(true);
        }
    }
    Some(false)
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("failed to serialize fixture manifest: {error}"))?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read manifest {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse manifest {}: {error}", path.display()))
}

/// Generates a save from `recipe` into `save_dir` and hashes it.
fn generate(recipe: &Recipe, save_dir: &Path) -> Result<Generated, String> {
    recipe.validate()?;
    if save_dir.exists() {
        let empty = fs::read_dir(save_dir)
            .map_err(|error| format!("failed to read {}: {error}", save_dir.display()))?
            .next()
            .is_none();
        if !empty {
            return Err(format!(
                "save directory {} is not empty; refusing to mix a new fixture with old bytes",
                save_dir.display()
            ));
        }
    }
    fs::create_dir_all(save_dir)
        .map_err(|error| format!("failed to create {}: {error}", save_dir.display()))?;

    let generator = WorldGenerator::new(recipe)?;
    let (cave_count, extra_count) = generator.feature_counts();
    let mut world = World::new(
        save_dir,
        Box::new(move |pos, out| generator.fill(pos, out)),
        MemSink::default(),
    )
    .map_err(|error| format!("failed to open fixture world: {error}"))?;

    for pos in recipe.positions() {
        world
            .load(pos)
            .map_err(|error| format!("failed to generate chunk {pos:?}: {error}"))?;
        let dag = world
            .chunk(pos)
            .ok_or_else(|| format!("chunk {pos:?} disappeared after loading"))?
            .dag
            .clone();
        world
            .disk_mut()
            .put_dag(pos, &dag)
            .map_err(|error| format!("failed to write chunk {pos:?}: {error}"))?;
    }
    world
        .disk_mut()
        .sync()
        .map_err(|error| format!("failed to sync fixture save: {error}"))?;
    drop(world);

    let files = read_files(save_dir)?;
    let integrity = integrity(&files);
    let report = scan(save_dir).map_err(|error| {
        format!(
            "failed to scan freshly generated save {}: {error}",
            save_dir.display()
        )
    })?;
    let observed = Observed::from_report(report);
    if !observed.warnings.is_empty() {
        return Err(format!(
            "freshly generated save has scan warnings: {:?}",
            observed.warnings
        ));
    }
    if observed.total_overrides != 0 || observed.superseded_bytes != 0 {
        return Err(format!(
            "freshly generated save is not a clean freeze: {} overrides, {} superseded bytes",
            observed.total_overrides, observed.superseded_bytes
        ));
    }
    let checks = analyze(save_dir)?;
    if cave_count == 0 || extra_count == 0 {
        return Err("generator produced no caves or no extra features".to_owned());
    }
    Ok(Generated {
        files,
        integrity,
        observed,
        checks,
    })
}

fn read_files(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    for entry in
        fs::read_dir(dir).map_err(|error| format!("failed to read {}: {error}", dir.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read {}: {error}", dir.display()))?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("non-UTF-8 file name in {}", dir.display()))?;
        if !path.is_file() {
            return Err(format!(
                "unexpected non-file entry {} in the save",
                path.display()
            ));
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        files.insert(name, bytes);
    }
    Ok(files)
}

fn integrity(files: &BTreeMap<String, Vec<u8>>) -> Integrity {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"voxel-fixture-world-v1\0");
    let mut total_bytes = 0u64;
    let mut digests = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        total_bytes += bytes.len() as u64;
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
        digests.push(FileDigest {
            path: path.clone(),
            bytes: bytes.len() as u64,
            blake3: blake3::hash(bytes).to_hex().to_string(),
        });
    }
    Integrity {
        algorithm: "blake3-256".to_owned(),
        world_hash: hasher.finalize().to_hex().to_string(),
        total_bytes,
        files: digests,
    }
}

fn describe_integrity_mismatch(expected: &Integrity, actual: &Integrity) -> String {
    if expected.world_hash != actual.world_hash {
        return format!(
            "fixture hash mismatch: manifest {} but checked-in save is {}",
            expected.world_hash, actual.world_hash
        );
    }
    if expected.total_bytes != actual.total_bytes {
        return format!(
            "fixture size mismatch: manifest {} bytes but checked-in save is {} bytes",
            expected.total_bytes, actual.total_bytes
        );
    }
    let expected_names: BTreeSet<&str> = expected
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let actual_names: BTreeSet<&str> = actual.files.iter().map(|file| file.path.as_str()).collect();
    let missing: Vec<&str> = expected_names.difference(&actual_names).copied().collect();
    let extra: Vec<&str> = actual_names.difference(&expected_names).copied().collect();
    if !missing.is_empty() || !extra.is_empty() {
        return format!("fixture file set changed: missing {missing:?}, extra {extra:?}");
    }
    for (expected, actual) in expected.files.iter().zip(actual.files.iter()) {
        if expected.bytes != actual.bytes {
            return format!(
                "fixture file {} size {} != {}",
                expected.path, expected.bytes, actual.bytes
            );
        }
    }
    "fixture integrity differs from the manifest".to_owned()
}

fn first_difference(
    expected: &BTreeMap<String, Vec<u8>>,
    actual: &BTreeMap<String, Vec<u8>>,
) -> String {
    let expected_names: BTreeSet<&str> = expected.keys().map(String::as_str).collect();
    let actual_names: BTreeSet<&str> = actual.keys().map(String::as_str).collect();
    let missing: Vec<&str> = expected_names.difference(&actual_names).copied().collect();
    let extra: Vec<&str> = actual_names.difference(&expected_names).copied().collect();
    if !missing.is_empty() || !extra.is_empty() {
        return format!(
            "rebuild produced a different file set: missing {missing:?}, extra {extra:?}"
        );
    }
    for (name, expected) in expected {
        let actual = &actual[name];
        if expected != actual {
            if let Some(index) = expected.iter().zip(actual).position(|(a, b)| a != b) {
                return format!(
                    "rebuild differs from the frozen fixture in {name} at byte {index} ({} vs {} bytes)",
                    expected.len(),
                    actual.len()
                );
            }
            return format!(
                "rebuild differs from the frozen fixture in {name} length ({} vs {} bytes)",
                expected.len(),
                actual.len()
            );
        }
    }
    "rebuild produced identical files".to_owned()
}

fn material_name(id: u16) -> String {
    MATERIALS
        .iter()
        .find(|material| material.id == id)
        .map_or_else(
            || format!("unknown:{id}"),
            |material| material.name.to_owned(),
        )
}

/// Recomputes the geometric evidence from the save, independently of the
/// generator's feature list.
fn analyze(dir: &Path) -> Result<Checks, String> {
    let mut dense = vec![0u16; CHUNK_VOLUME];
    let mut checks = Checks {
        cave_air_voxels: 0,
        overhang_roof_blocks: 0,
        structure_blocks: 0,
        tree_blocks: 0,
    };
    let summary = scan_save(dir, |chunk| {
        chunk.dag.expand(&mut dense);
        for (index, material) in chunk.overlay.iter() {
            dense[index as usize] = material;
        }
        let edge = CHUNK_DIM as i32;
        let oy = chunk.pos.y * edge;
        for ly in 0..CHUNK_DIM {
            let y = oy + ly as i32;
            for lz in 0..CHUNK_DIM {
                for lx in 0..CHUNK_DIM {
                    let material = dense[local_index(lx as u32, ly as u32, lz as u32) as usize];
                    if material == mat::AIR && y < DEEP_AIR_CEILING {
                        checks.cave_air_voxels += 1;
                    }
                    if material == mat::COBBLESTONE {
                        checks.structure_blocks += 1;
                    }
                    if material == mat::OAK_LOG || material == mat::OAK_LEAVES {
                        checks.tree_blocks += 1;
                    }
                    if is_roof_material(material) {
                        checks.overhang_roof_blocks +=
                            overhang_roof(&dense, lx as u32, ly as u32, lz as u32);
                    }
                }
            }
        }
    })
    .map_err(|error| format!("failed to analyze fixture {}: {error}", dir.display()))?;
    if !summary.warnings.is_empty() {
        return Err(format!(
            "fixture analysis reported warnings: {:?}",
            summary.warnings
        ));
    }
    Ok(checks)
}

/// Counts one solid cell as a roof when it has two air cells below and air
/// within four cells above.
fn overhang_roof(dense: &[u16], lx: u32, ly: u32, lz: u32) -> u64 {
    let edge = CHUNK_DIM as i32;
    if ly < 2 {
        return 0;
    }
    let below = |depth: u32| dense[local_index(lx, ly - depth, lz) as usize];
    if below(1) != mat::AIR || below(2) != mat::AIR {
        return 0;
    }
    for dy in 1..=4 {
        if (ly as i32 + dy) >= edge {
            break;
        }
        if dense[local_index(lx, ly + dy as u32, lz) as usize] == mat::AIR {
            return 1;
        }
    }
    0
}

fn is_roof_material(material: u16) -> bool {
    matches!(
        material,
        mat::STONE
            | mat::DIRT
            | mat::GRASS
            | mat::COBBLESTONE
            | mat::SAND
            | mat::GRAVEL
            | mat::SNOW
            | mat::ICE
            | mat::SANDSTONE
    )
}

fn validate_evidence(recipe: &Recipe, observed: &Observed, checks: &Checks) -> Result<(), String> {
    if observed.live_chunks != recipe.chunk_volume() {
        return Err(format!(
            "fixture has {} live chunks but the recipe describes {}",
            observed.live_chunks,
            recipe.chunk_volume()
        ));
    }
    if observed.total_voxels == 0 {
        return Err("fixture has no non-air voxels".to_owned());
    }
    if checks.cave_air_voxels < 1_000 {
        return Err(format!(
            "fixture has only {} cave-air voxels below y={DEEP_AIR_CEILING}; expected caves",
            checks.cave_air_voxels
        ));
    }
    if checks.overhang_roof_blocks < 256 {
        return Err(format!(
            "fixture has only {} overhang roof blocks; expected overhangs",
            checks.overhang_roof_blocks
        ));
    }
    if checks.structure_blocks < 512 {
        return Err(format!(
            "fixture has only {} structure blocks; expected a tower, ruin and bridge",
            checks.structure_blocks
        ));
    }
    if checks.tree_blocks < 512 {
        return Err(format!(
            "fixture has only {} tree blocks; expected trees",
            checks.tree_blocks
        ));
    }
    let present: BTreeSet<&str> = observed
        .materials
        .iter()
        .map(|material| material.name.as_str())
        .collect();
    let required = [
        "stone",
        "dirt",
        "grass_block",
        "cobblestone",
        "sand",
        "gravel",
        "water",
        "oak_log",
        "oak_leaves",
        "bedrock",
        "coal_ore",
        "iron_ore",
        "gold_ore",
        "diamond_ore",
        "tall_grass",
        "red_flower",
        "snow",
        "ice",
        "sandstone",
    ];
    let missing: Vec<&str> = required
        .into_iter()
        .filter(|name| !present.contains(name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "fixture is missing required materials {missing:?}; it is not mixed enough"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_recipe_is_valid_and_covers_five_features() {
        let recipe = Recipe::default();
        recipe.validate().unwrap();
        assert_eq!(recipe.chunk_volume(), 72);
        let edge = CHUNK_DIM as i32;
        assert_eq!(recipe.world_bounds().0, [-3 * edge, 0, -3 * edge]);
        assert_eq!(
            recipe.world_bounds().1,
            [3 * edge - 1, 2 * edge - 1, 3 * edge - 1]
        );
        assert_eq!(recipe.features.len(), 5);
    }

    #[test]
    fn manifest_limitation_names_the_first_real_save() {
        assert!(LIMITATION.contains("Provisional until the first real save"));
        assert!(LIMITATION.contains("M1-35"));
    }

    #[test]
    fn utc_stamp_matches_a_known_date() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_793_342_701), "20261030T064501Z");
    }

    #[test]
    fn hash_changes_when_a_file_changes() {
        let mut files = BTreeMap::new();
        files.insert("r.0.0.0.vxr".to_owned(), vec![1u8, 2, 3]);
        let one = integrity(&files);
        files.insert("r.0.0.0.vxr".to_owned(), vec![1u8, 2, 4]);
        let two = integrity(&files);
        assert_ne!(one.world_hash, two.world_hash);
        assert_ne!(one.files[0].blake3, two.files[0].blake3);
    }

    #[test]
    fn save_root_rejects_traversal() {
        assert!(fixture_path(Path::new("/tmp/x"), "../escape").is_err());
        assert!(fixture_path(Path::new("/tmp/x"), "/absolute").is_err());
        assert!(fixture_path(Path::new("/tmp/x"), "save").is_ok());
    }
}
