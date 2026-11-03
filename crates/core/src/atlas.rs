//! Material atlas layout: one 256×256 texture split into a 16×16 grid of tiles.
//!
//! A tile is one block face: 16×16 texels for the 16 voxels along a block edge,
//! so one voxel face reads one texel (6.25 cm). The tile index is the material
//! id (`tileIndex = materialId`), and the atlas holds exactly [`MAX_MATERIALS`]
//! materials. Per-face variants such as `grass_top` and `grass_side` are
//! separate materials and therefore separate tiles.
//!
//! Tiles are laid out row-major. Tile `id` starts at texel
//! `((id % TILES_PER_EDGE) * TILE_SIZE, (id / TILES_PER_EDGE) * TILE_SIZE)`.
//!
//! [`uv`] maps a normalized position on a face (`0.0..=1.0` on each axis) to
//! normalized atlas coordinates. The result is inset by half a texel, so a
//! bilinear sample at a tile edge cannot reach the neighbouring tile:
//!
//! ```text
//! uv = (tileOrigin + clamp(facePos * TILE_SIZE + 0.5, 0.5, TILE_SIZE - 0.5))
//!      / ATLAS_SIZE
//! ```
//!
//! Mip generation stops at [`MAX_MIP`] (level 4), where a 16×16 tile has shrunk
//! to 1×1. Deeper levels would average texels from different tiles.
//!
//! # Runtime tile densities
//!
//! The layout above is the committed default. [`AtlasLayout`] expresses the
//! same 16×16-slot model at any edge in [`TILE_SIZES`], so the viewer can build
//! 32², 64², 128² or 256² tiles from the committed 256² sources without
//! changing material ids, mesh data or the shader's tile addressing. Only the
//! atlas edge (`16 × tile_size`) and the mip count grow.
//!
//! # Example
//!
//! ```
//! use voxel_core::atlas::{tile_origin, uv, ATLAS_SIZE, TILE_SIZE};
//!
//! assert_eq!(tile_origin(0), (0, 0));
//! assert_eq!(tile_origin(17), (TILE_SIZE, TILE_SIZE));
//!
//! let (origin_x, _) = tile_origin(17);
//! let (u, v) = uv(17, (0.5, 0.5));
//! assert_eq!(u, v);
//! assert!(u > origin_x as f32 / ATLAS_SIZE as f32);
//! assert!(u < (origin_x + TILE_SIZE) as f32 / ATLAS_SIZE as f32);
//! ```

/// Edge length of the material atlas in texels.
pub const ATLAS_SIZE: u32 = 256;

/// Edge length of one material tile in texels.
pub const TILE_SIZE: u32 = 16;

/// Material tiles along one atlas edge.
pub const TILES_PER_EDGE: u32 = ATLAS_SIZE / TILE_SIZE;

/// Number of material slots in the atlas.
///
/// A material id is also its tile index, so ids must be dense and smaller than
/// this cap.
pub const MAX_MATERIALS: u32 = TILES_PER_EDGE * TILES_PER_EDGE;

/// Deepest mip level the atlas may generate.
///
/// Level 0 is the full 256×256 image and level 4 is 16×16, where each tile is
/// one texel. Including the base level, the atlas has `MAX_MIP + 1` mip levels.
pub const MAX_MIP: u32 = 4;

/// Number of mip levels in the atlas, including the base level.
pub const MIP_LEVELS: u32 = MAX_MIP + 1;

/// Edge of the committed face sources, in texels.
///
/// Every runtime tile edge divides this exactly, so one source face can feed
/// any [`TILE_SIZES`] density with an integer box filter.
pub const SOURCE_TILE_SIZE: u32 = 256;

/// Tile edges the runtime can build, in texels.
///
/// The committed default is 16 (6.25 cm per texel). Larger tiles keep the same
/// 16×16 grid and the same 256 material slots, so the atlas edge grows to
/// `16 × TILE_SIZES[n]` (256 … 4096) and the shader reads 16× more texels per
/// voxel face at each step.
pub const TILE_SIZES: [u32; 5] = [16, 32, 64, 128, 256];

/// Whether `tile_size` is one of [`TILE_SIZES`].
pub const fn is_tile_size(tile_size: u32) -> bool {
    matches!(tile_size, 16 | 32 | 64 | 128 | 256)
}

/// The layout of one atlas: 16×16 tiles of `tile_size` texels.
///
/// It generalizes the compile-time constants above. [`AtlasLayout::default`]
/// is the committed 16-texel layout, and the runtime can request any
/// [`TILE_SIZES`] edge without changing the slot model: `tileIndex =
/// materialId`, row-major, and every tile stays mip-aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AtlasLayout {
    tile_size: u32,
}

impl AtlasLayout {
    /// The committed layout: 16×16 texels per tile, one 256×256 atlas.
    pub const DEFAULT: Self = Self::new(TILE_SIZE);

    /// A layout with `tile_size` texels per tile edge.
    ///
    /// # Panics
    ///
    /// Panics when `tile_size` is not one of [`TILE_SIZES`]. Parse untrusted
    /// input with [`Self::parse`] instead.
    pub const fn new(tile_size: u32) -> Self {
        assert!(
            is_tile_size(tile_size),
            "tile edge must be one of 16, 32, 64, 128 or 256"
        );
        Self { tile_size }
    }

    /// Parses a tile edge from text, such as an environment variable.
    pub fn parse(text: &str) -> Option<Self> {
        text.trim()
            .parse::<u32>()
            .ok()
            .filter(|edge| is_tile_size(*edge))
            .map(Self::new)
    }

    /// Tile edge in texels.
    pub const fn tile_size(self) -> u32 {
        self.tile_size
    }

    /// Atlas edge in texels: `TILES_PER_EDGE × tile_size`.
    pub const fn atlas_size(self) -> u32 {
        self.tile_size * TILES_PER_EDGE
    }

    /// RGBA8 bytes in one tile.
    pub const fn tile_bytes(self) -> usize {
        (self.tile_size * self.tile_size * 4) as usize
    }

    /// Mip levels including the base level.
    pub const fn mip_count(self) -> u32 {
        let mut count = 1;
        let mut edge = self.tile_size;
        while edge > 1 {
            edge >>= 1;
            count += 1;
        }
        count
    }

    /// Deepest mip level, where every tile is exactly one texel.
    pub const fn max_mip(self) -> u32 {
        self.mip_count() - 1
    }

    /// Upper-left texel of a tile in the atlas, as `(x, y)`.
    pub const fn tile_origin(self, tile: u16) -> (u32, u32) {
        assert!((tile as u32) < MAX_MATERIALS);
        let tile = tile as u32;
        (
            (tile % TILES_PER_EDGE) * self.tile_size,
            (tile / TILES_PER_EDGE) * self.tile_size,
        )
    }

    /// Total RGBA8 bytes across every mip level.
    pub const fn byte_len(self) -> usize {
        let mut bytes = 0;
        let mut level = 0;
        while level < self.mip_count() {
            let edge = self.atlas_size() >> level;
            bytes += (edge * edge * 4) as usize;
            level += 1;
        }
        bytes
    }
}

impl Default for AtlasLayout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

const HALF_TEXEL: f32 = 0.5;
const ATLAS_SIZE_F32: f32 = ATLAS_SIZE as f32;
const TILE_SIZE_F32: f32 = TILE_SIZE as f32;
const LAST_TEXEL_CENTER: f32 = TILE_SIZE_F32 - HALF_TEXEL;

// The atlas layout is a contract with the mesher, the atlas builder and the
// shader. Check that the grid covers the atlas exactly and that the mip cap
// stops while every tile is still at least one texel wide.
const _: () = {
    assert!(ATLAS_SIZE == TILE_SIZE * TILES_PER_EDGE);
    assert!(MAX_MATERIALS == TILES_PER_EDGE * TILES_PER_EDGE);
    assert!(MAX_MATERIALS <= u16::MAX as u32 + 1);

    assert!(MAX_MIP == 4);
    assert!(TILE_SIZE == 1 << MAX_MIP);
    assert!(TILE_SIZE.is_multiple_of(1 << MAX_MIP));
    assert!(MAX_MIP < u32::BITS);
    assert!(MIP_LEVELS == MAX_MIP + 1);

    assert!(HALF_TEXEL > 0.0);
    assert!(LAST_TEXEL_CENTER < TILE_SIZE_F32);
};

/// Upper-left texel of a tile in the atlas, as `(x, y)`.
///
/// Tile ids are row-major and must be in `0..MAX_MATERIALS`; debug builds
/// assert the range. For every valid id the returned origin plus [`TILE_SIZE`]
/// stays inside the atlas, so a tile never wraps or crosses its own boundary.
///
/// ```
/// use voxel_core::atlas::{tile_origin, TILE_SIZE};
///
/// assert_eq!(tile_origin(0), (0, 0));
/// assert_eq!(tile_origin(15), (15 * TILE_SIZE, 0));
/// assert_eq!(tile_origin(16), (0, TILE_SIZE));
/// ```
pub const fn tile_origin(tile: u16) -> (u32, u32) {
    assert!((tile as u32) < MAX_MATERIALS);
    let tile = tile as u32;
    (
        (tile % TILES_PER_EDGE) * TILE_SIZE,
        (tile / TILES_PER_EDGE) * TILE_SIZE,
    )
}

/// Normalized atlas coordinates for a point on a tile.
///
/// `face_pos` is the position on the block face, normalized to `0.0..=1.0` on
/// each axis. Values outside that range are clamped, so the result always lies
/// inside `tile` and inside the atlas. The point is inset by half a texel to
/// keep bilinear filtering away from the neighbouring tile.
///
/// ```
/// use voxel_core::atlas::{uv, ATLAS_SIZE};
///
/// // The centre of tile 0 lands half a texel past its halfway point.
/// let (u, v) = uv(0, (0.5, 0.5));
/// assert_eq!(u, 8.5 / ATLAS_SIZE as f32);
/// assert_eq!(v, u);
/// ```
pub const fn uv(tile: u16, face_pos: (f32, f32)) -> (f32, f32) {
    let (origin_x, origin_y) = tile_origin(tile);
    (axis_uv(origin_x, face_pos.0), axis_uv(origin_y, face_pos.1))
}

const fn axis_uv(origin: u32, face_pos: f32) -> f32 {
    // Written as nested comparisons so a NaN position maps to the tile origin
    // rather than propagating a NaN into the shader.
    let face_pos = if face_pos > 0.0 {
        if face_pos < 1.0 { face_pos } else { 1.0 }
    } else {
        0.0
    };

    // Integer face positions (voxel boundaries) map to texel centres. Clamp
    // the final half texel so the last row/column stays inside the tile.
    let texel = face_pos * TILE_SIZE_F32 + HALF_TEXEL;
    let texel = if texel > LAST_TEXEL_CENTER {
        LAST_TEXEL_CENTER
    } else {
        texel
    };

    (origin as f32 + texel) / ATLAS_SIZE_F32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_constants_match_the_locked_layout() {
        assert_eq!(ATLAS_SIZE, 256);
        assert_eq!(TILE_SIZE, 16);
        assert_eq!(TILES_PER_EDGE, 16);
        assert_eq!(MAX_MATERIALS, 256);
        assert_eq!(MAX_MIP, 4);
        assert_eq!(MIP_LEVELS, MAX_MIP + 1);
        assert_eq!(MIP_LEVELS, 5);
    }

    #[test]
    fn tile_sizes_cover_the_supported_densities() {
        assert_eq!(TILE_SIZES, [16, 32, 64, 128, 256]);
        assert_eq!(AtlasLayout::default(), AtlasLayout::DEFAULT);
        assert_eq!(AtlasLayout::DEFAULT.tile_size(), TILE_SIZE);
        assert_eq!(AtlasLayout::DEFAULT.atlas_size(), ATLAS_SIZE);
        assert_eq!(AtlasLayout::DEFAULT.mip_count(), MIP_LEVELS);
        assert_eq!(AtlasLayout::DEFAULT.max_mip(), MAX_MIP);
        assert_eq!(AtlasLayout::DEFAULT.byte_len(), 349_184);

        for (edge, atlas_edge, mips) in [
            (16, 256, 5),
            (32, 512, 6),
            (64, 1024, 7),
            (128, 2048, 8),
            (256, 4096, 9),
        ] {
            let layout = AtlasLayout::new(edge);
            assert_eq!(layout.atlas_size(), atlas_edge);
            assert_eq!(layout.mip_count(), mips);
            assert_eq!(layout.max_mip(), mips - 1);
            assert_eq!(layout.tile_bytes(), (edge * edge * 4) as usize);
            assert_eq!(SOURCE_TILE_SIZE % edge, 0, "source does not divide evenly");
        }
    }

    #[test]
    fn tile_sizes_parse_and_reject_unknown_edges() {
        assert_eq!(AtlasLayout::parse("16"), Some(AtlasLayout::DEFAULT));
        assert_eq!(AtlasLayout::parse(" 256 "), Some(AtlasLayout::new(256)));
        for rejected in ["", "0", "8", "24", "512", "wide", "-16"] {
            assert_eq!(AtlasLayout::parse(rejected), None, "{rejected:?}");
        }
    }

    #[test]
    fn every_density_keeps_tiles_row_major_and_mip_aligned() {
        for edge in TILE_SIZES {
            let layout = AtlasLayout::new(edge);
            assert_eq!(layout.tile_origin(0), (0, 0));
            assert_eq!(layout.tile_origin(15), (15 * edge, 0));
            assert_eq!(layout.tile_origin(16), (0, edge));
            assert_eq!(
                layout.tile_origin((MAX_MATERIALS - 1) as u16),
                (15 * edge, 15 * edge)
            );

            for level in 0..=layout.max_mip() {
                let atlas_edge = layout.atlas_size() >> level;
                let tile_edge = edge >> level;
                assert!(tile_edge >= 1, "{edge}px tiles vanish at level {level}");
                assert_eq!(atlas_edge % tile_edge, 0);

                for tile in 0..MAX_MATERIALS {
                    let (x, y) = layout.tile_origin(tile as u16);
                    let (x, y) = (x >> level, y >> level);
                    assert_eq!(x % tile_edge, 0);
                    assert_eq!(y % tile_edge, 0);
                    assert!(x + tile_edge <= atlas_edge);
                    assert!(y + tile_edge <= atlas_edge);
                }
            }
        }
    }

    #[test]
    fn atlas_tile_origins_are_row_major() {
        assert_eq!(tile_origin(0), (0, 0));
        assert_eq!(tile_origin(1), (TILE_SIZE, 0));
        assert_eq!(tile_origin(TILES_PER_EDGE as u16 - 1), (240, 0));
        assert_eq!(tile_origin(TILES_PER_EDGE as u16), (0, TILE_SIZE));
        assert_eq!(tile_origin((MAX_MATERIALS - 1) as u16), (240, 240));
    }

    #[test]
    fn atlas_tile_origins_cover_every_slot_in_bounds() {
        let mut seen = [false; MAX_MATERIALS as usize];

        for tile in 0..MAX_MATERIALS {
            let (x, y) = tile_origin(tile as u16);

            assert!(x < ATLAS_SIZE, "tile {tile} starts past the atlas");
            assert!(y < ATLAS_SIZE, "tile {tile} starts past the atlas");
            assert!(
                x + TILE_SIZE <= ATLAS_SIZE,
                "tile {tile} wraps horizontally"
            );
            assert!(y + TILE_SIZE <= ATLAS_SIZE, "tile {tile} wraps vertically");
            assert_eq!(x % TILE_SIZE, 0, "tile {tile} is not tile-aligned");
            assert_eq!(y % TILE_SIZE, 0, "tile {tile} is not tile-aligned");

            let column = x / TILE_SIZE;
            let row = y / TILE_SIZE;
            let slot = (row * TILES_PER_EDGE + column) as usize;
            assert_eq!(slot, tile as usize);
            assert!(!seen[slot], "tile {tile} shares an origin");
            seen[slot] = true;
        }

        assert!(seen.iter().all(|covered| *covered));
    }

    #[test]
    fn atlas_uv_stays_inside_its_tile() {
        // Corners, interior points and out-of-range positions all have to stay
        // within the tile and within normalized atlas coordinates.
        let positions = [
            (-1.0, -1.0),
            (0.0, 0.0),
            (0.25, 0.75),
            (0.5, 0.5),
            (1.0, 1.0),
            (2.0, 2.0),
        ];

        for tile in 0..MAX_MATERIALS {
            let (origin_x, origin_y) = tile_origin(tile as u16);
            let min_u = origin_x as f32 / ATLAS_SIZE as f32;
            let min_v = origin_y as f32 / ATLAS_SIZE as f32;
            let max_u = (origin_x + TILE_SIZE) as f32 / ATLAS_SIZE as f32;
            let max_v = (origin_y + TILE_SIZE) as f32 / ATLAS_SIZE as f32;

            for (x, y) in positions {
                let (u, v) = uv(tile as u16, (x, y));

                assert!(
                    (0.0..1.0).contains(&u),
                    "tile {tile}: u={u} is outside [0,1)"
                );
                assert!(
                    (0.0..1.0).contains(&v),
                    "tile {tile}: v={v} is outside [0,1)"
                );
                assert!(
                    u >= min_u && u < max_u,
                    "tile {tile}: u={u} leaves the tile"
                );
                assert!(
                    v >= min_v && v < max_v,
                    "tile {tile}: v={v} leaves the tile"
                );

                // The half-texel inset keeps sampling off the tile boundary.
                assert!(u >= (origin_x as f32 + HALF_TEXEL) / ATLAS_SIZE_F32);
                assert!(u <= (origin_x as f32 + LAST_TEXEL_CENTER) / ATLAS_SIZE_F32);
                assert!(v >= (origin_y as f32 + HALF_TEXEL) / ATLAS_SIZE_F32);
                assert!(v <= (origin_y as f32 + LAST_TEXEL_CENTER) / ATLAS_SIZE_F32);
            }
        }
    }

    #[test]
    fn atlas_uv_clamps_out_of_range_face_positions() {
        for tile in 0..MAX_MATERIALS {
            let tile = tile as u16;
            assert_eq!(uv(tile, (-1.0, -1.0)), uv(tile, (0.0, 0.0)));
            assert_eq!(uv(tile, (2.0, 2.0)), uv(tile, (1.0, 1.0)));

            // Both axes use the same mapping, independently.
            let (u, v) = uv(tile, (0.25, 0.25));
            let (u_axis, _) = uv(tile, (0.25, 0.0));
            let (_, v_axis) = uv(tile, (0.0, 0.25));
            assert_eq!(u, u_axis);
            assert_eq!(v, v_axis);
        }
    }

    #[test]
    fn atlas_uv_lands_on_texel_centres_at_voxel_boundaries() {
        // A block face is 16 voxels wide; each 1/16 step should advance by one
        // texel centre (0.5, 1.5, ... 15.5), not by a texel boundary.
        let (origin_x, _) = tile_origin(0);
        for voxel in 0..TILE_SIZE {
            let face_pos = voxel as f32 / TILE_SIZE as f32;
            let (u, _) = uv(0, (face_pos, 0.0));
            let expected = (origin_x as f32 + HALF_TEXEL + voxel as f32) / ATLAS_SIZE_F32;
            assert!((u - expected).abs() < f32::EPSILON, "voxel {voxel}");
        }
    }

    #[test]
    fn atlas_mip_cap_keeps_every_tile_whole() {
        // At the cap a tile is exactly 1x1, so no deeper level can be built
        // without averaging across tiles.
        assert_eq!(TILE_SIZE >> MAX_MIP, 1);
        assert_eq!(TILE_SIZE % (1 << MAX_MIP), 0);
        assert_eq!(1 << MAX_MIP, 16);

        for tile in 0..MAX_MATERIALS {
            let (x, y) = tile_origin(tile as u16);
            assert_eq!(x % (1 << MAX_MIP), 0, "tile {tile} crosses a mip boundary");
            assert_eq!(y % (1 << MAX_MIP), 0, "tile {tile} crosses a mip boundary");
        }
    }

    /// The helpers are `const`, so buffer sizes and shader constants can use
    /// them at compile time.
    #[test]
    fn atlas_helpers_work_in_const_context() {
        const ORIGIN: (u32, u32) = tile_origin(255);
        const UV: (f32, f32) = uv(0, (0.0, 0.0));

        assert_eq!(ORIGIN, (240, 240));
        assert_eq!(
            UV,
            (HALF_TEXEL / ATLAS_SIZE_F32, HALF_TEXEL / ATLAS_SIZE_F32)
        );
    }
}
