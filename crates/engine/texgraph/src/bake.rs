//! Deterministic atlas bake: 256x256 sources -> 16x16 tiles -> 256x256 atlas.
//!
//! The filter is `box16-linear-premul-v1` (docs/generator.spec.md section
//! 6.2): exact 16x16 boxes, alpha-weighted averaging in linear light, integer
//! math only, and an alpha rule per [`AlphaClass`].

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use voxel_block::texture_manifest::{AlphaClass, TextureEntry, TextureManifest};
use voxel_core::atlas::{ATLAS_SIZE, MAX_MIP, MIP_LEVELS, TILE_SIZE, tile_origin};

use crate::luts::{LINEAR_TO_SRGB, SRGB_TO_LINEAR};
use crate::png::encode_rgba;
use crate::{graph, pipeline};

/// Filter id written into `atlas.hash.ron`.
pub const FILTER: &str = "box16-linear-premul-v1";
/// Source edge, texels.
const SOURCE_EDGE: usize = 256;
/// Tile edge, texels.
const TILE_EDGE: usize = TILE_SIZE as usize;
/// RGBA bytes in one tile.
const TILE_BYTES: usize = TILE_EDGE * TILE_EDGE * 4;

/// One mip level.
#[derive(Clone, Debug)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// A built atlas and its hashes.
#[derive(Clone, Debug)]
pub struct AtlasBuild {
    pub levels: Vec<Level>,
    pub png: Vec<u8>,
    pub atlas_png_blake3: String,
    pub mip_chain_blake3: String,
    pub manifest_blake3: String,
    pub tile_count: u32,
}

/// The committed `atlas.hash.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AtlasHash {
    pub schema: u32,
    pub filter: String,
    pub tile_count: u32,
    pub manifest_blake3: String,
    pub atlas_png_blake3: String,
    pub mip_chain_blake3: String,
}

/// A bake failure.
#[derive(Debug)]
pub enum BakeError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Manifest(voxel_block::texture_manifest::ManifestError),
    Graph(graph::GraphError),
    Png(crate::png::PngError),
    Ron(ron::error::Error),
    Invalid(String),
}

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Manifest(error) => write!(f, "manifest: {error}"),
            Self::Graph(error) => write!(f, "graph: {error}"),
            Self::Png(error) => write!(f, "png: {error}"),
            Self::Ron(error) => write!(f, "ron: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for BakeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Manifest(error) => Some(error),
            Self::Graph(error) => Some(error),
            Self::Png(error) => Some(error),
            Self::Ron(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

/// Builds the atlas from the committed manifest and sources, without writing.
pub fn build_atlas(workspace: &pipeline::Workspace) -> Result<AtlasBuild, BakeError> {
    let manifest_path = workspace.manifest_path();
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| BakeError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = TextureManifest::load(&manifest_path).map_err(BakeError::Manifest)?;
    voxel_block::texture_manifest::validate(&manifest, &workspace.block_crate())
        .map_err(BakeError::Manifest)?;

    let mut base = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
    for entry in manifest.sorted() {
        let image = voxel_block::texture_manifest::load_source(&workspace.block_crate(), entry)
            .map_err(BakeError::Manifest)?;
        let tile = downsample(&image.pixels, entry.alpha)?;
        blit_tile(&mut base, entry.slot, &tile, TILE_SIZE)?;
    }

    let mut levels = Vec::with_capacity(MIP_LEVELS as usize);
    levels.push(Level {
        width: ATLAS_SIZE,
        height: ATLAS_SIZE,
        pixels: base,
    });
    for level in 1..=MAX_MIP {
        let edge = ATLAS_SIZE >> level;
        let mut pixels = vec![0u8; (edge * edge * 4) as usize];
        let tile_edge = TILE_SIZE >> level;
        for entry in manifest.sorted() {
            let origin = tile_origin(entry.slot);
            let source_origin = (origin.0 >> (level - 1), origin.1 >> (level - 1));
            let previous = &levels[(level - 1) as usize];
            let tile = read_region(previous, source_origin, TILE_SIZE >> (level - 1))?;
            let reduced = halve(&tile, TILE_SIZE >> (level - 1));
            blit_region(
                &mut pixels,
                edge,
                (origin.0 >> level, origin.1 >> level),
                &reduced,
                tile_edge,
            )?;
        }
        levels.push(Level {
            width: edge,
            height: edge,
            pixels,
        });
    }

    let png = encode_rgba(ATLAS_SIZE, ATLAS_SIZE, &levels[0].pixels).map_err(BakeError::Png)?;
    let atlas_png_blake3 = blake3::hash(&png).to_hex().to_string();
    let mut hasher = blake3::Hasher::new();
    for level in &levels {
        hasher.update(&level.pixels);
    }
    let mip_chain_blake3 = hasher.finalize().to_hex().to_string();
    let manifest_blake3 = blake3::hash(&manifest_bytes).to_hex().to_string();

    Ok(AtlasBuild {
        levels,
        png,
        atlas_png_blake3,
        mip_chain_blake3,
        manifest_blake3,
        tile_count: manifest.textures.len() as u32,
    })
}

/// Returns the canonical `atlas.hash.ron` text.
pub fn hash_ron(build: &AtlasBuild) -> Result<String, BakeError> {
    let hash = AtlasHash {
        schema: 1,
        filter: FILTER.to_owned(),
        tile_count: build.tile_count,
        manifest_blake3: build.manifest_blake3.clone(),
        atlas_png_blake3: build.atlas_png_blake3.clone(),
        mip_chain_blake3: build.mip_chain_blake3.clone(),
    };
    let mut text =
        ron::ser::to_string_pretty(&hash, graph::pretty_config()).map_err(BakeError::Ron)?;
    text.push('\n');
    Ok(text)
}

/// Writes the atlas, or with `check` compares against committed bytes.
///
/// Returns a short human-readable status either way.
pub fn bake_atlas(workspace: &pipeline::Workspace, check: bool) -> Result<String, BakeError> {
    let build = build_atlas(workspace)?;
    let hash_text = hash_ron(&build)?;
    let png_path = workspace.atlas_png_path();
    let hash_path = workspace.atlas_hash_path();

    if check {
        let committed_png = fs::read(&png_path).map_err(|source| BakeError::Io {
            path: png_path.clone(),
            source,
        })?;
        if committed_png != build.png {
            return Err(BakeError::Invalid(format!(
                "{} differs from the generated atlas",
                png_path.display()
            )));
        }
        let committed_hash = fs::read_to_string(&hash_path).map_err(|source| BakeError::Io {
            path: hash_path.clone(),
            source,
        })?;
        if committed_hash != hash_text {
            return Err(BakeError::Invalid(format!(
                "{} differs from the generated atlas hash",
                hash_path.display()
            )));
        }
        return Ok(format!(
            "atlas is current: {} tiles, {}",
            build.tile_count, build.atlas_png_blake3
        ));
    }

    pipeline::write_atomic(&png_path, &build.png)
        .map_err(|error| BakeError::Invalid(error.to_string()))?;
    pipeline::write_atomic(&hash_path, hash_text.as_bytes())
        .map_err(|error| BakeError::Invalid(error.to_string()))?;
    Ok(format!(
        "baked {} tiles -> {} ({})",
        build.tile_count,
        png_path.display(),
        build.atlas_png_blake3
    ))
}

/// Box-filters one 256x256 RGBA8 source to a 16x16 tile.
pub fn downsample(source: &[u8], alpha: AlphaClass) -> Result<[u8; TILE_BYTES], BakeError> {
    downsample_edge(source, SOURCE_EDGE as u32, alpha)
}

/// Box-filters any square RGBA8 source whose edge is a multiple of 16.
///
/// The editor uses this at 64x64 and 128x128 for a live tile preview; the final
/// bake always calls [`downsample`] at 256x256. Rejects a source edge below one
/// tile or a buffer whose length does not match `edge × edge × 4`, so a
/// mis-sized RGBA8 buffer is an error rather than a panic.
pub fn downsample_edge(
    source: &[u8],
    source_edge: u32,
    alpha: AlphaClass,
) -> Result<[u8; TILE_BYTES], BakeError> {
    let tile_edge = TILE_EDGE as u32;
    if source_edge < tile_edge || !source_edge.is_multiple_of(tile_edge) {
        return Err(BakeError::Invalid(format!(
            "source edge {source_edge} must be a positive multiple of {tile_edge}"
        )));
    }
    let edge = source_edge as usize;
    let expected = edge
        .checked_mul(edge)
        .and_then(|texels| texels.checked_mul(4))
        .ok_or_else(|| BakeError::Invalid(format!("source edge {source_edge} is too large")))?;
    if source.len() != expected {
        return Err(BakeError::Invalid(format!(
            "source is {} bytes, expected {expected} for {source_edge}x{source_edge} RGBA8",
            source.len(),
        )));
    }
    let box_edge = edge / TILE_EDGE;
    let source_edge = source_edge as usize;
    let mut tile = [0u8; TILE_BYTES];
    for ty in 0..TILE_EDGE {
        for tx in 0..TILE_EDGE {
            let mut sums = [0u64; 3];
            let mut sum_alpha = 0u64;
            for by in 0..box_edge {
                let y = ty * box_edge + by;
                for bx in 0..box_edge {
                    let x = tx * box_edge + bx;
                    let offset = (y * source_edge + x) * 4;
                    let a = source[offset + 3] as u64;
                    sum_alpha += a;
                    sums[0] += SRGB_TO_LINEAR[source[offset] as usize] as u64 * a;
                    sums[1] += SRGB_TO_LINEAR[source[offset + 1] as usize] as u64 * a;
                    sums[2] += SRGB_TO_LINEAR[source[offset + 2] as usize] as u64 * a;
                }
            }
            let destination = (ty * TILE_EDGE + tx) * 4;
            if sum_alpha == 0 {
                tile[destination] = 0;
                tile[destination + 1] = 0;
                tile[destination + 2] = 0;
            } else {
                for channel in 0..3 {
                    let linear = sums[channel]
                        .checked_add(sum_alpha / 2)
                        .and_then(|sum| sum.checked_div(sum_alpha))
                        .unwrap_or(0)
                        .min(65_535);
                    tile[destination + channel] = LINEAR_TO_SRGB[linear as usize];
                }
            }
            tile[destination + 3] = match alpha {
                AlphaClass::Opaque => 255,
                AlphaClass::Cutout => {
                    if 2 * sum_alpha >= (box_edge * box_edge * 255) as u64 {
                        255
                    } else {
                        0
                    }
                }
                AlphaClass::Translucent => {
                    ((sum_alpha + (box_edge * box_edge) as u64 / 2) / (box_edge * box_edge) as u64)
                        as u8
                }
            };
        }
    }
    Ok(tile)
}

fn blit_tile(base: &mut [u8], slot: u16, tile: &[u8], tile_edge: u32) -> Result<(), BakeError> {
    let origin = tile_origin(slot);
    blit_region(base, ATLAS_SIZE, origin, tile, tile_edge)
}

fn blit_region(
    destination: &mut [u8],
    edge: u32,
    origin: (u32, u32),
    tile: &[u8],
    tile_edge: u32,
) -> Result<(), BakeError> {
    let row_bytes = tile_edge as usize * 4;
    let stride = edge as usize * 4;
    let start = origin.1 as usize * stride + origin.0 as usize * 4;
    for (row, source) in tile.chunks_exact(row_bytes).enumerate() {
        let offset = start + row * stride;
        let Some(target) = destination.get_mut(offset..offset + row_bytes) else {
            return Err(BakeError::Invalid(format!(
                "tile blit at ({}, {}) is outside the {edge}x{edge} destination",
                origin.0, origin.1,
            )));
        };
        target.copy_from_slice(source);
    }
    Ok(())
}

fn read_region(level: &Level, origin: (u32, u32), edge: u32) -> Result<Vec<u8>, BakeError> {
    let row_bytes = edge as usize * 4;
    let stride = level.width as usize * 4;
    let mut out = vec![0u8; row_bytes * edge as usize];
    for row in 0..edge as usize {
        let source = origin.1 as usize * stride + origin.0 as usize * 4 + row * stride;
        let Some(pixels) = level.pixels.get(source..source + row_bytes) else {
            return Err(BakeError::Invalid(format!(
                "mip read at ({}, {}) is outside the {}x{} level",
                origin.0, origin.1, level.width, level.height,
            )));
        };
        out[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(pixels);
    }
    Ok(out)
}

fn halve(source: &[u8], edge: u32) -> Vec<u8> {
    let edge = edge as usize;
    let next = edge / 2;
    let mut out = vec![0u8; next * next * 4];
    for y in 0..next {
        for x in 0..next {
            let top = ((y * 2) * edge + x * 2) * 4;
            let bottom = top + edge * 4;
            for channel in 0..4 {
                let sum = source[top + channel] as u16
                    + source[top + 4 + channel] as u16
                    + source[bottom + channel] as u16
                    + source[bottom + 4 + channel] as u16;
                out[(y * next + x) * 4 + channel] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

/// Convenience for the editor's material browser: the 16x16 tile for one
/// entry, using the same filter as the committed atlas.
pub fn preview_tile(
    workspace: &pipeline::Workspace,
    entry: &TextureEntry,
) -> Result<[u8; TILE_BYTES], BakeError> {
    let image = voxel_block::texture_manifest::load_source(&workspace.block_crate(), entry)
        .map_err(BakeError::Manifest)?;
    downsample(&image.pixels, entry.alpha)
}

/// Writes a PNG without going through the atlas layout (used by tests).
pub fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), BakeError> {
    let bytes = encode_rgba(width, height, pixels).map_err(BakeError::Png)?;
    pipeline::write_atomic(path, &bytes).map_err(|error| BakeError::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_box_is_the_alpha_weighted_mean() {
        let mut source = vec![0u8; SOURCE_EDGE * SOURCE_EDGE * 4];
        for texel in source.as_chunks_mut::<4>().0 {
            *texel = [255, 0, 0, 255];
        }
        let tile = downsample(&source, AlphaClass::Opaque).expect("256x256 RGBA8");
        assert_eq!(tile[0..4], [255, 0, 0, 255]);
        assert!(
            tile.as_chunks::<4>()
                .0
                .iter()
                .all(|t| *t == [255, 0, 0, 255])
        );
    }

    #[test]
    fn cutout_coverage_threshold_is_half() {
        let mut source = vec![0u8; SOURCE_EDGE * SOURCE_EDGE * 4];
        for (index, texel) in source.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = index % SOURCE_EDGE;
            let filled = (x / 8).is_multiple_of(2);
            *texel = [10, 20, 30, if filled { 255 } else { 0 }];
        }
        let tile = downsample(&source, AlphaClass::Cutout).expect("256x256 RGBA8");
        for texel in tile.as_chunks::<4>().0 {
            assert_eq!(texel[3], 255, "half coverage rounds to opaque");
        }
    }

    #[test]
    fn downsample_edge_rejects_invalid_geometry() {
        let tiny = vec![0u8; 8 * 8 * 4];
        assert!(
            downsample_edge(&tiny, 8, AlphaClass::Opaque).is_err(),
            "an edge below one tile would divide by zero"
        );

        let short = vec![0u8; 8 * 8 * 4];
        assert!(
            downsample_edge(&short, 64, AlphaClass::Opaque).is_err(),
            "the buffer length must match the edge"
        );

        let ok = vec![0u8; 64 * 64 * 4];
        assert!(downsample_edge(&ok, 64, AlphaClass::Opaque).is_ok());
    }
}
