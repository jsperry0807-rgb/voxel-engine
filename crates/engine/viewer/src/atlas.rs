//! Material atlas construction: one 256×256 RGBA8 image in a 16×16 grid of
//! 16×16 tiles.
//!
//! The layout is owned by [`voxel_core::atlas`] and re-exported here. The
//! builder turns 256 tiles into the base image plus four mip levels. Mips are
//! filtered **per tile**, never across tile boundaries, so every tile stays
//! whole at every level and level 4 is exactly one texel per tile.
//!
//! ```text
//! level 0: 256×256  (16×16 texels per tile)
//! level 1: 128×128  ( 8× 8)
//! level 2:  64× 64  ( 4× 4)
//! level 3:  32× 32  ( 2× 2)
//! level 4:  16× 16  ( 1× 1)
//! ```
//!
//! Empty tiles are transparent black until a source face fills them; M1.5-07
//! supplies the downsampled face data.

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use voxel_core::atlas::{
    ATLAS_SIZE, MAX_MATERIALS, MAX_MIP, MIP_LEVELS, TILE_SIZE, TILES_PER_EDGE, tile_origin, uv,
};

/// RGBA8 bytes in one 16×16 tile.
pub const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 4) as usize;
/// RGBA8 bytes in the base atlas image.
pub const ATLAS_BYTES: usize = (ATLAS_SIZE * ATLAS_SIZE * 4) as usize;

/// A failure while assembling an atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasError {
    /// The tile id is outside the 16×16 address space.
    TileOutOfRange(u16),
}

impl fmt::Display for AtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TileOutOfRange(tile) => {
                write!(f, "tile {tile} is outside 0..{MAX_MATERIALS}")
            }
        }
    }
}

impl Error for AtlasError {}

/// A failure while loading a baked atlas from PNG files.
#[derive(Debug)]
pub enum AtlasLoadError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Png {
        path: PathBuf,
        source: png::DecodingError,
    },
    Format {
        path: PathBuf,
        message: String,
    },
    Dimensions {
        path: PathBuf,
        width: u32,
        height: u32,
        expected: u32,
    },
    Hash(String),
}

impl fmt::Display for AtlasLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Png { path, source } => {
                write!(f, "could not decode {}: {source}", path.display())
            }
            Self::Format { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Dimensions {
                path,
                width,
                height,
                expected,
            } => write!(
                f,
                "{} is {width}×{height}; expected {expected}×{expected}",
                path.display()
            ),
            Self::Hash(message) => f.write_str(message),
        }
    }
}

impl Error for AtlasLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Png { source, .. } => Some(source),
            Self::Format { .. } | Self::Dimensions { .. } | Self::Hash(_) => None,
        }
    }
}

/// The committed atlas verification data.
#[derive(Debug, Deserialize)]
struct AtlasHashFile {
    schema: u32,
    filter: String,
    tile_count: u32,
    manifest_blake3: String,
    atlas_png_blake3: String,
    mip_chain_blake3: String,
}

/// Collects source tiles and builds the atlas.
///
/// Every tile starts transparent, so a builder can fill only the materials it
/// has art for. Tile ids are material ids.
pub struct AtlasBuilder {
    tiles: Vec<[u8; TILE_BYTES]>,
}

impl AtlasBuilder {
    /// Creates a builder with 256 empty tiles.
    pub fn new() -> Self {
        Self {
            tiles: vec![[0; TILE_BYTES]; MAX_MATERIALS as usize],
        }
    }

    /// Copies one 16×16 RGBA8 tile into `tile`.
    ///
    /// # Errors
    ///
    /// Returns [`AtlasError::TileOutOfRange`] when `tile` is not a slot in the
    /// 16×16 grid.
    pub fn set_tile(&mut self, tile: u16, pixels: &[u8; TILE_BYTES]) -> Result<(), AtlasError> {
        let slot = self
            .tiles
            .get_mut(tile as usize)
            .ok_or(AtlasError::TileOutOfRange(tile))?;
        slot.copy_from_slice(pixels);
        Ok(())
    }

    /// Builds the base image and its four mip levels.
    pub fn build(self) -> Atlas {
        let mut levels: Vec<MipLevel> = (0..MIP_LEVELS)
            .map(|level| {
                let edge = ATLAS_SIZE >> level;
                MipLevel {
                    width: edge,
                    height: edge,
                    pixels: vec![0; (edge * edge * 4) as usize],
                }
            })
            .collect();

        for (tile, pixels) in self.tiles.iter().enumerate() {
            let (origin_x, origin_y) = tile_origin(tile as u16);
            let mut mip = pixels.to_vec();
            let mut tile_edge = TILE_SIZE;
            for (level, atlas) in levels.iter_mut().enumerate() {
                blit(
                    atlas,
                    (origin_x >> level, origin_y >> level),
                    &mip,
                    tile_edge,
                );
                if level + 1 < MIP_LEVELS as usize {
                    mip = downsample(&mip, tile_edge);
                    tile_edge /= 2;
                }
            }
        }

        Atlas { levels }
    }
}

impl Default for AtlasBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// One mip level of the atlas.
#[derive(Clone, Debug)]
pub struct MipLevel {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl MipLevel {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Row-major RGBA8 texels.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// One texel, or `None` when the coordinates are outside the level.
    pub fn texel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = ((y * self.width + x) * 4) as usize;
        Some(self.pixels[offset..offset + 4].try_into().unwrap())
    }
}

/// The finished atlas: base image plus [`MIP_LEVELS`] - 1 mip levels.
#[derive(Clone, Debug)]
pub struct Atlas {
    levels: Vec<MipLevel>,
}

impl Atlas {
    /// Loads `atlas-0.png` through `atlas-4.png` from `dir`.
    ///
    /// This is the temporary local-development path for the baked `.temp`
    /// atlas. The files are RGBA8 and each level must be the expected size.
    pub fn load_dir(dir: &Path) -> Result<Self, AtlasLoadError> {
        let mut levels = Vec::with_capacity(MIP_LEVELS as usize);
        for level in 0..MIP_LEVELS {
            let path = dir.join(format!("atlas-{level}.png"));
            let expected = ATLAS_SIZE >> level;
            let (width, height, pixels) = decode_png(&path)?;
            if width != expected || height != expected {
                return Err(AtlasLoadError::Dimensions {
                    path,
                    width,
                    height,
                    expected,
                });
            }
            levels.push(MipLevel {
                width,
                height,
                pixels,
            });
        }
        Ok(Self { levels })
    }

    /// Loads the base `atlas.png` and regenerates the four per-tile mip
    /// levels. The committed format stores only the base image.
    pub fn load(path: &Path) -> Result<Self, AtlasLoadError> {
        let (width, height, pixels) = decode_png(path)?;
        if width != ATLAS_SIZE || height != ATLAS_SIZE {
            return Err(AtlasLoadError::Dimensions {
                path: path.to_owned(),
                width,
                height,
                expected: ATLAS_SIZE,
            });
        }
        let mut builder = AtlasBuilder::new();
        for tile in 0..MAX_MATERIALS {
            let (origin_x, origin_y) = tile_origin(tile as u16);
            let mut tile_pixels = [0u8; TILE_BYTES];
            for row in 0..TILE_SIZE as usize {
                let source =
                    ((origin_y as usize + row) * ATLAS_SIZE as usize + origin_x as usize) * 4;
                tile_pixels[row * TILE_SIZE as usize * 4..(row + 1) * TILE_SIZE as usize * 4]
                    .copy_from_slice(&pixels[source..source + TILE_SIZE as usize * 4]);
            }
            builder
                .set_tile(tile as u16, &tile_pixels)
                .map_err(|error| AtlasLoadError::Format {
                    path: path.to_owned(),
                    message: error.to_string(),
                })?;
        }
        Ok(builder.build())
    }

    /// Loads `atlas.png` and verifies it against the committed
    /// `atlas.hash.ron` and the manifest hash.
    ///
    /// A mismatch is an error. There is no magenta fallback (spec 6.4).
    pub fn load_verified(path: &Path) -> Result<Self, AtlasLoadError> {
        let bytes = fs::read(path).map_err(|source| AtlasLoadError::Io {
            path: path.to_owned(),
            source,
        })?;
        let atlas = Self::load(path)?;
        let hash_path = path.with_file_name("atlas.hash.ron");
        let hash_text = fs::read_to_string(&hash_path).map_err(|source| AtlasLoadError::Io {
            path: hash_path.clone(),
            source,
        })?;
        let hash: AtlasHashFile =
            ron::from_str(&hash_text).map_err(|error| AtlasLoadError::Format {
                path: hash_path.clone(),
                message: format!("could not parse: {error}"),
            })?;
        if hash.schema != 1 {
            return Err(AtlasLoadError::Hash(format!(
                "{}: schema {} is not supported",
                hash_path.display(),
                hash.schema
            )));
        }
        if hash.filter != "box16-linear-premul-v1" {
            return Err(AtlasLoadError::Hash(format!(
                "{}: unknown filter {:?}",
                hash_path.display(),
                hash.filter
            )));
        }
        let actual_png = blake3::hash(&bytes).to_hex().to_string();
        if actual_png != hash.atlas_png_blake3 {
            return Err(AtlasLoadError::Hash(format!(
                "{}: atlas.png hash {actual_png} does not match {}",
                hash_path.display(),
                hash.atlas_png_blake3
            )));
        }
        let mut hasher = blake3::Hasher::new();
        for level in &atlas.levels {
            hasher.update(&level.pixels);
        }
        let actual_mips = hasher.finalize().to_hex().to_string();
        if actual_mips != hash.mip_chain_blake3 {
            return Err(AtlasLoadError::Hash(format!(
                "{}: mip-chain hash {actual_mips} does not match {}",
                hash_path.display(),
                hash.mip_chain_blake3
            )));
        }
        let manifest_path = path
            .parent()
            .map(|directory| directory.join("../../../../engine/block/data/textures.ron"))
            .unwrap_or_else(|| PathBuf::from("crates/engine/block/data/textures.ron"));
        let manifest = fs::read(&manifest_path).map_err(|source| AtlasLoadError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let actual_manifest = blake3::hash(&manifest).to_hex().to_string();
        if actual_manifest != hash.manifest_blake3 {
            return Err(AtlasLoadError::Hash(format!(
                "{}: manifest hash {actual_manifest} does not match {}",
                hash_path.display(),
                hash.manifest_blake3
            )));
        }
        if hash.tile_count == 0 {
            return Err(AtlasLoadError::Hash(format!(
                "{}: tile_count must be nonzero",
                hash_path.display()
            )));
        }
        Ok(atlas)
    }

    /// Number of levels including the base image; always [`MIP_LEVELS`].
    pub fn mip_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// All levels, base first.
    pub fn levels(&self) -> &[MipLevel] {
        &self.levels
    }

    /// One level, or `None` past [`MAX_MIP`].
    pub fn level(&self, level: u32) -> Option<&MipLevel> {
        self.levels.get(level as usize)
    }

    /// Total RGBA8 bytes across every level.
    pub fn byte_len(&self) -> usize {
        self.levels.iter().map(|level| level.pixels.len()).sum()
    }
}

fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), AtlasLoadError> {
    let file = File::open(path).map_err(|source| AtlasLoadError::Io {
        path: path.to_owned(),
        source,
    })?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().map_err(|source| AtlasLoadError::Png {
        path: path.to_owned(),
        source,
    })?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|source| AtlasLoadError::Png {
            path: path.to_owned(),
            source,
        })?;
    if info.bit_depth != png::BitDepth::Eight || info.color_type != png::ColorType::Rgba {
        return Err(AtlasLoadError::Format {
            path: path.to_owned(),
            message: format!(
                "expected RGBA8, found {:?} {:?}",
                info.color_type, info.bit_depth
            ),
        });
    }
    pixels.truncate(info.buffer_size());
    Ok((info.width, info.height, pixels))
}

/// Copies one tile-sized image into a mip level at `origin`.
fn blit(atlas: &mut MipLevel, origin: (u32, u32), tile: &[u8], tile_edge: u32) {
    let row_bytes = tile_edge as usize * 4;
    let stride = atlas.width as usize * 4;
    let start = origin.1 as usize * stride + origin.0 as usize * 4;
    for (row, source) in tile.chunks_exact(row_bytes).enumerate() {
        let destination = start + row * stride;
        atlas.pixels[destination..destination + row_bytes].copy_from_slice(source);
    }
}

/// Box-filters a square RGBA8 image to half its edge, rounding to nearest.
fn downsample(source: &[u8], edge: u32) -> Vec<u8> {
    let edge = edge as usize;
    let next = edge / 2;
    let mut output = vec![0; next * next * 4];
    for y in 0..next {
        for x in 0..next {
            let top = ((y * 2) * edge + x * 2) * 4;
            let bottom = top + edge * 4;
            let mut texel = [0; 4];
            for (channel, value) in texel.iter_mut().enumerate() {
                let sum = source[top + channel] as u16
                    + source[top + 4 + channel] as u16
                    + source[bottom + channel] as u16
                    + source[bottom + 4 + channel] as u16;
                *value = ((sum + 2) / 4) as u8;
            }
            let destination = (y * next + x) * 4;
            output[destination..destination + 4].copy_from_slice(&texel);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(color: [u8; 4]) -> [u8; TILE_BYTES] {
        let mut tile = [0; TILE_BYTES];
        for texel in tile.as_chunks_mut::<4>().0 {
            *texel = color;
        }
        tile
    }

    #[test]
    fn atlas_has_the_locked_size_and_mip_count() {
        let atlas = AtlasBuilder::new().build();

        assert_eq!(ATLAS_SIZE, 256);
        assert_eq!(TILE_SIZE, 16);
        assert_eq!(ATLAS_BYTES, 256 * 256 * 4);
        assert_eq!(atlas.mip_count(), 5);
        assert_eq!(atlas.mip_count(), MIP_LEVELS);

        let edges: Vec<u32> = atlas.levels().iter().map(MipLevel::width).collect();
        assert_eq!(edges, [256, 128, 64, 32, 16]);
        for level in atlas.levels() {
            assert_eq!(level.width(), level.height());
            assert_eq!(
                level.pixels().len(),
                (level.width() * level.height() * 4) as usize
            );
        }
        assert_eq!(atlas.byte_len(), 349_184);
    }

    #[test]
    fn all_256_tiles_are_addressable_at_every_level() {
        let mut builder = AtlasBuilder::new();
        for id in 0..MAX_MATERIALS {
            let color = [id as u8, (id >> 8) as u8, 0x80, 0xff];
            builder
                .set_tile(id as u16, &solid(color))
                .expect("every id fits the atlas");
        }
        let atlas = builder.build();
        let base = atlas.level(0).unwrap();
        let deepest = atlas.level(MAX_MIP).unwrap();

        for id in 0..MAX_MATERIALS {
            let color = [id as u8, (id >> 8) as u8, 0x80, 0xff];
            let (x, y) = tile_origin(id as u16);
            assert_eq!(base.texel(x, y), Some(color), "tile {id} base origin");
            assert_eq!(
                deepest.texel(x >> MAX_MIP, y >> MAX_MIP),
                Some(color),
                "tile {id} deepest mip"
            );
        }
    }

    #[test]
    fn no_tile_crosses_a_mip_boundary() {
        for level in 0..=MAX_MIP {
            let atlas_edge = ATLAS_SIZE >> level;
            let tile_edge = TILE_SIZE >> level;
            assert_eq!(atlas_edge % tile_edge, 0);

            for id in 0..MAX_MATERIALS {
                let (x, y) = tile_origin(id as u16);
                let (x, y) = (x >> level, y >> level);
                assert_eq!(x % tile_edge, 0, "tile {id} at level {level} is misaligned");
                assert_eq!(y % tile_edge, 0, "tile {id} at level {level} is misaligned");
                assert!(
                    x + tile_edge <= atlas_edge,
                    "tile {id} overflows level {level}"
                );
                assert!(
                    y + tile_edge <= atlas_edge,
                    "tile {id} overflows level {level}"
                );
            }
        }
    }

    #[test]
    fn mips_do_not_bleed_between_tiles() {
        let mut builder = AtlasBuilder::new();
        builder.set_tile(0, &solid([255, 0, 0, 255])).unwrap();
        builder.set_tile(1, &solid([0, 0, 255, 255])).unwrap();
        let atlas = builder.build();
        let level = atlas.level(1).unwrap();

        // Level 1: tile 0 is 8×8 at (0,0), tile 1 is 8×8 at (8,0), and tile 2
        // stays transparent at (16,0).
        assert_eq!(level.texel(7, 0), Some([255, 0, 0, 255]));
        assert_eq!(level.texel(8, 0), Some([0, 0, 255, 255]));
        assert_eq!(level.texel(16, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn downsample_averages_each_2x2_block() {
        let mut tile = [0; TILE_BYTES];
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let value = if x < 8 { 0 } else { 255 };
                let offset = ((y * TILE_SIZE + x) * 4) as usize;
                tile[offset..offset + 4].copy_from_slice(&[value, 0, 0, 255]);
            }
        }

        let mip = downsample(&tile, TILE_SIZE);
        assert_eq!(mip.len(), 8 * 8 * 4);
        assert_eq!(&mip[..4], &[0, 0, 0, 255]);
        assert_eq!(&mip[(3 * 4)..(3 * 4 + 4)], &[0, 0, 0, 255]);
        assert_eq!(&mip[(4 * 4)..(4 * 4 + 4)], &[255, 0, 0, 255]);
        assert_eq!(&mip[(7 * 4)..(7 * 4 + 4)], &[255, 0, 0, 255]);
    }

    #[test]
    fn committed_atlas_verifies_against_its_hash_and_manifest() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/atlas/atlas.png");
        let atlas = Atlas::load_verified(&path).expect("committed atlas should verify");
        assert_eq!(atlas.mip_count(), MIP_LEVELS);
        assert_eq!(atlas.byte_len(), 349_184);
    }

    #[test]
    fn out_of_range_tiles_are_rejected() {
        let mut builder = AtlasBuilder::new();

        assert_eq!(
            builder.set_tile(MAX_MATERIALS as u16, &solid([0; 4])),
            Err(AtlasError::TileOutOfRange(MAX_MATERIALS as u16))
        );
        assert!(builder.set_tile(0, &solid([1; 4])).is_ok());
    }
}
