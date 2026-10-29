//! The generation pipeline: graphs -> PNGs -> manifest -> materials -> atlas.
//!
//! Every entry point in this module is deterministic and rooted at a
//! [`Workspace`], the directory that contains `crates/engine/block`. The GUI
//! and the CLI share exactly these functions, so a button press and a CI check
//! cannot disagree about what "generate" means.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use voxel_block::texture_manifest::{
    AlphaClass, TextureEntry, TextureManifest, Tiling, validate as validate_manifest,
};

use crate::bake::{self, BakeError};
use crate::graph::{Graph, GraphError, pretty_config};
use crate::png::{self, PngError};

/// The committed face set: key and atlas slot. Graph files are named after the
/// part following `voxel:block/`.
pub const SLICES: &[(&str, u16)] = &[
    ("voxel:block/stone", 1),
    ("voxel:block/dirt", 2),
    ("voxel:block/grass_block_top", 3),
    ("voxel:block/grass_block_side", 4),
    ("voxel:block/cobblestone", 5),
    ("voxel:block/sand", 6),
    ("voxel:block/gravel", 7),
    ("voxel:block/water_still", 8),
    ("voxel:block/oak_log_top", 9),
    ("voxel:block/oak_log", 10),
    ("voxel:block/oak_leaves", 11),
    ("voxel:block/bedrock", 12),
    ("voxel:block/coal_ore", 13),
    ("voxel:block/iron_ore", 14),
    ("voxel:block/gold_ore", 15),
    ("voxel:block/diamond_ore", 16),
    ("voxel:block/tall_grass", 17),
    ("voxel:block/red_flower", 18),
    ("voxel:block/snow", 19),
    ("voxel:block/ice", 20),
    ("voxel:block/sandstone_top", 21),
    ("voxel:block/sandstone_bottom", 22),
    ("voxel:block/sandstone_side", 23),
    ("voxel:block/stone_bricks", 24),
    ("voxel:block/mossy_cobblestone", 25),
    ("voxel:block/smooth_stone", 26),
    ("voxel:block/granite", 27),
    ("voxel:block/diorite", 28),
    ("voxel:block/andesite", 29),
    ("voxel:block/deepslate", 30),
    ("voxel:block/tuff", 31),
    ("voxel:block/birch_log_top", 32),
    ("voxel:block/birch_log", 33),
    ("voxel:block/birch_leaves", 34),
    ("voxel:block/spruce_log_top", 35),
    ("voxel:block/spruce_log", 36),
    ("voxel:block/spruce_leaves", 37),
    ("voxel:block/dark_oak_log_top", 38),
    ("voxel:block/dark_oak_log", 39),
    ("voxel:block/dark_oak_leaves", 40),
];

/// A workspace rooted at the repository that holds the block crate.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Finds the repository root by walking up from the current directory.
    pub fn locate() -> Result<Self, PipelineError> {
        let mut directory = std::env::current_dir().map_err(|source| PipelineError::Io {
            path: PathBuf::from("."),
            source,
        })?;
        loop {
            if directory
                .join("crates/engine/block/data/blocks.ron")
                .is_file()
            {
                return Ok(Self { root: directory });
            }
            if !directory.pop() {
                return Err(PipelineError::Invalid(
                    "could not find crates/engine/block/data/blocks.ron; pass --root".to_owned(),
                ));
            }
        }
    }

    /// Creates a workspace at an explicit root.
    pub fn at(root: impl Into<PathBuf>) -> Result<Self, PipelineError> {
        let root = root.into();
        if !root.join("crates/engine/block/data/blocks.ron").is_file() {
            return Err(PipelineError::Invalid(format!(
                "{} does not contain crates/engine/block/data/blocks.ron",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn block_crate(&self) -> PathBuf {
        self.root.join("crates/engine/block")
    }

    pub fn graph_dir(&self) -> PathBuf {
        self.block_crate().join("textures/graph")
    }

    pub fn texture_dir(&self) -> PathBuf {
        self.block_crate().join("textures/block")
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.block_crate().join("data/textures.ron")
    }

    pub fn materials_path(&self) -> PathBuf {
        self.block_crate().join("data/materials.gen.ron")
    }

    pub fn blocks_path(&self) -> PathBuf {
        self.block_crate().join("data/blocks.ron")
    }

    pub fn atlas_dir(&self) -> PathBuf {
        self.root.join("crates/engine/viewer/assets/atlas")
    }

    pub fn atlas_png_path(&self) -> PathBuf {
        self.atlas_dir().join("atlas.png")
    }

    pub fn atlas_hash_path(&self) -> PathBuf {
        self.atlas_dir().join("atlas.hash.ron")
    }

    /// Graph path for a key.
    pub fn graph_path(&self, key: &str) -> PathBuf {
        self.graph_dir().join(format!("{}.ron", name_of(key)))
    }

    /// Committed PNG path for a key.
    pub fn texture_path(&self, key: &str) -> PathBuf {
        self.texture_dir().join(format!("{}.png", name_of(key)))
    }

    /// All `*.ron` graph files that are not editor subgraphs.
    pub fn graph_files(&self) -> Result<Vec<PathBuf>, PipelineError> {
        let dir = self.graph_dir();
        let mut files = Vec::new();
        let entries = fs::read_dir(&dir).map_err(|source| PipelineError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| PipelineError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "ron")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('_'))
            {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }
}

/// `voxel:block/stone` -> `stone`.
pub fn name_of(key: &str) -> &str {
    key.strip_prefix("voxel:block/").unwrap_or(key)
}

/// Slot for a face key.
pub fn slot_of(key: &str) -> Option<u16> {
    SLICES
        .iter()
        .find_map(|(candidate, slot)| (*candidate == key).then_some(*slot))
}

/// A graph rendered in memory.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub key: String,
    pub slot: u16,
    pub alpha: AlphaClass,
    pub tiling: Tiling,
    pub graph_path: PathBuf,
    pub graph_hash: String,
    pub png: Vec<u8>,
    pub pixels: Vec<u8>,
}

/// Renders one graph file at full 256x256.
pub fn render_graph(path: &Path) -> Result<Artifact, PipelineError> {
    let bytes = fs::read(path).map_err(|source| PipelineError::Io {
        path: path.to_owned(),
        source,
    })?;
    let graph = Graph::load(path)?;
    let slot = slot_of(&graph.key).ok_or_else(|| {
        PipelineError::Invalid(format!(
            "{}: key {} is not in the M1.5 slice table",
            path.display(),
            graph.key
        ))
    })?;
    let program = graph.compile()?;
    if program.tiling != graph.tiling || program.alpha != graph.alpha {
        return Err(PipelineError::Invalid(format!(
            "{}: compiled tiling/alpha disagrees with the graph header",
            path.display()
        )));
    }
    let pixels = crate::eval::evaluate(&program);
    let png = png::encode_rgba(graph.size, graph.size, &pixels)?;
    Ok(Artifact {
        key: graph.key.clone(),
        slot,
        alpha: graph.alpha,
        tiling: graph.tiling,
        graph_path: path.to_owned(),
        graph_hash: blake3::hash(&bytes).to_hex().to_string(),
        png,
        pixels,
    })
}

/// Renders every committed slice graph, in slot order.
pub fn render_all(workspace: &Workspace) -> Result<Vec<Artifact>, PipelineError> {
    let files = workspace.graph_files()?;
    let mut by_key = BTreeMap::new();
    for path in files {
        let graph = Graph::load(&path)?;
        if by_key.insert(graph.key.clone(), path.clone()).is_some() {
            return Err(PipelineError::Invalid(format!(
                "{}: duplicate graph key {}",
                path.display(),
                graph.key
            )));
        }
    }

    let mut artifacts = Vec::with_capacity(SLICES.len());
    for (key, slot) in SLICES {
        let path = by_key.get(*key).ok_or_else(|| {
            PipelineError::Invalid(format!("missing graph for {key} ({}.ron)", name_of(key)))
        })?;
        let mut artifact = render_graph(path)?;
        if artifact.slot != *slot {
            return Err(PipelineError::Invalid(format!(
                "{}: slot {} does not match the slice table ({slot})",
                artifact.key, artifact.slot
            )));
        }
        artifact.slot = *slot;
        artifacts.push(artifact);
    }
    if by_key.len() != SLICES.len() {
        let extra: Vec<&String> = by_key.keys().filter(|key| slot_of(key).is_none()).collect();
        return Err(PipelineError::Invalid(format!(
            "graph directory has {} graphs for {} slice faces; extra: {extra:?}",
            by_key.len(),
            SLICES.len()
        )));
    }
    Ok(artifacts)
}

/// Builds the manifest value for a set of artifacts.
pub fn manifest_from(artifacts: &[Artifact]) -> TextureManifest {
    let mut textures: Vec<TextureEntry> = artifacts
        .iter()
        .map(|artifact| TextureEntry {
            key: artifact.key.clone(),
            slot: artifact.slot,
            source: PathBuf::from(format!("textures/block/{}.png", name_of(&artifact.key))),
            alpha: artifact.alpha,
            tiling: artifact.tiling,
            hash: blake3::hash(&artifact.png).to_hex().to_string(),
            graph: Some(PathBuf::from(format!(
                "textures/graph/{}.ron",
                name_of(&artifact.key)
            ))),
            graph_hash: Some(artifact.graph_hash.clone()),
            placeholder: false,
        })
        .collect();
    textures.sort_by_key(|entry| entry.slot);
    TextureManifest {
        schema: 1,
        license: "CC0-1.0".to_owned(),
        provenance:
            "Original artwork procedurally generated for Voxel-Stuff by texgen (graphs in textures/graph/)"
                .to_owned(),
        textures,
    }
}

/// Writes every generated PNG, then the manifest and material table.
pub fn generate_all(workspace: &Workspace) -> Result<String, PipelineError> {
    let artifacts = render_all(workspace)?;
    let texture_dir = workspace.texture_dir();
    fs::create_dir_all(&texture_dir).map_err(|source| PipelineError::Io {
        path: texture_dir.clone(),
        source,
    })?;
    for artifact in &artifacts {
        write_atomic(&workspace.texture_path(&artifact.key), &artifact.png)?;
    }

    let manifest = manifest_from(&artifacts);
    let mut text =
        ron::ser::to_string_pretty(&manifest, pretty_config()).map_err(PipelineError::Ron)?;
    text.push('\n');
    write_atomic(&workspace.manifest_path(), text.as_bytes())?;
    validate_manifest(&manifest, &workspace.block_crate()).map_err(PipelineError::Manifest)?;

    let materials = write_materials(workspace)?;
    Ok(format!(
        "generated {} textures, {} ({materials})",
        artifacts.len(),
        workspace.manifest_path().display()
    ))
}

/// Re-renders in memory and fails if any committed byte differs.
pub fn check_all(workspace: &Workspace) -> Result<String, PipelineError> {
    let artifacts = render_all(workspace)?;
    for artifact in &artifacts {
        let path = workspace.texture_path(&artifact.key);
        let committed = fs::read(&path).map_err(|source| PipelineError::Io {
            path: path.clone(),
            source,
        })?;
        if committed != artifact.png {
            return Err(PipelineError::Invalid(format!(
                "{} differs from the graph output",
                path.display()
            )));
        }
    }

    let manifest = TextureManifest::load(&workspace.manifest_path())?;
    validate_manifest(&manifest, &workspace.block_crate()).map_err(PipelineError::Manifest)?;
    for artifact in &artifacts {
        let entry = manifest.entry(&artifact.key).ok_or_else(|| {
            PipelineError::Invalid(format!("manifest is missing {}", artifact.key))
        })?;
        let png_hash = blake3::hash(&artifact.png).to_hex().to_string();
        if entry.hash != png_hash
            || entry.graph_hash.as_deref() != Some(artifact.graph_hash.as_str())
            || entry.slot != artifact.slot
            || entry.alpha != artifact.alpha
            || entry.tiling != artifact.tiling
        {
            return Err(PipelineError::Invalid(format!(
                "{}: manifest entry is stale; run update-manifest",
                artifact.key
            )));
        }
    }

    check_materials(workspace)?;
    bake::bake_atlas(workspace, true)?;
    Ok(format!(
        "{} textures, manifest, materials and atlas are current",
        artifacts.len()
    ))
}

// ---------------------------------------------------------------------------
// Material table (M1.5-08)
// ---------------------------------------------------------------------------

pub use voxel_block::material_table::MaterialTable;

/// Builds the material table in memory.
pub fn material_table(workspace: &Workspace) -> Result<MaterialTable, PipelineError> {
    voxel_block::material_table::generate(&workspace.block_crate()).map_err(PipelineError::Material)
}

/// Writes `materials.gen.ron` and returns a status line.
pub fn write_materials(workspace: &Workspace) -> Result<String, PipelineError> {
    let table = material_table(workspace)?;
    let text = voxel_block::material_table::to_ron(&table).map_err(PipelineError::Material)?;
    let path = workspace.materials_path();
    write_atomic(&path, text.as_bytes())?;
    Ok(format!(
        "{} materials, {} block face rows -> {}",
        table.materials.len(),
        table.faces.len(),
        path.display()
    ))
}

/// Fails if `materials.gen.ron` is not exactly the regenerated output.
pub fn check_materials(workspace: &Workspace) -> Result<(), PipelineError> {
    let table = material_table(workspace)?;
    let text = voxel_block::material_table::to_ron(&table).map_err(PipelineError::Material)?;
    let path = workspace.materials_path();
    let committed = fs::read_to_string(&path).map_err(|source| PipelineError::Io {
        path: path.clone(),
        source,
    })?;
    if committed != text {
        return Err(PipelineError::Invalid(format!(
            "{} is stale; run update-manifest",
            path.display()
        )));
    }
    Ok(())
}

/// Full pipeline: textures, manifest, materials, atlas.
pub fn update_all(workspace: &Workspace) -> Result<String, PipelineError> {
    let textures = generate_all(workspace)?;
    let atlas = bake::bake_atlas(workspace, false)?;
    Ok(format!("{textures}; {atlas}"))
}

/// Writes bytes via a temp file in the same directory, then renames.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PipelineError> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| PipelineError::Io {
        path: parent.to_owned(),
        source,
    })?;
    // A per-write suffix keeps two writers (threads or two workspaces sharing a
    // directory) from clobbering each other's temp file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.tmp-{}-{unique}",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut file = fs::File::create(&temp).map_err(|source| PipelineError::Io {
        path: temp.clone(),
        source,
    })?;
    file.write_all(bytes).map_err(|source| PipelineError::Io {
        path: temp.clone(),
        source,
    })?;
    // Without this a crash between write and rename can leave a zero-length
    // file at `path`.
    file.sync_all().map_err(|source| PipelineError::Io {
        path: temp.clone(),
        source,
    })?;
    drop(file);
    fs::rename(&temp, path).map_err(|source| PipelineError::Io {
        path: path.to_owned(),
        source,
    })
}

/// A pipeline failure.
#[derive(Debug)]
pub enum PipelineError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Graph(GraphError),
    Png(PngError),
    Manifest(voxel_block::texture_manifest::ManifestError),
    Bake(BakeError),
    Material(voxel_block::material_table::MaterialTableError),
    Ron(ron::error::Error),
    Invalid(String),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Graph(error) => error.fmt(f),
            Self::Png(error) => error.fmt(f),
            Self::Manifest(error) => error.fmt(f),
            Self::Bake(error) => error.fmt(f),
            Self::Material(error) => error.fmt(f),
            Self::Ron(error) => write!(f, "ron: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for PipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Graph(error) => Some(error),
            Self::Png(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Bake(error) => Some(error),
            Self::Material(error) => Some(error),
            Self::Ron(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<GraphError> for PipelineError {
    fn from(error: GraphError) -> Self {
        Self::Graph(error)
    }
}

impl From<PngError> for PipelineError {
    fn from(error: PngError) -> Self {
        Self::Png(error)
    }
}

impl From<voxel_block::texture_manifest::ManifestError> for PipelineError {
    fn from(error: voxel_block::texture_manifest::ManifestError) -> Self {
        Self::Manifest(error)
    }
}

impl From<BakeError> for PipelineError {
    fn from(error: BakeError) -> Self {
        Self::Bake(error)
    }
}

impl From<voxel_block::material_table::MaterialTableError> for PipelineError {
    fn from(error: voxel_block::material_table::MaterialTableError) -> Self {
        Self::Material(error)
    }
}

impl From<ron::error::Error> for PipelineError {
    fn from(error: ron::error::Error) -> Self {
        Self::Ron(error)
    }
}
