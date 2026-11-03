//! The generation pipeline: graphs -> PNGs -> manifest -> materials -> atlas.
//!
//! Every entry point in this module is deterministic and rooted at a
//! [`Workspace`], the directory that contains `crates/engine/block`. The GUI
//! and the CLI share exactly these functions, so a button press and a CI check
//! cannot disagree about what "generate" means.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use voxel_block::texture_manifest::{
    AlphaClass, FaceTable, TextureEntry, TextureManifest, Tiling, validate as validate_manifest,
};

use crate::bake::{self, BakeError};
use crate::graph::{Graph, GraphError, pretty_config};
use crate::png::{self, PngError};

/// A workspace rooted at the repository that holds the block crate.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    faces: FaceTable,
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
                return Self::at(directory);
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
        let block_crate = root.join("crates/engine/block");
        if !block_crate.join("data/blocks.ron").is_file() {
            return Err(PipelineError::Invalid(format!(
                "{} does not contain crates/engine/block/data/blocks.ron",
                root.display()
            )));
        }
        let faces = FaceTable::load(&block_crate).map_err(PipelineError::Manifest)?;
        Ok(Self { root, faces })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The committed face table, in atlas slot order.
    pub fn faces(&self) -> &FaceTable {
        &self.faces
    }

    /// Slot for a face key, or `None` when `data/faces.ron` does not list it.
    pub fn slot_of(&self, key: &str) -> Option<u16> {
        self.faces.slot_of(key)
    }

    /// Resolves `stone`, `voxel:stone` or `voxel:block/stone` to a table key.
    pub fn key_from(&self, input: &str) -> Result<String, PipelineError> {
        let key = if input.starts_with("voxel:block/") {
            input.to_owned()
        } else {
            format!(
                "voxel:block/{}",
                input.trim_start_matches("voxel:").trim_start_matches('/')
            )
        };
        if self.slot_of(&key).is_some() {
            Ok(key)
        } else {
            Err(PipelineError::Invalid(format!(
                "unknown face `{input}`; expected one of: {}",
                self.faces
                    .iter()
                    .map(|(key, _)| name_of(key))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
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
        self.root.join("apps/viewer/assets/atlas")
    }

    pub fn atlas_png_path(&self) -> PathBuf {
        self.atlas_dir().join("atlas.png")
    }

    pub fn atlas_hash_path(&self) -> PathBuf {
        self.atlas_dir().join("atlas.hash.ron")
    }

    /// Graph path for a key.
    pub fn graph_path(&self, key: &str) -> PathBuf {
        self.graph_dir().join(format!("{}.ron", path_name(key)))
    }

    /// Committed PNG path for a key.
    pub fn texture_path(&self, key: &str) -> PathBuf {
        self.texture_dir().join(format!("{}.png", path_name(key)))
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

/// The file-name component of a key.
///
/// Committed keys are already checked by `normalize_key`, but the editor also
/// mints `voxel:block/_subgraph/{name}` keys for scratch graphs, so this must
/// be total rather than an assertion in a GUI-reachable path. Anything outside
/// the allowlist becomes `_`, which also removes separators and `..`.
fn path_name(key: &str) -> String {
    let name: String = name_of(key)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "_".to_owned()
    } else {
        name
    }
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

/// Renders one graph file at full 256x256, taking its slot from the face table.
pub fn render_graph(workspace: &Workspace, path: &Path) -> Result<Artifact, PipelineError> {
    let bytes = fs::read(path).map_err(|source| PipelineError::Io {
        path: path.to_owned(),
        source,
    })?;
    let graph = Graph::load(path)?;
    let slot = workspace.slot_of(&graph.key).ok_or_else(|| {
        PipelineError::Invalid(format!(
            "{}: key {} is not listed in {}",
            path.display(),
            graph.key,
            voxel_block::texture_manifest::FACES_PATH
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

/// Renders every committed face graph, in slot order.
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

    let faces = workspace.faces();
    let mut artifacts = Vec::with_capacity(faces.len());
    for (key, _) in faces.iter() {
        let path = by_key.get(key).ok_or_else(|| {
            PipelineError::Invalid(format!(
                "missing graph for {key} ({}.ron is listed in {}, but the graph file is absent)",
                name_of(key),
                voxel_block::texture_manifest::FACES_PATH
            ))
        })?;
        artifacts.push(render_graph(workspace, path)?);
    }
    if by_key.len() != faces.len() {
        let extra: Vec<&String> = by_key
            .keys()
            .filter(|key| faces.slot_of(key).is_none())
            .collect();
        return Err(PipelineError::Invalid(format!(
            "graph directory has {} graphs for {} faces; extra: {extra:?}",
            by_key.len(),
            faces.len()
        )));
    }
    Ok(artifacts)
}

/// The manifest entry for one rendered artifact.
fn entry_from(artifact: &Artifact) -> TextureEntry {
    TextureEntry {
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
    }
}

/// Builds the manifest value for a set of artifacts.
pub fn manifest_from(artifacts: &[Artifact]) -> TextureManifest {
    let mut textures: Vec<TextureEntry> = artifacts.iter().map(entry_from).collect();
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

/// Resolves `--key` arguments to table keys, in slot order and without repeats.
fn resolve_selection(
    workspace: &Workspace,
    inputs: &[String],
) -> Result<Vec<String>, PipelineError> {
    if inputs.is_empty() {
        return Err(PipelineError::Invalid(
            "--key needs at least one face name".to_owned(),
        ));
    }
    let mut wanted = BTreeSet::new();
    for input in inputs {
        wanted.insert(workspace.key_from(input)?);
    }
    Ok(workspace
        .faces()
        .iter()
        .map(|(key, _)| key.to_owned())
        .filter(|key| wanted.contains(key))
        .collect())
}

/// Renders the selected faces only; every graph must exist.
fn render_selection(
    workspace: &Workspace,
    selected: &[String],
) -> Result<Vec<Artifact>, PipelineError> {
    let mut artifacts = Vec::with_capacity(selected.len());
    for key in selected {
        let path = workspace.graph_path(key);
        if !path.is_file() {
            return Err(PipelineError::Invalid(format!(
                "missing graph for {key} ({}); add the graph before listing the face in {}",
                path.display(),
                voxel_block::texture_manifest::FACES_PATH
            )));
        }
        artifacts.push(render_graph(workspace, &path)?);
    }
    Ok(artifacts)
}

fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>, PipelineError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PipelineError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Renders and writes only `inputs`, leaving every other face untouched.
///
/// A PNG is rewritten only when its bytes change, and the manifest and material
/// table likewise. The atlas is rebaked only when the manifest changes, because
/// `atlas.hash.ron` embeds the manifest hash: a graph edit that keeps the same
/// pixels rewrites the atlas hash but leaves `atlas.png` byte-identical.
pub fn generate_keys(workspace: &Workspace, inputs: &[String]) -> Result<String, PipelineError> {
    let selected = resolve_selection(workspace, inputs)?;
    let artifacts = render_selection(workspace, &selected)?;
    let committed = TextureManifest::load(&workspace.manifest_path())?;

    let texture_dir = workspace.texture_dir();
    fs::create_dir_all(&texture_dir).map_err(|source| PipelineError::Io {
        path: texture_dir.clone(),
        source,
    })?;
    let mut written = 0usize;
    for artifact in &artifacts {
        let path = workspace.texture_path(&artifact.key);
        let changed = read_if_exists(&path)?.as_deref() != Some(artifact.png.as_slice());
        if changed {
            write_atomic(&path, &artifact.png)?;
            written += 1;
        }
    }

    // Replace only the selected faces' manifest entries; every other entry is
    // kept as committed.
    let new_entries: Vec<TextureEntry> = artifacts.iter().map(entry_from).collect();
    let selected_keys: BTreeSet<String> =
        new_entries.iter().map(|entry| entry.key.clone()).collect();
    let mut manifest = committed;
    manifest
        .textures
        .retain(|entry| !selected_keys.contains(&entry.key));
    manifest.textures.extend(new_entries);
    manifest.textures.sort_by_key(|entry| entry.slot);
    let mut text =
        ron::ser::to_string_pretty(&manifest, pretty_config()).map_err(PipelineError::Ron)?;
    text.push('\n');
    let manifest_path = workspace.manifest_path();
    let manifest_changed = read_if_exists(&manifest_path)?.as_deref() != Some(text.as_bytes());
    if manifest_changed {
        write_atomic(&manifest_path, text.as_bytes())?;
    }
    validate_manifest(&manifest, &workspace.block_crate()).map_err(PipelineError::Manifest)?;

    let materials_changed = write_materials_if_changed(workspace)?;
    // `atlas.hash.ron` embeds the manifest hash, so any manifest change needs a
    // rebake even when the pixels are identical. The bake is cheap next to the
    // graph evaluation, which is already limited to the selected faces.
    let atlas_changed = if written > 0 || manifest_changed {
        bake::bake_atlas(workspace, false)?;
        true
    } else {
        false
    };

    Ok(incremental_status(
        artifacts.len(),
        written,
        manifest_changed,
        materials_changed,
        atlas_changed,
    ))
}

fn incremental_status(
    faces: usize,
    written: usize,
    manifest: bool,
    materials: bool,
    atlas: bool,
) -> String {
    let plural = if faces == 1 { "" } else { "s" };
    let head = if written == 0 {
        format!("{faces} texture{plural} already current")
    } else {
        format!("{faces} texture{plural} written ({written} changed)")
    };
    format!(
        "{head}, manifest {}, materials {}, atlas {}",
        if manifest { "updated" } else { "unchanged" },
        if materials { "updated" } else { "unchanged" },
        if atlas { "rebuilt" } else { "unchanged" }
    )
}

/// Fails if a PNG or manifest entry disagrees with the re-rendered artifacts.
fn check_artifacts(
    workspace: &Workspace,
    manifest: &TextureManifest,
    artifacts: &[Artifact],
) -> Result<(), PipelineError> {
    for artifact in artifacts {
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

    for artifact in artifacts {
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
    Ok(())
}

/// Re-renders in memory and fails if any committed byte differs.
pub fn check_all(workspace: &Workspace) -> Result<String, PipelineError> {
    let artifacts = render_all(workspace)?;
    let manifest = TextureManifest::load(&workspace.manifest_path())?;
    validate_manifest(&manifest, &workspace.block_crate()).map_err(PipelineError::Manifest)?;
    check_artifacts(workspace, &manifest, &artifacts)?;

    check_materials(workspace)?;
    bake::bake_atlas(workspace, true)?;
    Ok(format!(
        "{} textures, manifest, materials and atlas are current",
        artifacts.len()
    ))
}

/// `--check --key`: verifies only the named faces, plus the shared outputs.
pub fn check_keys(workspace: &Workspace, inputs: &[String]) -> Result<String, PipelineError> {
    let selected = resolve_selection(workspace, inputs)?;
    let artifacts = render_selection(workspace, &selected)?;
    let manifest = TextureManifest::load(&workspace.manifest_path())?;
    validate_manifest(&manifest, &workspace.block_crate()).map_err(PipelineError::Manifest)?;
    check_artifacts(workspace, &manifest, &artifacts)?;

    check_materials(workspace)?;
    bake::bake_atlas(workspace, true)?;
    let plural = if artifacts.len() == 1 { "" } else { "s" };
    Ok(format!(
        "{} texture{plural}, materials and atlas are current",
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

/// Writes `materials.gen.ron` only when its bytes change; returns whether it did.
pub fn write_materials_if_changed(workspace: &Workspace) -> Result<bool, PipelineError> {
    let table = material_table(workspace)?;
    let text = voxel_block::material_table::to_ron(&table).map_err(PipelineError::Material)?;
    let path = workspace.materials_path();
    if read_if_exists(&path)?.as_deref() == Some(text.as_bytes()) {
        return Ok(false);
    }
    write_atomic(&path, text.as_bytes())?;
    Ok(true)
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

    // `"foo.ron".parent()` is `Some("")`; opening the empty path for the
    // directory fsync then fails after the rename has already succeeded.
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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
    let result = (|| -> Result<(), PipelineError> {
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
        })?;
        // The rename must itself be durable: without a directory fsync a crash
        // can resurrect the old file (or lose the new one) even though the
        // bytes were synced.
        #[cfg(unix)]
        {
            fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|source| PipelineError::Io {
                    path: parent.to_owned(),
                    source,
                })?;
        }
        Ok(())
    })();
    if result.is_err() {
        // A failed write must not leave `.{name}.tmp-{pid}-{n}` behind.
        let _ = fs::remove_file(&temp);
    }
    result
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
