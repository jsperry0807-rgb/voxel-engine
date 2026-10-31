//! RON (Rusty Object Notation) support for block definitions.
//!
//! These types mirror [`crate::BlockProperties`] but use RON-friendly field
//! types (strings for keys/textures, a raw `u32` for flags) so that a
//! `blocks.ron` file can be parsed by `ron` and type-checked by `ron-lsp`
//! against the Rust definitions here.

use crate::id::KeyError;
use crate::id::NamespacedKey;
use crate::material_table::MaterialTable;
use crate::properties::{BlockFlags, BlockProperties, FaceTextures, RenderType};
use crate::registry::BlockRegistryBuilder;
use crate::registry::RegistryError;

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RonFaceTextures {
    pub top: String,
    pub bottom: String,
    pub north: String,
    pub south: String,
    pub east: String,
    pub west: String,
}

impl RonFaceTextures {
    pub fn into_face_textures(self) -> Result<FaceTextures, KeyError> {
        Ok(FaceTextures {
            top: NamespacedKey::parse(&self.top)?,
            bottom: NamespacedKey::parse(&self.bottom)?,
            north: NamespacedKey::parse(&self.north)?,
            south: NamespacedKey::parse(&self.south)?,
            east: NamespacedKey::parse(&self.east)?,
            west: NamespacedKey::parse(&self.west)?,
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RonBlock {
    pub key: String,
    #[serde(default)]
    pub display_name: String,
    pub render_type: RenderType,
    #[serde(default)]
    pub textures: Option<RonFaceTextures>,
    #[serde(default)]
    pub template_root: Option<u64>,
    #[serde(default)]
    pub flags: u32,
    #[serde(default)]
    pub light_emission: u8,
    #[serde(default)]
    pub light_opacity: u8,
    #[serde(default)]
    pub hardness: f32,
}

impl RonBlock {
    pub fn into_properties(self) -> Result<(NamespacedKey, BlockProperties), KeyError> {
        let key = NamespacedKey::parse(&self.key)?;
        let textures = self
            .textures
            .map(RonFaceTextures::into_face_textures)
            .transpose()?;
        let mut props = BlockProperties::new()
            .with_render_type(self.render_type)
            .with_flags(BlockFlags::from_bits(self.flags))
            .with_light(self.light_emission, self.light_opacity)
            .with_hardness(self.hardness);
        if !self.display_name.is_empty() {
            props = props.with_display_name(self.display_name);
        }
        if let Some(tex) = textures {
            props = props.with_textures(tex);
        }
        if let Some(root) = self.template_root {
            props = props.with_template_root(root);
        }
        Ok((key, props))
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct RonBlockList {
    pub blocks: Vec<RonBlock>,
}

#[derive(Debug)]
pub enum BlockLoadError {
    Parse(ron::error::SpannedError),
    MaterialParse(ron::error::SpannedError),
    Key(KeyError),
    Registry(RegistryError),
    /// The material table was generated from a different `blocks.ron`.
    ///
    /// A stale but internally consistent pair (slots and rows line up) would
    /// otherwise freeze with atlas slots from the previous generation.
    StaleMaterials {
        expected: String,
        actual: String,
    },
}

impl core::fmt::Display for BlockLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BlockLoadError::Parse(e) => write!(f, "blocks.ron parse error: {e}"),
            BlockLoadError::MaterialParse(e) => write!(f, "materials.gen.ron parse error: {e}"),
            BlockLoadError::Key(e) => write!(f, "invalid key: {e}"),
            BlockLoadError::Registry(e) => write!(f, "registry error: {e}"),
            BlockLoadError::StaleMaterials { expected, actual } => write!(
                f,
                "materials.gen.ron was generated from a different blocks.ron \
                 (table blocks hash {expected}, input {actual})"
            ),
        }
    }
}

impl core::error::Error for BlockLoadError {}

impl BlockRegistryBuilder {
    /// Register every block described by a RON document and attach a generated
    /// material table.
    ///
    /// The material table is validated against the registered blocks at
    /// [`BlockRegistryBuilder::freeze`], so a stale or missing row is reported
    /// there rather than becoming a silent atlas lookup.
    pub fn load_ron_with_materials(
        &mut self,
        blocks: &str,
        materials: &str,
    ) -> Result<(), BlockLoadError> {
        let table: MaterialTable =
            ron::from_str(materials).map_err(BlockLoadError::MaterialParse)?;
        // The table records the `blocks.ron` it was generated from. An empty
        // hash means an in-memory table (tests/builders), which has no pairing
        // to check.
        if !table.blocks_blake3.is_empty() {
            let actual = blake3::hash(blocks.as_bytes()).to_hex().to_string();
            if table.blocks_blake3 != actual {
                return Err(BlockLoadError::StaleMaterials {
                    expected: table.blocks_blake3.clone(),
                    actual,
                });
            }
        }
        // Commit the table and the blocks together: a parse or registry error
        // must not leave a half-built builder that a retry then rejects as a
        // duplicate.
        let mut scratch = self.clone();
        scratch.set_materials(table);
        scratch.load_ron(blocks)?;
        *self = scratch;
        Ok(())
    }

    /// Register every block described by a RON document.
    pub fn load_ron(&mut self, input: &str) -> Result<(), BlockLoadError> {
        let list: RonBlockList = ron::from_str(input).map_err(BlockLoadError::Parse)?;
        let mut scratch = self.clone();
        for block in list.blocks {
            let (key, props) = block.into_properties().map_err(BlockLoadError::Key)?;
            // Air is pre-registered by `BlockRegistryBuilder::new`; a RON file
            // that also lists it (for completeness) must not be a duplicate.
            if key == crate::kind::BlockKind::Air.key() && !scratch.is_empty() {
                continue;
            }
            scratch
                .register(key, props)
                .map_err(BlockLoadError::Registry)?;
        }
        *self = scratch;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::BlockRegistryBuilder;

    #[test]
    fn blocks_ron_loads() {
        let input = include_str!("../data/blocks.ron");
        let mut builder = BlockRegistryBuilder::new();
        builder.load_ron(input).expect("blocks.ron should load");
        // air (pre-registered) + 33 blocks
        assert_eq!(builder.len(), 34);
    }
}
