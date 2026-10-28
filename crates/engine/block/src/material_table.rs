//! Generated material table (`data/materials.gen.ron`, M1.5-08).
//!
//! The table is derived from `textures.ron` and `blocks.ron`: one row per
//! material slot and one six-slot row per block. It is committed and checked
//! for staleness by `materials_gen_is_current`.
//!
//! The generator app calls this module rather than reimplementing the format,
//! so the committed file has exactly one writer.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ron::RonBlockList;
use crate::texture_manifest::{AlphaClass, TextureManifest};

/// One material row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialRow {
    pub id: u16,
    pub key: String,
    pub alpha: AlphaClass,
}

/// One block's six face slots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceRow {
    pub block: String,
    pub top: u16,
    pub bottom: u16,
    pub north: u16,
    pub south: u16,
    pub east: u16,
    pub west: u16,
}

/// The generated material table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialTable {
    pub schema: u32,
    pub manifest_blake3: String,
    pub blocks_blake3: String,
    pub materials: Vec<MaterialRow>,
    pub faces: Vec<FaceRow>,
}

/// A material-table failure.
#[derive(Debug)]
pub enum MaterialTableError {
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Parse(String),
    Invalid(String),
}

impl fmt::Display for MaterialTableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse(message) => f.write_str(message),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for MaterialTableError {}

/// Builds the table from the bytes of `textures.ron` and `blocks.ron`.
pub fn build(
    manifest_bytes: &[u8],
    blocks_bytes: &[u8],
) -> Result<MaterialTable, MaterialTableError> {
    let manifest_text = std::str::from_utf8(manifest_bytes).map_err(|error| {
        MaterialTableError::Parse(format!("textures.ron is not UTF-8: {error}"))
    })?;
    let manifest: TextureManifest = ron::from_str(manifest_text)
        .map_err(|error| MaterialTableError::Parse(format!("textures.ron: {error}")))?;
    if manifest.schema != 1 {
        return Err(MaterialTableError::Invalid(format!(
            "textures.ron schema {} is not supported",
            manifest.schema
        )));
    }
    let blocks_text = std::str::from_utf8(blocks_bytes)
        .map_err(|error| MaterialTableError::Parse(format!("blocks.ron is not UTF-8: {error}")))?;
    let blocks: RonBlockList = ron::from_str(blocks_text)
        .map_err(|error| MaterialTableError::Parse(format!("blocks.ron: {error}")))?;

    let mut slots: std::collections::BTreeMap<&str, u16> = std::collections::BTreeMap::new();
    for entry in &manifest.textures {
        if entry.slot == 0 || entry.slot as usize > u8::MAX as usize {
            return Err(MaterialTableError::Invalid(format!(
                "{}: slot {} is outside 1..=255",
                entry.key, entry.slot
            )));
        }
        if slots.insert(entry.key.as_str(), entry.slot).is_some() {
            return Err(MaterialTableError::Invalid(format!(
                "duplicate material key {}",
                entry.key
            )));
        }
    }
    let mut seen_slots = std::collections::BTreeSet::new();
    for entry in &manifest.textures {
        if !seen_slots.insert(entry.slot) {
            return Err(MaterialTableError::Invalid(format!(
                "duplicate material slot {}",
                entry.slot
            )));
        }
    }
    for (expected, slot) in (1u16..).zip(seen_slots.iter()) {
        if *slot != expected {
            return Err(MaterialTableError::Invalid(format!(
                "material slots must be contiguous from 1; found {slot} where {expected} was expected"
            )));
        }
    }
    let mut materials: Vec<MaterialRow> = manifest
        .textures
        .iter()
        .map(|entry| MaterialRow {
            id: entry.slot,
            key: entry.key.clone(),
            alpha: entry.alpha,
        })
        .collect();
    materials.sort_by_key(|row| row.id);

    let mut faces = Vec::new();
    for block in &blocks.blocks {
        let Some(textures) = &block.textures else {
            continue;
        };
        let slot = |key: &str| {
            slots.get(key).copied().ok_or_else(|| {
                MaterialTableError::Invalid(format!(
                    "{}: face key {key} has no manifest entry",
                    block.key
                ))
            })
        };
        faces.push(FaceRow {
            block: block.key.clone(),
            top: slot(&textures.top)?,
            bottom: slot(&textures.bottom)?,
            north: slot(&textures.north)?,
            south: slot(&textures.south)?,
            east: slot(&textures.east)?,
            west: slot(&textures.west)?,
        });
    }

    Ok(MaterialTable {
        schema: 1,
        manifest_blake3: blake3::hash(manifest_bytes).to_hex().to_string(),
        blocks_blake3: blake3::hash(blocks_bytes).to_hex().to_string(),
        materials,
        faces,
    })
}

/// Reads both inputs from a block crate root and builds the table.
pub fn generate(crate_root: &Path) -> Result<MaterialTable, MaterialTableError> {
    let manifest_path = crate_root.join("data/textures.ron");
    let blocks_path = crate_root.join("data/blocks.ron");
    let manifest = fs::read(&manifest_path).map_err(|source| MaterialTableError::Io {
        path: manifest_path,
        source,
    })?;
    let blocks = fs::read(&blocks_path).map_err(|source| MaterialTableError::Io {
        path: blocks_path,
        source,
    })?;
    build(&manifest, &blocks)
}

/// Canonical RON with fixed field order, four-space indent and trailing newline.
pub fn to_ron(table: &MaterialTable) -> Result<String, MaterialTableError> {
    let pretty = ron::ser::PrettyConfig::default()
        .struct_names(false)
        .indentor("    ".to_owned())
        .new_line("\n".to_owned())
        .separate_tuple_members(false)
        .enumerate_arrays(false)
        .compact_arrays(false);
    let mut text = ron::ser::to_string_pretty(table, pretty)
        .map_err(|error| MaterialTableError::Parse(format!("materials.gen.ron: {error}")))?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_slots_are_rejected() {
        let manifest = br#"(
            schema: 1,
            license: "CC0-1.0",
            provenance: "test",
            textures: [
                (key: "voxel:block/a", slot: 1, source: "a.png", alpha: Opaque, tiling: Both, hash: "", graph: None, graph_hash: None, placeholder: false),
                (key: "voxel:block/b", slot: 1, source: "b.png", alpha: Opaque, tiling: Both, hash: "", graph: None, graph_hash: None, placeholder: false),
            ],
        )"#;
        let blocks = br#"(blocks: [])"#;
        assert!(build(manifest, blocks).is_err());
    }
}
