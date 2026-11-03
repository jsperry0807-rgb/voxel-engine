//! Material atlas construction: 16×16 tiles, one texel-pure mip chain.
//!
//! The layout is owned by [`voxel_core::atlas`] and re-exported here. The
//! committed atlas is 256×256 with 16×16 tiles; [`AtlasLayout`] can carry the
//! same 16×16-slot model at 32², 64², 128² or 256² texels per tile for the
//! runtime density option (`VOXEL_VIEWER_TILE`).
//!
//! The builder turns 256 tiles into the base image plus one mip level per
//! halving. Mips are filtered **per tile**, never across tile boundaries, so
//! every tile stays whole at every level and the deepest level is exactly one
//! texel per tile. For the committed 16² layout that is:
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
    ATLAS_SIZE, AtlasLayout, MAX_MATERIALS, MAX_MIP, MIP_LEVELS, TILE_SIZE, TILE_SIZES,
    TILES_PER_EDGE, is_tile_size, tile_origin, uv,
};

/// RGBA8 bytes in one 16×16 tile (the committed density).
pub const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 4) as usize;
/// RGBA8 bytes in the base 256×256 atlas image (the committed density).
pub const ATLAS_BYTES: usize = (ATLAS_SIZE * ATLAS_SIZE * 4) as usize;

/// A failure while assembling an atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasError {
    /// The tile id is outside the 16×16 address space.
    TileOutOfRange(u16),
    /// The tile pixels are not one tile of the requested density.
    TileSize { expected: usize, actual: usize },
}

impl fmt::Display for AtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TileOutOfRange(tile) => {
                write!(f, "tile {tile} is outside 0..{MAX_MATERIALS}")
            }
            Self::TileSize { expected, actual } => write!(
                f,
                "tile is {actual} RGBA8 bytes, expected {expected} for this density"
            ),
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
    layout: AtlasLayout,
    tiles: Vec<Vec<u8>>,
}

impl AtlasBuilder {
    /// Creates a builder with 256 empty tiles at the committed 16² density.
    pub fn new() -> Self {
        Self::with_layout(AtlasLayout::DEFAULT)
    }

    /// Creates a builder with 256 empty tiles at `layout`'s density.
    pub fn with_layout(layout: AtlasLayout) -> Self {
        Self {
            layout,
            tiles: vec![vec![0; layout.tile_bytes()]; MAX_MATERIALS as usize],
        }
    }

    /// The density this builder is filling.
    pub fn layout(&self) -> AtlasLayout {
        self.layout
    }

    /// Copies one `tile_size × tile_size` RGBA8 tile into `tile`.
    ///
    /// # Errors
    ///
    /// Returns [`AtlasError::TileOutOfRange`] when `tile` is not a slot in the
    /// 16×16 grid, or [`AtlasError::TileSize`] when `pixels` is not one tile of
    /// this builder's density.
    pub fn set_tile(&mut self, tile: u16, pixels: &[u8]) -> Result<(), AtlasError> {
        let expected = self.layout.tile_bytes();
        if pixels.len() != expected {
            return Err(AtlasError::TileSize {
                expected,
                actual: pixels.len(),
            });
        }
        let slot = self
            .tiles
            .get_mut(tile as usize)
            .ok_or(AtlasError::TileOutOfRange(tile))?;
        slot.copy_from_slice(pixels);
        Ok(())
    }

    /// Builds the base image and its per-tile mip levels.
    pub fn build(self) -> Atlas {
        let layout = self.layout;
        let atlas_edge = layout.atlas_size();
        let mut levels: Vec<MipLevel> = (0..layout.mip_count())
            .map(|level| {
                let edge = atlas_edge >> level;
                MipLevel {
                    width: edge,
                    height: edge,
                    pixels: vec![0; (edge * edge * 4) as usize],
                }
            })
            .collect();

        for (tile, pixels) in self.tiles.iter().enumerate() {
            let (origin_x, origin_y) = layout.tile_origin(tile as u16);
            let mut mip = pixels.clone();
            let mut tile_edge = layout.tile_size();
            for (level, atlas) in levels.iter_mut().enumerate() {
                blit(
                    atlas,
                    (origin_x >> level, origin_y >> level),
                    &mip,
                    tile_edge,
                );
                if level + 1 < layout.mip_count() as usize {
                    mip = downsample(&mip, tile_edge);
                    tile_edge /= 2;
                }
            }
        }

        Atlas { layout, levels }
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
    /// Builds one level from a row-major RGBA8 buffer.
    ///
    /// Used by the viewer to adopt the texgraph bake without copying through a
    /// PNG.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, AtlasLoadError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|texels| texels.checked_mul(4))
            .unwrap_or(usize::MAX);
        if pixels.len() != expected {
            return Err(AtlasLoadError::Format {
                path: PathBuf::from("atlas level"),
                message: format!(
                    "{width}x{height} needs {expected} RGBA8 bytes, got {}",
                    pixels.len()
                ),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

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

/// The finished atlas: base image plus the per-tile mip chain.
#[derive(Clone, Debug)]
pub struct Atlas {
    layout: AtlasLayout,
    levels: Vec<MipLevel>,
}

impl Atlas {
    /// Wraps already-built levels for `layout`.
    ///
    /// The levels must start at `layout.atlas_size()` and halve each time, with
    /// `layout.mip_count()` entries. This is the path the viewer uses for a
    /// density built from the committed 256² sources.
    pub fn from_levels(layout: AtlasLayout, levels: Vec<MipLevel>) -> Result<Self, AtlasLoadError> {
        if levels.len() != layout.mip_count() as usize {
            return Err(AtlasLoadError::Format {
                path: PathBuf::from("atlas levels"),
                message: format!(
                    "{} levels for {}-texel tiles, expected {}",
                    levels.len(),
                    layout.tile_size(),
                    layout.mip_count()
                ),
            });
        }
        let mut expected = layout.atlas_size();
        for level in &levels {
            if level.width != expected || level.height != expected {
                return Err(AtlasLoadError::Format {
                    path: PathBuf::from("atlas levels"),
                    message: format!(
                        "level is {}x{}, expected {expected}x{expected}",
                        level.width, level.height
                    ),
                });
            }
            expected = (expected / 2).max(1);
        }
        Ok(Self { layout, levels })
    }

    /// Loads `atlas-0.png` through the last mip level from `dir`.
    ///
    /// This is the local texture-set path (for example
    /// `assets/atlas/resourcepack`, written by `texgen bake-set`). The files
    /// are RGBA8 and each level must be the expected size. The density is read
    /// from the base image, so a set baked at a larger tile still loads.
    pub fn load_dir(dir: &Path) -> Result<Self, AtlasLoadError> {
        let base = dir.join("atlas-0.png");
        let (width, height, _) = decode_png(&base)?;
        if width != height || width % TILES_PER_EDGE != 0 || !is_tile_size(width / TILES_PER_EDGE) {
            return Err(AtlasLoadError::Format {
                path: base,
                message: format!(
                    "{width}x{height} is not a square atlas of {TILES_PER_EDGE} tiles at one of {TILE_SIZES:?}"
                ),
            });
        }
        let layout = AtlasLayout::new(width / TILES_PER_EDGE);
        let mut levels = Vec::with_capacity(layout.mip_count() as usize);
        for level in 0..layout.mip_count() {
            let path = dir.join(format!("atlas-{level}.png"));
            let edge = level_edge(layout.atlas_size(), level);
            let (width, height, pixels) = decode_png(&path)?;
            if width != edge || height != edge {
                return Err(AtlasLoadError::Dimensions {
                    path,
                    width,
                    height,
                    expected: edge,
                });
            }
            levels.push(MipLevel {
                width,
                height,
                pixels,
            });
        }
        Self::from_levels(layout, levels)
    }

    /// Loads the base `atlas.png` and regenerates the per-tile mip levels.
    ///
    /// The committed format stores only the base image. The density follows the
    /// image edge, so a 4096² atlas is accepted when it is a supported tile
    /// size.
    pub fn load(path: &Path) -> Result<Self, AtlasLoadError> {
        let (width, height, pixels) = decode_png(path)?;
        if width != height || width % TILES_PER_EDGE != 0 || !is_tile_size(width / TILES_PER_EDGE) {
            return Err(AtlasLoadError::Format {
                path: path.to_owned(),
                message: format!(
                    "{width}x{height} is not a square atlas of {TILES_PER_EDGE} tiles at one of {TILE_SIZES:?}"
                ),
            });
        }
        let layout = AtlasLayout::new(width / TILES_PER_EDGE);
        let tile_size = layout.tile_size() as usize;
        let mut builder = AtlasBuilder::with_layout(layout);
        let mut tile_pixels = vec![0u8; layout.tile_bytes()];
        for tile in 0..MAX_MATERIALS {
            let (origin_x, origin_y) = layout.tile_origin(tile as u16);
            for row in 0..tile_size {
                let source = ((origin_y as usize + row) * width as usize + origin_x as usize) * 4;
                tile_pixels[row * tile_size * 4..(row + 1) * tile_size * 4]
                    .copy_from_slice(&pixels[source..source + tile_size * 4]);
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
        if atlas.tile_size() != TILE_SIZE {
            return Err(AtlasLoadError::Hash(format!(
                "{}: only the committed {TILE_SIZE}-texel atlas has a committed hash",
                path.display()
            )));
        }
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
            .map(|directory| directory.join("../../../../crates/engine/block/data/textures.ron"))
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

    /// The density this atlas was built at.
    pub fn layout(&self) -> AtlasLayout {
        self.layout
    }

    /// Texels per tile edge.
    pub fn tile_size(&self) -> u32 {
        self.layout.tile_size()
    }

    /// Texels per atlas edge (`16 × tile_size`).
    pub fn atlas_size(&self) -> u32 {
        self.layout.atlas_size()
    }

    /// Number of levels including the base image.
    pub fn mip_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// All levels, base first.
    pub fn levels(&self) -> &[MipLevel] {
        &self.levels
    }

    /// One level, or `None` past [`AtlasLayout::max_mip`].
    pub fn level(&self, level: u32) -> Option<&MipLevel> {
        self.levels.get(level as usize)
    }

    /// Total RGBA8 bytes across every level.
    pub fn byte_len(&self) -> usize {
        self.levels.iter().map(|level| level.pixels.len()).sum()
    }
}

/// Edge of mip level `level`, never below one texel.
fn level_edge(atlas_edge: u32, level: u32) -> u32 {
    (atlas_edge >> level).max(1)
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
    fn density_layout_sets_the_atlas_edge_and_mip_count() {
        let layout = AtlasLayout::new(64);
        let atlas = AtlasBuilder::with_layout(layout).build();

        assert_eq!(atlas.tile_size(), 64);
        assert_eq!(atlas.atlas_size(), 1024);
        assert_eq!(atlas.mip_count(), 7);
        assert_eq!(atlas.layout(), layout);

        let edges: Vec<u32> = atlas.levels().iter().map(MipLevel::width).collect();
        assert_eq!(edges, [1024, 512, 256, 128, 64, 32, 16]);
        assert_eq!(
            atlas.byte_len(),
            (1024 * 1024 + 512 * 512 + 256 * 256 + 128 * 128 + 64 * 64 + 32 * 32 + 16 * 16) * 4
        );

        let deepest = atlas.level(layout.max_mip()).unwrap();
        for id in [0u16, 1, MAX_MATERIALS as u16 - 1] {
            let (x, y) = layout.tile_origin(id);
            assert_eq!(
                deepest.texel(x >> layout.max_mip(), y >> layout.max_mip()),
                Some([0, 0, 0, 0]),
                "tile {id} deepest mip"
            );
        }
    }

    #[test]
    fn set_tile_rejects_pixels_from_another_density() {
        let mut builder = AtlasBuilder::with_layout(AtlasLayout::new(64));
        assert_eq!(
            builder.set_tile(0, &solid([1; 4])),
            Err(AtlasError::TileSize {
                expected: 64 * 64 * 4,
                actual: TILE_BYTES,
            })
        );
        let tile = vec![9u8; 64 * 64 * 4];
        assert!(builder.set_tile(0, &tile).is_ok());
    }

    #[test]
    fn from_levels_rejects_a_mismatched_chain() {
        let layout = AtlasLayout::new(32);
        let atlas = AtlasBuilder::with_layout(layout).build();
        let levels = atlas.levels().to_vec();

        assert!(Atlas::from_levels(layout, levels.clone()).is_ok());
        assert!(Atlas::from_levels(AtlasLayout::new(16), levels.clone()).is_err());
        assert!(Atlas::from_levels(layout, levels[..3].to_vec()).is_err());
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
