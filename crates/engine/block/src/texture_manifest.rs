//! Material texture manifest (`data/textures.ron`) and its validation.
//!
//! The manifest is the single source of truth for *which* source face backs
//! which atlas slot. Every entry names a 256x256 RGBA8 PNG, its alpha class,
//! its tiling requirement, and the BLAKE3 hashes of both the PNG and the node
//! graph that generated it.
//!
//! Validation lives here rather than in the generator or the bake tool so the
//! committed files are checked by the same code that reads them. See
//! `docs/generator.spec.md` section 4.3 for the list this module enforces.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::properties::RenderType;
use crate::ron::RonBlockList;

/// Manifest path, relative to the block crate root.
pub const MANIFEST_PATH: &str = "data/textures.ron";
/// Block definitions path, relative to the block crate root.
pub const BLOCKS_PATH: &str = "data/blocks.ron";
/// Source face edge in texels. The atlas tile is 16x16, so the box filter is
/// exactly 16x16.
pub const SOURCE_EDGE: u32 = 256;
/// Bytes in one decoded source face.
pub const SOURCE_BYTES: usize = (SOURCE_EDGE * SOURCE_EDGE * 4) as usize;
/// Number of faces in the M1.5 vertical slice.
pub const SLICE_FACE_COUNT: usize = 23;

/// Alpha class of a source face, as locked by decision D1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaClass {
    Opaque,
    Cutout,
    Translucent,
}

/// Tiling requirement of a source face (section 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tiling {
    /// Wraps left-right and top-bottom.
    Both,
    /// Wraps left-right only.
    Horizontal,
    /// No wrap required; content keeps at least 8 texels of side margin.
    None,
}

/// The committed texture manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureManifest {
    pub schema: u32,
    pub license: String,
    pub provenance: String,
    pub textures: Vec<TextureEntry>,
}

/// One source face in the manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureEntry {
    pub key: String,
    pub slot: u16,
    pub source: PathBuf,
    pub alpha: AlphaClass,
    pub tiling: Tiling,
    pub hash: String,
    pub graph: Option<PathBuf>,
    pub graph_hash: Option<String>,
    pub placeholder: bool,
}

/// A decoded source face.
#[derive(Clone, Debug)]
pub struct SourceImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// A manifest failure. Every variant names what failed so a CI log points at
/// the offending file or entry without a second run.
#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse(ron::error::SpannedError),
    BlockParse(ron::error::SpannedError),
    Decode {
        path: PathBuf,
        source: png::DecodingError,
    },
    Invalid(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse(error) => write!(f, "textures.ron: {error}"),
            Self::BlockParse(error) => write!(f, "blocks.ron: {error}"),
            Self::Decode { path, source } => {
                write!(f, "could not decode {}: {source}", path.display())
            }
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(error) | Self::BlockParse(error) => Some(error),
            Self::Decode { source, .. } => Some(source),
            Self::Invalid(_) => None,
        }
    }
}

impl TextureManifest {
    /// Reads and parses a manifest without validating its sources.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_owned(),
            source,
        })?;
        ron::from_str(&text).map_err(ManifestError::Parse)
    }

    /// The entry for a texture key.
    pub fn entry(&self, key: &str) -> Option<&TextureEntry> {
        self.textures.iter().find(|entry| entry.key == key)
    }

    /// Entries in slot order.
    pub fn sorted(&self) -> Vec<&TextureEntry> {
        let mut entries: Vec<&TextureEntry> = self.textures.iter().collect();
        entries.sort_by_key(|entry| entry.slot);
        entries
    }
}

/// Resolves a manifest-relative path against the block crate root.
///
/// Rejects absolute paths and `..`, so a manifest cannot point outside the
/// crate even if a hostile or hand-edited entry tries to.
pub fn resolve_inside(crate_root: &Path, relative: &Path) -> Result<PathBuf, ManifestError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(ManifestError::Invalid(format!(
            "{} must be relative to the block crate and may not use `..`",
            relative.display()
        )));
    }
    Ok(crate_root.join(relative))
}

/// Reads a source PNG and checks its size, colour type and hash.
pub fn load_source(crate_root: &Path, entry: &TextureEntry) -> Result<SourceImage, ManifestError> {
    let path = resolve_inside(crate_root, &entry.source)?;
    let bytes = fs::read(&path).map_err(|source| ManifestError::Io {
        path: path.clone(),
        source,
    })?;
    if entry.hash.len() != 64 || !entry.hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::Invalid(format!(
            "{}: hash must be 64 lowercase hex characters",
            entry.key
        )));
    }
    let actual = blake3::hash(&bytes).to_hex().to_string();
    if actual != entry.hash {
        return Err(ManifestError::Invalid(format!(
            "{}: hash mismatch (manifest {}, file {actual})",
            entry.key, entry.hash
        )));
    }
    decode_source(&path, &bytes)
}

/// Decodes a 256x256 RGBA8 source PNG.
pub fn decode_source(path: &Path, bytes: &[u8]) -> Result<SourceImage, ManifestError> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .map_err(|source| ManifestError::Decode {
            path: path.to_owned(),
            source,
        })?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|source| ManifestError::Decode {
            path: path.to_owned(),
            source,
        })?;
    if info.width != SOURCE_EDGE
        || info.height != SOURCE_EDGE
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(ManifestError::Invalid(format!(
            "{} is {}x{} {:?} {:?}; expected {SOURCE_EDGE}x{SOURCE_EDGE} RGBA8 colour type 6",
            path.display(),
            info.width,
            info.height,
            info.color_type,
            info.bit_depth
        )));
    }
    pixels.truncate(SOURCE_BYTES);
    Ok(SourceImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// The alpha class a render type requires.
pub const fn alpha_class(render_type: RenderType) -> Option<AlphaClass> {
    match render_type {
        RenderType::Solid => Some(AlphaClass::Opaque),
        RenderType::Cutout => Some(AlphaClass::Cutout),
        RenderType::Translucent => Some(AlphaClass::Translucent),
        RenderType::Invisible => None,
    }
}

/// Distinct face keys referenced by `blocks.ron`, with the render type every
/// user agrees on.
pub fn block_face_classes(blocks_ron: &str) -> Result<BTreeMap<String, RenderType>, ManifestError> {
    let list: RonBlockList = ron::from_str(blocks_ron).map_err(ManifestError::BlockParse)?;
    let mut classes = BTreeMap::new();
    for block in list.blocks {
        let Some(textures) = block.textures else {
            continue;
        };
        for key in [
            textures.top,
            textures.bottom,
            textures.north,
            textures.south,
            textures.east,
            textures.west,
        ] {
            match classes.entry(key) {
                std::collections::btree_map::Entry::Occupied(existing) => {
                    if *existing.get() != block.render_type {
                        return Err(ManifestError::Invalid(format!(
                            "RenderTypeConflict: {} is used as both {:?} and {:?}",
                            existing.key(),
                            existing.get(),
                            block.render_type
                        )));
                    }
                }
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(block.render_type);
                }
            }
        }
    }
    Ok(classes)
}

/// Validates a manifest against the block crate.
///
/// This is the implementation behind `cargo test -p voxel-block
/// texture_manifest`; the bake tool and the generator call the same function
/// before writing anything.
pub fn validate(manifest: &TextureManifest, crate_root: &Path) -> Result<(), ManifestError> {
    if manifest.schema != 1 {
        return Err(ManifestError::Invalid(format!(
            "manifest schema {} is not supported (expected 1)",
            manifest.schema
        )));
    }
    if manifest.license.trim().is_empty() || manifest.provenance.trim().is_empty() {
        return Err(ManifestError::Invalid(
            "license and provenance must not be empty".to_owned(),
        ));
    }

    let blocks_path = crate_root.join(BLOCKS_PATH);
    let blocks = fs::read_to_string(&blocks_path).map_err(|source| ManifestError::Io {
        path: blocks_path,
        source,
    })?;
    let face_classes = block_face_classes(&blocks)?;

    let mut keys = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for entry in &manifest.textures {
        if !keys.insert(entry.key.clone()) {
            return Err(ManifestError::Invalid(format!(
                "duplicate texture key {}",
                entry.key
            )));
        }
        if entry.slot == 0 || entry.slot as usize > u8::MAX as usize {
            return Err(ManifestError::Invalid(format!(
                "{}: slot {} is outside 1..=255",
                entry.key, entry.slot
            )));
        }
        if !slots.insert(entry.slot) {
            return Err(ManifestError::Invalid(format!(
                "duplicate slot {}",
                entry.slot
            )));
        }
    }

    if manifest.textures.len() != face_classes.len() {
        return Err(ManifestError::Invalid(format!(
            "manifest has {} textures but blocks.ron references {}",
            manifest.textures.len(),
            face_classes.len()
        )));
    }
    if manifest.textures.len() != SLICE_FACE_COUNT {
        return Err(ManifestError::Invalid(format!(
            "manifest has {} textures; the M1.5 slice is exactly {SLICE_FACE_COUNT}",
            manifest.textures.len()
        )));
    }
    for (expected, slot) in (1u16..).zip(slots.iter()) {
        if *slot != expected {
            return Err(ManifestError::Invalid(format!(
                "slots must be exactly 1..={}; found {slot} where {expected} was expected",
                manifest.textures.len()
            )));
        }
    }
    for key in face_classes.keys() {
        if !keys.contains(key) {
            return Err(ManifestError::Invalid(format!(
                "MissingManifestEntry: {key}"
            )));
        }
    }
    for key in &keys {
        if !face_classes.contains_key(key) {
            return Err(ManifestError::Invalid(format!(
                "UnusedManifestEntry: {key}"
            )));
        }
    }

    for entry in &manifest.textures {
        let Some(render_type) = face_classes.get(&entry.key) else {
            continue;
        };
        let required = alpha_class(*render_type).ok_or_else(|| {
            ManifestError::Invalid(format!(
                "{}: Invisible blocks must not reference a texture",
                entry.key
            ))
        })?;
        if required != entry.alpha {
            return Err(ManifestError::Invalid(format!(
                "{}: alpha {:?} does not match its RenderType {:?}",
                entry.key, entry.alpha, render_type
            )));
        }
        if entry.placeholder {
            return Err(ManifestError::Invalid(format!(
                "{}: placeholder entries fail the final manifest",
                entry.key
            )));
        }

        let source = load_source(crate_root, entry)?;
        check_alpha(entry, &source)?;
        check_tiling(entry, &source)?;

        match (&entry.graph, &entry.graph_hash) {
            (Some(graph), Some(hash)) => {
                let path = resolve_inside(crate_root, graph)?;
                let bytes = fs::read(&path).map_err(|source| ManifestError::Io {
                    path: path.clone(),
                    source,
                })?;
                let actual = blake3::hash(&bytes).to_hex().to_string();
                if actual != *hash {
                    return Err(ManifestError::Invalid(format!(
                        "{}: graph hash mismatch (manifest {hash}, file {actual})",
                        entry.key
                    )));
                }
            }
            (None, None) => {}
            _ => {
                return Err(ManifestError::Invalid(format!(
                    "{}: graph and graph_hash must both be set or both be None",
                    entry.key
                )));
            }
        }
    }

    check_sandstone(manifest, crate_root)
}

fn check_alpha(entry: &TextureEntry, image: &SourceImage) -> Result<(), ManifestError> {
    let mut zero = false;
    let mut full = false;
    let mut intermediate = false;
    for texel in image.pixels.as_chunks::<4>().0 {
        match texel[3] {
            0 => zero = true,
            255 => full = true,
            _ => intermediate = true,
        }
    }
    let ok = match entry.alpha {
        AlphaClass::Opaque => full && !zero && !intermediate,
        AlphaClass::Cutout => zero && full && !intermediate,
        AlphaClass::Translucent => intermediate,
    };
    if ok {
        Ok(())
    } else {
        Err(ManifestError::Invalid(format!(
            "{}: alpha pixels violate {:?} (zero={zero}, full={full}, intermediate={intermediate})",
            entry.key, entry.alpha
        )))
    }
}

/// Section 4.5: the wrap seam must not be worse than 1.5x an interior step.
fn check_tiling(entry: &TextureEntry, image: &SourceImage) -> Result<(), ManifestError> {
    let edge = image.width as usize;
    if matches!(entry.tiling, Tiling::Both | Tiling::Horizontal) {
        let seam = column_diff(image, 0, edge - 1) / edge as u64;
        let interior = adjacent_column_diff(image);
        if seam > interior.saturating_mul(3) / 2 {
            return Err(ManifestError::Invalid(format!(
                "{}: horizontal seam {seam} exceeds 1.5x interior {interior}",
                entry.key
            )));
        }
    }
    if entry.tiling == Tiling::Both {
        let seam = row_diff(image, 0, edge - 1) / edge as u64;
        let interior = adjacent_row_diff(image);
        if seam > interior.saturating_mul(3) / 2 {
            return Err(ManifestError::Invalid(format!(
                "{}: vertical seam {seam} exceeds 1.5x interior {interior}",
                entry.key
            )));
        }
    }
    Ok(())
}

/// Sum of absolute RGBA differences between two columns. The caller divides
/// once, so adjacent pairs are not individually truncated.
fn column_diff(image: &SourceImage, left: usize, right: usize) -> u64 {
    let edge = image.width as usize;
    let mut total = 0u64;
    for y in 0..edge {
        let a = (y * edge + left) * 4;
        let b = (y * edge + right) * 4;
        for channel in 0..4 {
            total += image.pixels[a + channel].abs_diff(image.pixels[b + channel]) as u64;
        }
    }
    total
}

fn adjacent_column_diff(image: &SourceImage) -> u64 {
    let edge = image.width as usize;
    let mut total = 0u64;
    for x in 0..edge - 1 {
        total += column_diff(image, x, x + 1);
    }
    total / ((edge - 1) * edge) as u64
}

fn row_diff(image: &SourceImage, top: usize, bottom: usize) -> u64 {
    let edge = image.width as usize;
    let mut total = 0u64;
    for x in 0..edge {
        let a = (top * edge + x) * 4;
        let b = (bottom * edge + x) * 4;
        for channel in 0..4 {
            total += image.pixels[a + channel].abs_diff(image.pixels[b + channel]) as u64;
        }
    }
    total
}

fn adjacent_row_diff(image: &SourceImage) -> u64 {
    let edge = image.width as usize;
    let mut total = 0u64;
    for y in 0..edge - 1 {
        total += row_diff(image, y, y + 1);
    }
    total / ((edge - 1) * edge) as u64
}

/// Checks that the sandstone side's top and bottom bands agree with the cap
/// faces (section 4.6).
fn check_sandstone(manifest: &TextureManifest, crate_root: &Path) -> Result<(), ManifestError> {
    let side = manifest.entry("voxel:block/sandstone_side");
    let top = manifest.entry("voxel:block/sandstone_top");
    let bottom = manifest.entry("voxel:block/sandstone_bottom");
    let (Some(side), Some(top), Some(bottom)) = (side, top, bottom) else {
        return Ok(());
    };

    let side = load_source(crate_root, side)?;
    let top = load_source(crate_root, top)?;
    let bottom = load_source(crate_root, bottom)?;
    let top_mean = mean_color(&top);
    let bottom_mean = mean_color(&bottom);
    let side_top = band_mean(&side, 0, 16);
    let side_bottom = band_mean(&side, 240, 256);

    for (channel, (band, mean)) in side_top.iter().zip(top_mean.iter()).enumerate() {
        if band.abs_diff(*mean) > 6 {
            return Err(ManifestError::Invalid(format!(
                "sandstone_faces_agree: side top band channel {channel} differs by {} from sandstone_top",
                band.abs_diff(*mean)
            )));
        }
    }
    for (channel, (band, mean)) in side_bottom.iter().zip(bottom_mean.iter()).enumerate() {
        if band.abs_diff(*mean) > 6 {
            return Err(ManifestError::Invalid(format!(
                "sandstone_faces_agree: side bottom band channel {channel} differs by {} from sandstone_bottom",
                band.abs_diff(*mean)
            )));
        }
    }
    Ok(())
}

fn mean_color(image: &SourceImage) -> [u8; 4] {
    let mut sums = [0u64; 4];
    let count = (image.width * image.height) as u64;
    for texel in image.pixels.as_chunks::<4>().0 {
        for (sum, value) in sums.iter_mut().zip(texel) {
            *sum += *value as u64;
        }
    }
    [
        (sums[0] / count) as u8,
        (sums[1] / count) as u8,
        (sums[2] / count) as u8,
        (sums[3] / count) as u8,
    ]
}

fn band_mean(image: &SourceImage, first_row: u32, end_row: u32) -> [u8; 4] {
    let edge = image.width as usize;
    let mut sums = [0u64; 4];
    let mut count = 0u64;
    for y in first_row..end_row {
        for x in 0..edge {
            let offset = (y as usize * edge + x) * 4;
            for (sum, value) in sums.iter_mut().zip(&image.pixels[offset..offset + 4]) {
                *sum += *value as u64;
            }
            count += 1;
        }
    }
    [
        (sums[0] / count) as u8,
        (sums[1] / count) as u8,
        (sums[2] / count) as u8,
        (sums[3] / count) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_types_map_to_alpha_classes() {
        assert_eq!(alpha_class(RenderType::Solid), Some(AlphaClass::Opaque));
        assert_eq!(alpha_class(RenderType::Cutout), Some(AlphaClass::Cutout));
        assert_eq!(
            alpha_class(RenderType::Translucent),
            Some(AlphaClass::Translucent)
        );
        assert_eq!(alpha_class(RenderType::Invisible), None);
    }

    #[test]
    fn parent_paths_are_rejected() {
        let root = Path::new("/tmp/crate");
        assert!(resolve_inside(root, Path::new("../outside.png")).is_err());
        assert!(resolve_inside(root, Path::new("/etc/passwd")).is_err());
        assert!(resolve_inside(root, Path::new("textures/block/stone.png")).is_ok());
    }
}
