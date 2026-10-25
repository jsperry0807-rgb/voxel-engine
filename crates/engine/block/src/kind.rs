//! Optional enum of built-in block kinds.
//!
//! This is a convenience for engine code that wants compile-time names for
//! the vanilla-ish set of blocks; gameplay and modded blocks live purely in
//! the registry and never appear here.

use core::fmt;

use crate::id::NamespacedKey;
use crate::properties::BlockProperties;
use crate::properties::{BlockFlags, FaceTextures, RenderType};

/// Built-in block kinds shipped with the engine.
///
/// Every variant has a stable `voxel:` [`NamespacedKey`]; the mapping to a
/// runtime [`crate::MaterialId`] is resolved through the registry at startup.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BlockKind {
    Air,
    Stone,
    Dirt,
    Grass,
    Cobblestone,
    Sand,
    Gravel,
    Water,
    OakLog,
    OakLeaves,
    Bedrock,
    CoalOre,
    IronOre,
    GoldOre,
    DiamondOre,
    TallGrass,
    RedFlower,
    Snow,
    Ice,
    Sandstone,
}

impl BlockKind {
    /// All built-in kinds, in registration order.
    ///
    /// New kinds are appended rather than inserted: ids are stable and saved
    /// chunks refer to them, so the order is part of the save contract.
    pub const ALL: [BlockKind; 20] = [
        BlockKind::Air,
        BlockKind::Stone,
        BlockKind::Dirt,
        BlockKind::Grass,
        BlockKind::Cobblestone,
        BlockKind::Sand,
        BlockKind::Gravel,
        BlockKind::Water,
        BlockKind::OakLog,
        BlockKind::OakLeaves,
        BlockKind::Bedrock,
        BlockKind::CoalOre,
        BlockKind::IronOre,
        BlockKind::GoldOre,
        BlockKind::DiamondOre,
        BlockKind::TallGrass,
        BlockKind::RedFlower,
        BlockKind::Snow,
        BlockKind::Ice,
        BlockKind::Sandstone,
    ];

    /// Stable namespaced key for this kind.
    pub fn key(self) -> NamespacedKey {
        NamespacedKey::builtin(match self {
            BlockKind::Air => "air",
            BlockKind::Stone => "stone",
            BlockKind::Dirt => "dirt",
            BlockKind::Grass => "grass_block",
            BlockKind::Cobblestone => "cobblestone",
            BlockKind::Sand => "sand",
            BlockKind::Gravel => "gravel",
            BlockKind::Water => "water",
            BlockKind::OakLog => "oak_log",
            BlockKind::OakLeaves => "oak_leaves",
            BlockKind::Bedrock => "bedrock",
            BlockKind::CoalOre => "coal_ore",
            BlockKind::IronOre => "iron_ore",
            BlockKind::GoldOre => "gold_ore",
            BlockKind::DiamondOre => "diamond_ore",
            BlockKind::TallGrass => "tall_grass",
            BlockKind::RedFlower => "red_flower",
            BlockKind::Snow => "snow",
            BlockKind::Ice => "ice",
            BlockKind::Sandstone => "sandstone",
        })
    }

    /// Default properties used when registering this kind.
    ///
    /// Callers can tweak the returned value (e.g. different hardness for a
    /// modded overworld) before handing it to the registry builder.
    pub fn default_properties(self) -> BlockProperties {
        use BlockFlags as F;
        use RenderType as R;

        let texture = |path: &str| NamespacedKey::builtin(path);

        match self {
            BlockKind::Air => BlockProperties::new().with_flags(F::REPLACEABLE),
            BlockKind::Bedrock => BlockProperties::new()
                .with_render_type(R::Solid)
                .with_textures(FaceTextures::all(texture("block/bedrock")))
                .with_flags(F::SOLID | F::OPAQUE)
                // Unbreakable floor of the world.
                .with_hardness(-1.0),
            BlockKind::Water => BlockProperties::new()
                .with_render_type(R::Translucent)
                .with_textures(FaceTextures::all(texture("block/water_still")))
                .with_flags(F::LIQUID | F::CULLS_SELF)
                .with_light(0, 3),
            BlockKind::OakLeaves => BlockProperties::new()
                .with_render_type(R::Cutout)
                .with_textures(FaceTextures::all(texture("block/oak_leaves")))
                // Leaves occlude partially: they let some light through but
                // still cull the faces hidden between two leaf blocks.
                .with_flags(F::SOLID | F::CULLS_SELF)
                .with_light(0, 1)
                .with_hardness(0.2),
            BlockKind::OakLog => BlockProperties::new()
                .with_render_type(R::Solid)
                .with_textures(FaceTextures::column(
                    texture("block/oak_log_top"),
                    texture("block/oak_log"),
                    texture("block/oak_log_top"),
                ))
                .with_flags(F::SOLID | F::OPAQUE)
                .with_hardness(2.0),
            BlockKind::Grass => BlockProperties::new()
                .with_render_type(R::Solid)
                .with_textures(FaceTextures::column(
                    texture("block/grass_block_top"),
                    texture("block/grass_block_side"),
                    texture("block/dirt"),
                ))
                .with_flags(F::SOLID | F::OPAQUE)
                .with_hardness(0.6),
            BlockKind::Stone => solid(texture("block/stone"), 1.5),
            BlockKind::Dirt => solid(texture("block/dirt"), 0.5),
            BlockKind::Cobblestone => solid(texture("block/cobblestone"), 2.0),
            BlockKind::Sand => solid(texture("block/sand"), 0.5),
            BlockKind::Gravel => solid(texture("block/gravel"), 0.6),
            BlockKind::CoalOre => solid(texture("block/coal_ore"), 3.0),
            BlockKind::IronOre => solid(texture("block/iron_ore"), 3.0),
            BlockKind::GoldOre => solid(texture("block/gold_ore"), 3.0),
            BlockKind::DiamondOre => solid(texture("block/diamond_ore"), 3.0),
            BlockKind::Snow => solid(texture("block/snow"), 0.3),
            // Ice is solid but translucent: it occludes its neighbours yet
            // still lets light through, so frozen surfaces read as ice rather
            // than as another opaque stone.
            BlockKind::Ice => BlockProperties::new()
                .with_render_type(R::Translucent)
                .with_textures(FaceTextures::all(texture("block/ice")))
                .with_flags(F::SOLID | F::CULLS_SELF)
                .with_light(0, 1)
                .with_hardness(0.5),
            BlockKind::Sandstone => BlockProperties::new()
                .with_render_type(R::Solid)
                .with_textures(FaceTextures::column(
                    texture("block/sandstone_top"),
                    texture("block/sandstone_side"),
                    texture("block/sandstone_bottom"),
                ))
                .with_flags(F::SOLID | F::OPAQUE)
                .with_hardness(0.8),
            // Plants are cutout and replaceable: they are drawn, but they do
            // not occupy the cell — walking through tall grass must not be
            // blocked, and a plant placed on top of them may replace them.
            BlockKind::TallGrass => plant(texture("block/tall_grass")),
            BlockKind::RedFlower => plant(texture("block/red_flower")),
        }
    }
}

fn solid(texture: NamespacedKey, hardness: f32) -> BlockProperties {
    BlockProperties::new()
        .with_render_type(RenderType::Solid)
        .with_textures(FaceTextures::all(texture))
        .with_flags(BlockFlags::SOLID | BlockFlags::OPAQUE)
        .with_hardness(hardness)
}

/// A decorative plant: drawn, but neither solid nor opaque.
///
/// `REPLACEABLE` is what lets a plant be placed on the block below it
/// without digging first, and what lets another plant replace it.
fn plant(texture: NamespacedKey) -> BlockProperties {
    BlockProperties::new()
        .with_render_type(RenderType::Cutout)
        .with_textures(FaceTextures::all(texture))
        .with_flags(BlockFlags::REPLACEABLE)
}

impl fmt::Display for BlockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.key(), f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for kind in BlockKind::ALL {
            assert!(seen.insert(kind.key()), "duplicate key for {kind}");
        }
    }

    #[test]
    fn default_properties_are_sane() {
        for kind in BlockKind::ALL {
            let props = kind.default_properties();
            // Everything except air is visible and has a texture.
            if kind != BlockKind::Air {
                assert_ne!(props.render_type, RenderType::Invisible, "{kind}");
                assert!(props.textures.is_some(), "{kind}");
            }
        }

        let air = BlockKind::Air.default_properties();
        assert!(!air.is_solid());
        assert!(air.flags.contains(BlockFlags::REPLACEABLE));

        let stone = BlockKind::Stone.default_properties();
        assert!(stone.is_solid() && stone.is_opaque());
        assert!(stone.hardness > 0.0);

        let bedrock = BlockKind::Bedrock.default_properties();
        assert!(bedrock.hardness < 0.0, "bedrock should be unbreakable");
    }

    #[test]
    fn kind_display_shows_key() {
        assert_eq!(BlockKind::OakLog.to_string(), "voxel:oak_log");
    }
}
