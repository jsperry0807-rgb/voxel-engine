//! `bake-atlas`: downsample source faces and pack the material atlas.
//!
//! The manifest is the source of truth for key, slot, source, alpha mode and
//! hash. This tool validates it, box-filters each 256×256 source to a 16×16
//! tile, builds the five atlas mip levels with the viewer's [`AtlasBuilder`],
//! and writes one PNG per level plus a BLAKE3 hash of the level data.
//!
//! Entries with an empty hash are accepted only with `--allow-unhashed` so a
//! local development set can be used before the texture generator exists.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::atlas::{Atlas, AtlasBuilder, MAX_MATERIALS, TILE_BYTES, TILE_SIZE};

/// Source faces are authored at 256×256.
const SOURCE_EDGE: u32 = 256;
/// Bytes in one source face.
const SOURCE_BYTES: usize = (SOURCE_EDGE * SOURCE_EDGE * 4) as usize;
/// Source texels per output texel.
const BOX: u32 = SOURCE_EDGE / TILE_SIZE;

/// A failure while baking the atlas.
#[derive(Debug)]
pub enum BakeError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse(ron::error::SpannedError),
    Decode {
        path: PathBuf,
        source: png::DecodingError,
    },
    Encode {
        path: PathBuf,
        source: png::EncodingError,
    },
    Invalid(String),
}

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse(error) => write!(f, "manifest parse error: {error}"),
            Self::Decode { path, source } => {
                write!(f, "could not decode {}: {source}", path.display())
            }
            Self::Encode { path, source } => {
                write!(f, "could not encode {}: {source}", path.display())
            }
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for BakeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(error) => Some(error),
            Self::Decode { source, .. } => Some(source),
            Self::Encode { source, .. } => Some(source),
            Self::Invalid(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u32,
    license: String,
    provenance: String,
    textures: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    key: String,
    slot: u16,
    source: PathBuf,
    alpha: AlphaMode,
    #[serde(default)]
    hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
enum AlphaMode {
    Opaque,
    Cutout,
    Translucent,
}

/// Parses `bake-atlas` arguments and bakes the atlas.
pub fn run(args: &[String]) -> Result<(), BakeError> {
    let mut manifest = PathBuf::from("crates/engine/block/data/textures.ron");
    let mut out = PathBuf::from("target/atlas");
    let mut allow_unhashed = false;

    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--manifest" => {
                manifest = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| BakeError::Invalid("--manifest needs a path".to_owned()))?;
            }
            "--out" => {
                out = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| BakeError::Invalid("--out needs a directory".to_owned()))?;
            }
            "--allow-unhashed" => allow_unhashed = true,
            "-h" | "--help" => {
                println!(
                    "usage: voxel-viewer bake-atlas [options]\n\
                     \n\
                     --manifest PATH   texture manifest (default: crates/engine/block/data/textures.ron)\n\
                     --out DIR         output directory (default: target/atlas)\n\
                     --allow-unhashed  accept entries with an empty hash (local development only)"
                );
                return Ok(());
            }
            other => {
                return Err(BakeError::Invalid(format!("unknown argument `{other}`")));
            }
        }
    }

    bake(&manifest, &out, allow_unhashed)
}

/// Reads `manifest`, validates every source, and writes the atlas levels.
pub fn bake(manifest_path: &Path, out_dir: &Path, allow_unhashed: bool) -> Result<(), BakeError> {
    let text = fs::read_to_string(manifest_path).map_err(|source| BakeError::Io {
        path: manifest_path.to_owned(),
        source,
    })?;
    let manifest: Manifest = ron::from_str(&text).map_err(BakeError::Parse)?;

    if manifest.schema != 1 {
        return Err(BakeError::Invalid(format!(
            "manifest schema {} is not supported (expected 1)",
            manifest.schema
        )));
    }
    if manifest.license.trim().is_empty() || manifest.provenance.trim().is_empty() {
        return Err(BakeError::Invalid(
            "manifest license and provenance must not be empty".to_owned(),
        ));
    }
    if manifest.textures.is_empty() || manifest.textures.len() >= MAX_MATERIALS as usize {
        return Err(BakeError::Invalid(format!(
            "manifest has {} textures; expected 1..{}",
            manifest.textures.len(),
            MAX_MATERIALS
        )));
    }

    validate_slots(&manifest.textures)?;

    let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut builder = AtlasBuilder::new();
    for entry in &manifest.textures {
        let path = resolve_source(base, &entry.source)?;
        let bytes = fs::read(&path).map_err(|source| BakeError::Io {
            path: path.clone(),
            source,
        })?;
        if entry.hash.is_empty() {
            if !allow_unhashed {
                return Err(BakeError::Invalid(format!(
                    "{} has no hash; rerun with --allow-unhashed for a local set",
                    entry.key
                )));
            }
        } else {
            let actual = blake3::hash(&bytes).to_hex().to_string();
            if actual != entry.hash.to_ascii_lowercase() {
                return Err(BakeError::Invalid(format!(
                    "{} hash mismatch: manifest {}, file {actual}",
                    entry.key, entry.hash
                )));
            }
        }

        let source = decode_png(&path, &bytes)?;
        check_alpha(entry.alpha, &source, &entry.key)?;
        let tile = downsample(&source);
        builder
            .set_tile(entry.slot, &tile)
            .map_err(|error| BakeError::Invalid(format!("{}: {error}", entry.key)))?;
    }

    let atlas = builder.build();
    write_atlas(&atlas, out_dir)?;
    println!(
        "baked {} textures into {} levels, {} bytes -> {}",
        manifest.textures.len(),
        atlas.mip_count(),
        atlas.byte_len(),
        out_dir.display()
    );
    Ok(())
}

fn validate_slots(entries: &[Entry]) -> Result<(), BakeError> {
    let mut slots = vec![false; MAX_MATERIALS as usize];
    let mut keys = HashSet::with_capacity(entries.len());
    for entry in entries {
        if entry.slot == 0 || entry.slot as usize >= MAX_MATERIALS as usize {
            return Err(BakeError::Invalid(format!(
                "{} has slot {} outside 1..{}",
                entry.key, entry.slot, MAX_MATERIALS
            )));
        }
        if slots[entry.slot as usize] {
            return Err(BakeError::Invalid(format!(
                "slot {} is used twice",
                entry.slot
            )));
        }
        slots[entry.slot as usize] = true;
        if !keys.insert(entry.key.as_str()) {
            return Err(BakeError::Invalid(format!(
                "texture key {} is listed twice",
                entry.key
            )));
        }
    }
    for (slot, used) in slots.iter().enumerate().skip(1).take(entries.len()) {
        if !used {
            return Err(BakeError::Invalid(format!(
                "slot {slot} is missing; slots must be contiguous from 1"
            )));
        }
    }
    if slots[entries.len() + 1..].iter().any(|used| *used) {
        return Err(BakeError::Invalid(
            "slots must be contiguous from 1".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_source(base: &Path, source: &Path) -> Result<PathBuf, BakeError> {
    if source.is_absolute()
        || source
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BakeError::Invalid(format!(
            "source {} must be a relative path inside the manifest directory",
            source.display()
        )));
    }
    Ok(base.join(source))
}

fn decode_png(path: &Path, bytes: &[u8]) -> Result<Vec<u8>, BakeError> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|source| BakeError::Decode {
        path: path.to_owned(),
        source,
    })?;
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut data)
        .map_err(|source| BakeError::Decode {
            path: path.to_owned(),
            source,
        })?;
    if info.width != SOURCE_EDGE
        || info.height != SOURCE_EDGE
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(BakeError::Invalid(format!(
            "{} is {}×{} {:?} {:?}; expected {SOURCE_EDGE}×{SOURCE_EDGE} RGBA8",
            path.display(),
            info.width,
            info.height,
            info.color_type,
            info.bit_depth
        )));
    }
    data.truncate(SOURCE_BYTES);
    Ok(data)
}

fn check_alpha(mode: AlphaMode, pixels: &[u8], key: &str) -> Result<(), BakeError> {
    let mut intermediate = false;
    for texel in pixels.as_chunks::<4>().0 {
        let alpha = texel[3];
        match mode {
            AlphaMode::Opaque if alpha != 255 => {
                return Err(BakeError::Invalid(format!(
                    "{key} is Opaque but has alpha {alpha}"
                )));
            }
            AlphaMode::Cutout if alpha != 0 && alpha != 255 => {
                return Err(BakeError::Invalid(format!(
                    "{key} is Cutout but has intermediate alpha {alpha}"
                )));
            }
            AlphaMode::Translucent if alpha > 0 && alpha < 255 => intermediate = true,
            _ => {}
        }
    }
    if mode == AlphaMode::Translucent && !intermediate {
        return Err(BakeError::Invalid(format!(
            "{key} is Translucent but has no intermediate alpha"
        )));
    }
    Ok(())
}

/// Box-filters a 256×256 RGBA8 face to one 16×16 tile.
fn downsample(source: &[u8]) -> [u8; TILE_BYTES] {
    let mut tile = [0; TILE_BYTES];
    let source_edge = SOURCE_EDGE as usize;
    let tile_edge = TILE_SIZE as usize;
    for y in 0..tile_edge {
        for x in 0..tile_edge {
            let mut sums = [0u32; 4];
            for box_y in 0..BOX as usize {
                for box_x in 0..BOX as usize {
                    let offset =
                        ((y * BOX as usize + box_y) * source_edge + x * BOX as usize + box_x) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += source[offset + channel] as u32;
                    }
                }
            }
            let destination = (y * tile_edge + x) * 4;
            for (channel, sum) in sums.iter().enumerate() {
                tile[destination + channel] = ((sum + (BOX * BOX) / 2) / (BOX * BOX)) as u8;
            }
        }
    }
    tile
}

fn write_atlas(atlas: &Atlas, out_dir: &Path) -> Result<(), BakeError> {
    fs::create_dir_all(out_dir).map_err(|source| BakeError::Io {
        path: out_dir.to_owned(),
        source,
    })?;
    let mut hasher = blake3::Hasher::new();
    for (index, level) in atlas.levels().iter().enumerate() {
        let path = out_dir.join(format!("atlas-{index}.png"));
        write_png(&path, level.width(), level.height(), level.pixels())?;
        hasher.update(&level.width().to_le_bytes());
        hasher.update(&level.height().to_le_bytes());
        hasher.update(level.pixels());
    }
    let hash = hasher.finalize().to_hex().to_string();
    let hash_path = out_dir.join("atlas.blake3");
    let mut file = File::create(&hash_path).map_err(|source| BakeError::Io {
        path: hash_path.clone(),
        source,
    })?;
    writeln!(file, "{hash}").map_err(|source| BakeError::Io {
        path: hash_path,
        source,
    })?;
    log::info!("atlas hash: {hash}");
    Ok(())
}

fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), BakeError> {
    let file = File::create(path).map_err(|source| BakeError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|source| BakeError::Encode {
        path: path.to_owned(),
        source,
    })?;
    writer
        .write_image_data(pixels)
        .map_err(|source| BakeError::Encode {
            path: path.to_owned(),
            source,
        })
}
