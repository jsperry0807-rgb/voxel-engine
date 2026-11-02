//! Local (uncommitted) texture sets.
//!
//! A texture set is a folder of named 256x256 RGBA8 faces, laid out flat:
//!
//! ```text
//! assets/textures/<set>/stone.png
//! assets/textures/<set>/dirt.png
//! ```
//!
//! Baking one writes the viewer's multi-level atlas format next to it:
//!
//! ```text
//! assets/atlas/<set>/atlas-0.png … atlas-4.png
//! assets/atlas/<set>/atlas.blake3
//! ```
//!
//! The set is matched against the committed face table (`data/faces.ron`), so
//! a set only has to supply the faces the engine actually samples. Resource
//! packs are the main user: their textures are licensed for local use only, so
//! they live under the gitignored `assets/` tree and never reach the committed
//! manifest.

use std::fs;
use std::path::{Component, Path, PathBuf};

use voxel_block::texture_manifest::{alpha_class, block_face_classes, decode_source};
use voxel_core::atlas::{ATLAS_SIZE, TILE_SIZE};

use crate::bake::{self, BakeError};
use crate::pipeline::{self, Workspace};
use crate::png::encode_rgba;

/// Folder holding every local texture set, relative to the workspace root.
pub const SETS_DIR: &str = "assets/textures";
/// Folder holding every baked local atlas, relative to the workspace root.
pub const ATLASES_DIR: &str = "assets/atlas";
/// Hash file written next to the baked mip levels (BLAKE3 of `atlas-0.png`).
pub const HASH_FILE: &str = "atlas.blake3";

/// The folder that holds one named texture set.
pub fn set_dir(workspace: &Workspace, set: &str) -> PathBuf {
    workspace.root().join(SETS_DIR).join(set)
}

/// The folder that receives one baked local atlas.
pub fn atlas_dir(workspace: &Workspace, set: &str) -> PathBuf {
    workspace.root().join(ATLASES_DIR).join(set)
}

/// Bakes `assets/textures/<set>` into `assets/atlas/<set>`.
///
/// With `check`, compares the bytes that would be written instead of writing
/// them. Every face in the committed table must exist in the set; the error
/// names the missing ones so a new face is easy to spot.
pub fn bake_set(workspace: &Workspace, set: &str, check: bool) -> Result<String, BakeError> {
    check_set_name(set)?;
    let sources = set_dir(workspace, set);
    if !sources.is_dir() {
        return Err(BakeError::Invalid(format!(
            "{} does not exist; create it and add the face PNGs",
            sources.display()
        )));
    }

    let blocks = fs::read_to_string(workspace.blocks_path()).map_err(|source| BakeError::Io {
        path: workspace.blocks_path(),
        source,
    })?;
    let classes = block_face_classes(&blocks).map_err(BakeError::Manifest)?;

    let mut base = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
    let mut slots = Vec::with_capacity(workspace.faces().len());
    let mut missing = Vec::new();
    for (key, slot) in workspace.faces().iter() {
        let name = key.strip_prefix("voxel:block/").unwrap_or(key);
        let path = sources.join(format!("{name}.png"));
        if !path.is_file() {
            missing.push(format!("{name}.png"));
            continue;
        }
        let render_type = classes.get(key).copied().ok_or_else(|| {
            BakeError::Invalid(format!("{key} is not referenced by data/blocks.ron"))
        })?;
        let alpha = alpha_class(render_type).ok_or_else(|| {
            BakeError::Invalid(format!("{key} is invisible and must not have a texture"))
        })?;
        let bytes = fs::read(&path).map_err(|source| BakeError::Io {
            path: path.clone(),
            source,
        })?;
        let image = decode_source(&path, &bytes).map_err(BakeError::Manifest)?;
        let tile = bake::downsample(&image.pixels, alpha)?;
        bake::blit_tile(&mut base, slot, &tile, TILE_SIZE)?;
        slots.push(slot);
    }
    if !missing.is_empty() {
        return Err(BakeError::Invalid(format!(
            "{} is missing {} face(s): {}",
            sources.display(),
            missing.len(),
            missing.join(", ")
        )));
    }

    let levels = bake::mip_chain(base, &slots)?;
    let mut encoded = Vec::with_capacity(levels.len());
    for level in &levels {
        encoded
            .push(encode_rgba(level.width, level.height, &level.pixels).map_err(BakeError::Png)?);
    }
    let hash = blake3::hash(&encoded[0]).to_hex().to_string();
    let hash_text = format!("{hash}\n");

    let destination = atlas_dir(workspace, set);
    if check {
        for (index, bytes) in encoded.iter().enumerate() {
            let path = destination.join(format!("atlas-{index}.png"));
            let committed = fs::read(&path).map_err(|_| {
                BakeError::Invalid(format!(
                    "{} is missing; run `texgen bake-set {set}`",
                    path.display()
                ))
            })?;
            if &committed != bytes {
                return Err(BakeError::Invalid(format!(
                    "{} differs from the set bake",
                    path.display()
                )));
            }
        }
        let hash_path = destination.join(HASH_FILE);
        let committed = fs::read_to_string(&hash_path).map_err(|_| {
            BakeError::Invalid(format!(
                "{} is missing; run `texgen bake-set {set}`",
                hash_path.display()
            ))
        })?;
        if committed != hash_text {
            return Err(BakeError::Invalid(format!(
                "{} differs from the set bake",
                hash_path.display()
            )));
        }
        return Ok(format!(
            "atlas for set `{set}` is current: {} faces, {hash}",
            slots.len()
        ));
    }

    fs::create_dir_all(&destination).map_err(|source| BakeError::Io {
        path: destination.clone(),
        source,
    })?;
    for (index, bytes) in encoded.iter().enumerate() {
        pipeline::write_atomic(&destination.join(format!("atlas-{index}.png")), bytes)?;
    }
    pipeline::write_atomic(&destination.join(HASH_FILE), hash_text.as_bytes())?;
    Ok(format!(
        "baked {} levels for set `{set}` from {} ({} faces, {hash}) -> {}",
        encoded.len(),
        sources.display(),
        slots.len(),
        destination.display()
    ))
}

/// Rejects a set name that could escape the sets directory.
fn check_set_name(set: &str) -> Result<(), BakeError> {
    let mut components = Path::new(set).components();
    let valid =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if valid {
        Ok(())
    } else {
        Err(BakeError::Invalid(format!(
            "`{set}` is not a plain texture-set name"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::check_set_name;

    #[test]
    fn set_names_cannot_escape_the_folder() {
        assert!(check_set_name("resourcepack").is_ok());
        assert!(check_set_name("voxgen-v2").is_ok());
        assert!(check_set_name("").is_err());
        assert!(check_set_name("../resourcepack").is_err());
        assert!(check_set_name("a/b").is_err());
        assert!(check_set_name("/abs").is_err());
    }
}
