//! The hot, per-hit slice of a block's runtime data: 8 bytes.
//!
//! A raymarcher or mesher that has just found a block id needs only a few
//! things: how it renders, a handful of flags, its light values, and *which*
//! six textures it uses. [`HotRecord`] holds exactly that, packed into two
//! `u32` words, so eight records share a 64-byte cache line and a 65 536-block
//! registry's whole hot table is 512 KiB.
//!
//! Everything else stays out of the way:
//!
//! - the six texture ids live in a deduplicated side table, reached through
//!   [`TextureSetId`] only when a surface is actually shaded;
//! - the SVDAG template root is placement-time data and lives in a sparse
//!   table;
//! - names, hardness and the full authoring shape stay in
//!   [`crate::BlockProperties`].
//!
//! # Layout
//!
//! Two little-endian `u32` words, `#[repr(C)]`, 4-byte aligned. The same
//! words are what a GPU storage buffer holds (WGSL has no 16-bit fields, so
//! everything is pre-packed into `u32`s).
//!
//! ```text
//! word 0 (meta)
//!   bits  0..=2   render type          (3 bits, 4 used)
//!   bit   3       has textures
//!   bits  4..=7   light emission       (0..=15)
//!   bits  8..=11  light opacity        (0..=15)
//!   bits 12..=27  flags                (low 16 bits of BlockFlags)
//!   bits 28..=31  spare
//! word 1
//!   bits  0..=15  texture set id       (valid iff "has textures")
//!   bits 16..=31  spare
//! ```
//!
//! WGSL decode, for the renderer:
//!
//! ```wgsl
//! let meta = hot[2u * id];
//! let render_type = meta & 7u;
//! let has_tex     = (meta >> 3u) & 1u;
//! let emission    = (meta >> 4u) & 15u;
//! let opacity     = (meta >> 8u) & 15u;
//! let flags       = (meta >> 12u) & 0xFFFFu;
//! let set         = hot[2u * id + 1u] & 0xFFFFu;
//! // texture set: three u32 words, two 16-bit ids each, in Face::ALL order
//! let w   = sets[3u * set + face / 2u];
//! let tex = (w >> (16u * (face & 1u))) & 0xFFFFu;
//! ```

use core::fmt;

use crate::properties::{BlockFlags, RenderType};
use crate::texture::TextureSetId;

pub const RENDER_MASK: u32 = 0b111;
pub const HAS_TEXTURES: u32 = 1 << 3;
pub const EMISSION_SHIFT: u32 = 4;
pub const OPACITY_SHIFT: u32 = 8;
pub const FLAGS_SHIFT: u32 = 12;
pub const LIGHT_MASK: u32 = 0xF;
pub const FLAGS_MASK: u32 = 0xFFFF;

/// Hot per-block data, 8 bytes. See the [module docs](self) for the layout.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotRecord {
    meta: u32,
    set: u32,
}

const _: () = assert!(size_of::<HotRecord>() == 8);
const _: () = assert!(align_of::<HotRecord>() == 4);
const _: () = assert!(64 % size_of::<HotRecord>() == 0);

impl HotRecord {
    /// Packs a record. Light values above 15 are clamped; flag bits above the
    /// 16th are dropped (they are cold-only, see [`BlockFlags::HOT_MASK`]).
    ///
    /// An `OPAQUE` block always packs opacity 15, whatever the authoring data
    /// says: the flag means "fully blocks light", and builtin blocks (and the
    /// committed `data/blocks.ron`) omit the explicit opacity.
    pub const fn pack(
        render_type: RenderType,
        flags: BlockFlags,
        light_emission: u8,
        light_opacity: u8,
        texture_set: Option<TextureSetId>,
    ) -> Self {
        let emission = if light_emission > 15 {
            15
        } else {
            light_emission
        } as u32;
        let opacity = if flags.contains(BlockFlags::OPAQUE) || light_opacity > 15 {
            15
        } else {
            light_opacity
        } as u32;
        let mut meta = render_type.to_bits()
            | emission << EMISSION_SHIFT
            | opacity << OPACITY_SHIFT
            | (flags.as_bits() & FLAGS_MASK) << FLAGS_SHIFT;
        let set = match texture_set {
            Some(id) => {
                meta |= HAS_TEXTURES;
                id.as_raw() as u32
            }
            None => 0,
        };
        Self { meta, set }
    }

    /// Rasterization mode.
    pub const fn render_type(self) -> RenderType {
        RenderType::from_bits(self.meta & RENDER_MASK)
    }

    /// The hot flag bits (low 16 of the block's [`BlockFlags`]).
    pub const fn flags(self) -> BlockFlags {
        BlockFlags::from_bits((self.meta >> FLAGS_SHIFT) & FLAGS_MASK)
    }

    /// Light emitted, `0..=15`. A block emits light iff this is non-zero.
    pub const fn light_emission(self) -> u8 {
        ((self.meta >> EMISSION_SHIFT) & LIGHT_MASK) as u8
    }

    /// Light absorbed per block crossed, `0..=15`.
    pub const fn light_opacity(self) -> u8 {
        ((self.meta >> OPACITY_SHIFT) & LIGHT_MASK) as u8
    }

    /// The block's six-face texture set, or `None` if it never draws.
    pub const fn texture_set(self) -> Option<TextureSetId> {
        if self.meta & HAS_TEXTURES != 0 {
            Some(TextureSetId::from_raw((self.set & 0xFFFF) as u16))
        } else {
            None
        }
    }

    /// The two raw words, exactly as uploaded to the GPU.
    pub const fn to_words(self) -> [u32; 2] {
        [self.meta, self.set]
    }
}

impl fmt::Debug for HotRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HotRecord")
            .field("render_type", &self.render_type())
            .field("flags", &self.flags())
            .field("light_emission", &self.light_emission())
            .field("light_opacity", &self.light_opacity())
            .field("texture_set", &self.texture_set())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RENDER: [RenderType; 4] = [
        RenderType::Solid,
        RenderType::Cutout,
        RenderType::Translucent,
        RenderType::Invisible,
    ];

    #[test]
    fn eight_records_fill_one_cache_line() {
        assert_eq!(size_of::<HotRecord>(), 8);
        assert_eq!(align_of::<HotRecord>(), 4);
        assert_eq!(64 / size_of::<HotRecord>(), 8);
    }

    #[test]
    fn every_field_round_trips() {
        for render in ALL_RENDER {
            for emission in [0u8, 1, 7, 15] {
                for opacity in [0u8, 3, 15] {
                    for set in [None, Some(0u16), Some(1), Some(65535)] {
                        let flags = BlockFlags::SOLID | BlockFlags::CULLS_SELF;
                        let rec = HotRecord::pack(
                            render,
                            flags,
                            emission,
                            opacity,
                            set.map(TextureSetId::from_raw),
                        );
                        assert_eq!(rec.render_type(), render);
                        assert_eq!(rec.flags(), flags);
                        assert_eq!(rec.light_emission(), emission);
                        assert_eq!(rec.light_opacity(), opacity);
                        assert_eq!(rec.texture_set().map(TextureSetId::as_raw), set);
                    }
                }
            }
        }
    }

    #[test]
    fn set_zero_is_distinct_from_no_textures() {
        let none = HotRecord::pack(RenderType::Solid, BlockFlags::NONE, 0, 0, None);
        let zero = HotRecord::pack(
            RenderType::Solid,
            BlockFlags::NONE,
            0,
            0,
            Some(TextureSetId::from_raw(0)),
        );
        assert_eq!(none.texture_set(), None);
        assert_eq!(zero.texture_set(), Some(TextureSetId::from_raw(0)));
        assert_ne!(none, zero);
    }

    #[test]
    fn light_is_clamped_and_cold_flag_bits_are_dropped() {
        let cold = BlockFlags::from_bits(1 << 20);
        let rec = HotRecord::pack(RenderType::Solid, BlockFlags::SOLID | cold, 200, 99, None);
        assert_eq!(rec.light_emission(), 15);
        assert_eq!(rec.light_opacity(), 15);
        assert!(rec.flags().contains(BlockFlags::SOLID));
        assert!(!rec.flags().contains(cold), "bit 20 is cold-only");
    }

    #[test]
    fn opaque_blocks_always_pack_full_opacity() {
        // Builtin blocks and `data/blocks.ron` set OPAQUE without an explicit
        // opacity; the flag alone must mean a full blocker.
        let stone = HotRecord::pack(
            RenderType::Solid,
            BlockFlags::SOLID | BlockFlags::OPAQUE,
            0,
            0,
            None,
        );
        assert_eq!(stone.light_opacity(), 15);

        // Non-opaque blocks keep the opacity they authored.
        let glass = HotRecord::pack(RenderType::Cutout, BlockFlags::NONE, 0, 3, None);
        assert_eq!(glass.light_opacity(), 3);
    }

    #[test]
    fn all_five_defined_flags_fit_with_room_to_spare() {
        let all = BlockFlags::SOLID
            | BlockFlags::OPAQUE
            | BlockFlags::REPLACEABLE
            | BlockFlags::LIQUID
            | BlockFlags::CULLS_SELF;
        assert!(BlockFlags::HOT_MASK.contains(all));
        let rec = HotRecord::pack(RenderType::Solid, all, 0, 0, None);
        assert_eq!(rec.flags(), all);
    }

    #[test]
    fn spare_bits_stay_zero() {
        let rec = HotRecord::pack(
            RenderType::Invisible,
            BlockFlags::HOT_MASK,
            15,
            15,
            Some(TextureSetId::from_raw(0xFFFF)),
        );
        let [meta, set] = rec.to_words();
        assert_eq!(meta >> 28, 0, "meta bits 28..32 are spare");
        assert_eq!(set >> 16, 0, "word 1 bits 16..32 are spare");
    }

    #[test]
    fn render_type_encoding_is_stable() {
        // The packed format is a contract with shaders: pin it.
        assert_eq!(RenderType::Solid.to_bits(), 0);
        assert_eq!(RenderType::Cutout.to_bits(), 1);
        assert_eq!(RenderType::Translucent.to_bits(), 2);
        assert_eq!(RenderType::Invisible.to_bits(), 3);
    }
}
