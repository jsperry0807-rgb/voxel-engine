//! Static per-block data: how a block looks and how it behaves.
//!
//! [`BlockProperties`] are assigned once at startup (see
//! [`crate::BlockRegistryBuilder`]) and then shared immutably by the meshing,
//! lighting and gameplay systems. Nothing here is per-position state — that
//! lives in chunk storage.

use core::fmt;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

use crate::id::NamespacedKey;
use crate::texture::FaceTextureIds;
pub use voxel_core::Face;

/// How a block's faces are rasterized.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Default, serde::Serialize, serde::Deserialize,
)]
pub enum RenderType {
    /// Opaque faces that may be merged with neighboring opaque faces
    /// (stone, dirt).
    #[default]
    Solid,
    /// Binary alpha; faces are not merged with neighbors but need no sorting
    /// (leaves, glass, fences).
    Cutout,
    /// Alpha blended with the frame buffer; faces need back-to-front sorting
    /// (water, ice, stained glass).
    Translucent,
    /// Never rasterized (air, markers).
    Invisible,
}

impl RenderType {
    /// Stable 3-bit encoding used by [`crate::HotRecord`]. Explicit rather than
    /// `as u8`, so reordering the enum can never change the packed format.
    pub const fn to_bits(self) -> u32 {
        match self {
            RenderType::Solid => 0,
            RenderType::Cutout => 1,
            RenderType::Translucent => 2,
            RenderType::Invisible => 3,
        }
    }

    /// Inverse of [`Self::to_bits`]. Unknown encodings read as `Invisible`.
    pub const fn from_bits(bits: u32) -> Self {
        match bits {
            0 => RenderType::Solid,
            1 => RenderType::Cutout,
            2 => RenderType::Translucent,
            _ => RenderType::Invisible,
        }
    }
}

/// Texture-array layer keys for each face of a block.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FaceTextures {
    pub top: NamespacedKey,
    pub bottom: NamespacedKey,
    pub north: NamespacedKey,
    pub south: NamespacedKey,
    pub east: NamespacedKey,
    pub west: NamespacedKey,
}

impl FaceTextures {
    /// Uses the same texture on every face (stone, glass).
    pub fn all(texture: NamespacedKey) -> Self {
        Self {
            top: texture.clone(),
            bottom: texture.clone(),
            north: texture.clone(),
            south: texture.clone(),
            east: texture.clone(),
            west: texture,
        }
    }

    /// A vertical column with distinct top and bottom caps (logs, grass).
    pub fn column(top: NamespacedKey, side: NamespacedKey, bottom: NamespacedKey) -> Self {
        Self {
            north: side.clone(),
            south: side.clone(),
            east: side.clone(),
            west: side,
            top,
            bottom,
        }
    }

    /// Texture for a given face.
    pub const fn get(&self, face: Face) -> &NamespacedKey {
        match face {
            Face::North => &self.north,
            Face::South => &self.south,
            Face::East => &self.east,
            Face::West => &self.west,
            Face::Top => &self.top,
            Face::Bottom => &self.bottom,
        }
    }
}

/// Bitset of coarse behavioral flags.
///
/// These are deliberately few and orthogonal; anything that needs richer data
/// (tool tiers, armor values) belongs in a gameplay-side component table keyed
/// by [`crate::MaterialId`].
///
/// `#[repr(transparent)]` keeps the in-memory layout identical to the raw
/// `u32`, so packed block data can be written straight to disk or network
/// buffers.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct BlockFlags(u32);

impl BlockFlags {
    pub const NONE: Self = Self(0);
    /// Has a full collision box (stone, planks). Absent for air, liquids and
    /// decorations.
    pub const SOLID: Self = Self(1 << 0);
    /// Fully hides neighbor faces and blocks light (stone). Absent for leaves,
    /// glass and liquids.
    pub const OPAQUE: Self = Self(1 << 1);
    /// Can be overwritten by placement without breaking it first (air, grass).
    pub const REPLACEABLE: Self = Self(1 << 2);
    /// Behaves as a fluid (water, lava).
    pub const LIQUID: Self = Self(1 << 3);
    /// Faces between two blocks of the same kind are culled (glass, water).
    /// Without this, adjacent glass panes would draw the hidden interface.
    pub const CULLS_SELF: Self = Self(1 << 4);

    /// The flag bits a [`crate::HotRecord`] carries: the low 16.
    ///
    /// Bits 16..32 are *cold-only*: they survive in [`BlockProperties::flags`]
    /// but are not copied into the hot table. Define a flag a raymarcher or
    /// mesher must read in the low 16 bits; put pure gameplay flags above.
    pub const HOT_MASK: Self = Self(0xFFFF);

    /// True if every flag in `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True if no flags are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Raw bit representation, for storage or logging.
    pub const fn as_bits(self) -> u32 {
        self.0
    }

    /// Build flags from a raw bit representation.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
}

impl BitOr for BlockFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for BlockFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for BlockFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for BlockFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for BlockFlags {
    type Output = Self;

    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl fmt::Debug for BlockFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMED: [(BlockFlags, &str); 5] = [
            (BlockFlags::SOLID, "SOLID"),
            (BlockFlags::OPAQUE, "OPAQUE"),
            (BlockFlags::REPLACEABLE, "REPLACEABLE"),
            (BlockFlags::LIQUID, "LIQUID"),
            (BlockFlags::CULLS_SELF, "CULLS_SELF"),
        ];

        let mut first = true;
        for (flag, name) in NAMED {
            if self.contains(flag) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        if first {
            write!(f, "BlockFlags({:#x})", self.0)?;
        }
        Ok(())
    }
}

/// Immutable data describing a block type.
///
/// Constructed with [`BlockProperties::new`] and tuned through the `with_*`
/// chainable setters, e.g.:
///
/// ```
/// use voxel_block::{BlockFlags, BlockProperties, NamespacedKey, RenderType};
///
/// let stone = BlockProperties::new()
///     .with_render_type(RenderType::Solid)
///     .with_textures(voxel_block::FaceTextures::all(NamespacedKey::builtin(
///         "block/stone",
///     )))
///     .with_hardness(1.5)
///     .with_flags(BlockFlags::SOLID | BlockFlags::OPAQUE);
/// ```
#[derive(Clone, PartialEq, Debug)]
pub struct BlockProperties {
    /// Human-readable name for UIs; derived from the key when unset.
    pub display_name: String,
    /// Rasterization mode.
    pub render_type: RenderType,
    /// Texture-array layer keys per face; `None` for blocks that never draw.
    pub textures: Option<FaceTextures>,
    /// Root node of a 16x16x16 sub-block template in the SVDAG, or `None` for
    /// a flat, uniform material.
    ///
    /// A template is copied into the chunk's SVDAG when this block is placed;
    /// a uniform material expands to its single [`crate::MaterialId`] instead.
    pub template_root: Option<u64>,
    /// Coarse behavioral flags.
    pub flags: BlockFlags,
    /// Light emitted by the block, `0..=15`.
    pub light_emission: u8,
    /// Light absorbed per block crossed, `0..=15` (0 for air, 15 for a full
    /// blocker).
    pub light_opacity: u8,
    /// Seconds to break by hand; negative means unbreakable (bedrock).
    pub hardness: f32,
}

impl Default for BlockProperties {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockProperties {
    /// Air-like defaults: invisible, no collision, no light interaction.
    pub fn new() -> Self {
        Self {
            display_name: String::new(),
            render_type: RenderType::Invisible,
            textures: None,
            template_root: None,
            flags: BlockFlags::NONE,
            light_emission: 0,
            light_opacity: 0,
            hardness: 0.0,
        }
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    pub fn with_render_type(mut self, render_type: RenderType) -> Self {
        self.render_type = render_type;
        self
    }

    pub fn with_textures(mut self, textures: FaceTextures) -> Self {
        self.textures = Some(textures);
        self
    }

    /// Sets the SVDAG root of the block's sub-block template.
    pub fn with_template_root(mut self, template_root: u64) -> Self {
        self.template_root = Some(template_root);
        self
    }

    pub fn with_flags(mut self, flags: BlockFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Sets emission and opacity together, the common case for light sources
    /// and blockers.
    pub fn with_light(mut self, emission: u8, opacity: u8) -> Self {
        self.light_emission = emission.min(15);
        self.light_opacity = opacity.min(15);
        self
    }

    pub fn with_hardness(mut self, hardness: f32) -> Self {
        self.hardness = hardness;
        self
    }

    /// True if the block has a collision box.
    pub fn is_solid(&self) -> bool {
        self.flags.contains(BlockFlags::SOLID)
    }

    /// True if the block fully blocks light and neighbor faces.
    pub fn is_opaque(&self) -> bool {
        self.flags.contains(BlockFlags::OPAQUE)
    }
}

/// A block's resolved data, assembled on demand.
///
/// This is a convenience *view*, not the storage. The registry keeps the
/// runtime data split by how it is read:
///
/// - [`crate::HotRecord`] — 8 bytes per block: render type, hot flags, light,
///   and a [`crate::TextureSetId`]. This is what a per-voxel or per-hit loop
///   reads ([`crate::BlockRegistry::hot`]).
/// - texture sets — each distinct six-face set stored once, read only on a
///   surface hit ([`crate::BlockRegistry::texture_set`]).
/// - template roots — sparse, read only at placement
///   ([`crate::BlockRegistry::template_root`]).
///
/// [`crate::BlockRegistry::resolved`] stitches the pieces back into this
/// shape for code that wants everything at once (tooling, tests, placement).
/// Do not call it in a per-voxel loop; use `hot` there.
///
/// Dynamic per-position data, such as wear and directional cracks, does not
/// belong here; it lives in the separate per-chunk `DamageStore`.
///
/// Fields are added here when a consumer needs one, not in anticipation of
/// one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BlockRecord {
    /// Resolved textures, one per face; `None` for blocks that never draw.
    pub textures: Option<FaceTextureIds>,
    /// Root node of the block's 16x16x16 sub-block template, or `None` when
    /// the block is a flat, uniform material.
    pub template_root: Option<u64>,
    /// Rasterization mode.
    pub render_type: RenderType,
    /// Coarse behavioral flags (all 32 bits, including cold-only ones).
    pub flags: BlockFlags,
    /// Light emitted by the block, `0..=15`.
    pub light_emission: u8,
    /// Light absorbed per block crossed, `0..=15` (0 for air, 15 for a full
    /// blocker).
    pub light_opacity: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_textures_all_and_column() {
        let stone = NamespacedKey::builtin("block/stone");
        let all = FaceTextures::all(stone.clone());
        for face in Face::ALL {
            assert_eq!(all.get(face), &stone);
        }

        let log = FaceTextures::column(
            NamespacedKey::builtin("block/log_oak_top"),
            NamespacedKey::builtin("block/log_oak_side"),
            NamespacedKey::builtin("block/log_oak_top"),
        );
        assert_eq!(log.get(Face::Top).path(), "block/log_oak_top");
        assert_eq!(log.get(Face::Bottom).path(), "block/log_oak_top");
        for face in [Face::North, Face::South, Face::East, Face::West] {
            assert_eq!(log.get(face).path(), "block/log_oak_side");
        }
    }

    /// The interning baseline, pinned: `FaceTextures::all()` gives six faces
    /// the *same* key, and a key is an `Arc<str>`, so the six faces share one
    /// string allocation instead of holding six copies of the text.
    #[test]
    fn face_textures_all_shares_one_string_allocation() {
        let key = NamespacedKey::new("mymod", "block/copper_ore").expect("valid key");
        let owned = key.clone();
        let all = FaceTextures::all(owned);

        // `key` plus the six faces: the one moved into `west`, and the five
        // clones the other faces got.
        assert_eq!(key.refcount(), 7, "six faces share one string");

        // And they all alias that same allocation.
        let expected = key.arc_ptr();
        for face in Face::ALL {
            assert_eq!(all.get(face).arc_ptr(), expected, "{face}");
        }
    }

    #[test]
    fn flags_combine_and_test() {
        let solid_opaque = BlockFlags::SOLID | BlockFlags::OPAQUE;
        assert!(solid_opaque.contains(BlockFlags::SOLID));
        assert!(solid_opaque.contains(BlockFlags::OPAQUE));
        assert!(!solid_opaque.contains(BlockFlags::LIQUID));
        assert!(!solid_opaque.is_empty());

        let mut flags = solid_opaque;
        flags |= BlockFlags::LIQUID;
        assert!(flags.contains(BlockFlags::LIQUID));
        flags &= BlockFlags::SOLID;
        assert!(!flags.contains(BlockFlags::LIQUID));
        assert!(flags.contains(BlockFlags::SOLID));
        assert!(BlockFlags::NONE.is_empty());
    }

    #[test]
    fn light_values_are_clamped_to_four_bits() {
        let props = BlockProperties::new().with_light(20, 99);
        assert_eq!(props.light_emission, 15);
        assert_eq!(props.light_opacity, 15);
    }

    #[test]
    fn block_flags_are_layout_identical_to_u32() {
        assert_eq!(size_of::<BlockFlags>(), size_of::<u32>());
        assert_eq!(align_of::<BlockFlags>(), align_of::<u32>());
    }
}
