//! Atlas materials and per-block face variants (M1.5-08).
//!
//! Two ids are in play and they are deliberately different types:
//!
//! - [`BlockId`] is a block's dense identity in the registry (the type the
//!   rest of the engine calls `MaterialId` today).
//! - [`MaterialId`] is an **atlas material slot**: `1..=255`, where the slot is
//!   also `tile_origin(slot)` in the 256² atlas. Slot `0` is air.
//!
//! A block's six faces are a [`FaceMaterials`]. The registry keeps one
//! `Option<FaceMaterials>` per block id, so the future atlas renderer reads
//! `tile_origin(face_materials.get(face).as_raw())` with no indirection.
//!
//! The generated input is `data/materials.gen.ron` (see
//! [`crate::material_table`]). This module is the only consumer that turns that
//! file into runtime data.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;

use voxel_core::Face;
use voxel_core::atlas::MAX_MATERIALS;

use crate::id::MaterialId as BlockIdType;
use crate::id::NamespacedKey;
use crate::material_table::MaterialTable;
use crate::registry::BlockRegistry;
use crate::texture_manifest::AlphaClass;

/// A block's dense identity. Alias of the crate's existing block-id type.
pub type BlockId = BlockIdType;

/// An atlas material slot. `1..=255`; `0` is air and is never sampled.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MaterialId(u8);

impl MaterialId {
    /// The reserved air slot.
    pub const AIR: Self = Self(0);

    /// Builds a slot from its raw atlas id.
    pub const fn from_raw(raw: u8) -> Self {
        Self(raw)
    }

    /// The raw atlas id, which is also the tile index.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Index into a dense materials table (`0..len` in atlas-slot order).
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// True for the air slot.
    pub const fn is_air(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MaterialId({})", self.0)
    }
}

impl fmt::Debug for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The six material slots of a block, in the spec's field order.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct FaceMaterials {
    pub top: MaterialId,
    pub bottom: MaterialId,
    pub north: MaterialId,
    pub south: MaterialId,
    pub east: MaterialId,
    pub west: MaterialId,
}

impl FaceMaterials {
    /// The same material on every face.
    pub const fn all(material: MaterialId) -> Self {
        Self {
            top: material,
            bottom: material,
            north: material,
            south: material,
            east: material,
            west: material,
        }
    }

    /// A vertical column with distinct caps.
    pub const fn column(top: MaterialId, side: MaterialId, bottom: MaterialId) -> Self {
        Self {
            top,
            bottom,
            north: side,
            south: side,
            east: side,
            west: side,
        }
    }

    /// The material for a face.
    pub const fn get(self, face: Face) -> MaterialId {
        match face {
            Face::Top => self.top,
            Face::Bottom => self.bottom,
            Face::North => self.north,
            Face::South => self.south,
            Face::East => self.east,
            Face::West => self.west,
        }
    }

    /// Six slots in [`Face::ALL`] order.
    pub const fn as_array(self) -> [MaterialId; 6] {
        [
            self.north,
            self.south,
            self.east,
            self.west,
            self.top,
            self.bottom,
        ]
    }

    /// Six raw atlas ids in [`Face::ALL`] order.
    pub const fn to_bytes(self) -> [u8; 6] {
        [
            self.north.as_raw(),
            self.south.as_raw(),
            self.east.as_raw(),
            self.west.as_raw(),
            self.top.as_raw(),
            self.bottom.as_raw(),
        ]
    }

    /// Packed for a future GPU upload: four ids in word 0, two in word 1,
    /// in [`Face::ALL`] order.
    pub const fn to_words(self) -> [u32; 2] {
        let ids = self.to_bytes();
        [
            ids[0] as u32 | (ids[1] as u32) << 8 | (ids[2] as u32) << 16 | (ids[3] as u32) << 24,
            ids[4] as u32 | (ids[5] as u32) << 8,
        ]
    }
}

/// One manifest material row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    pub id: MaterialId,
    pub key: NamespacedKey,
    pub alpha: AlphaClass,
}

/// Runtime material table: slots and per-block faces.
#[derive(Clone, Debug, Default)]
pub struct MaterialRegistry {
    /// `materials[i]` is slot `i + 1`.
    materials: Vec<Material>,
    /// `faces[block.index()]` is `None` for air and non-drawing blocks.
    faces: Vec<Option<FaceMaterials>>,
    by_key: HashMap<NamespacedKey, MaterialId>,
}

impl MaterialRegistry {
    /// Builds the runtime registry from a generated [`MaterialTable`] and the
    /// frozen block registry it describes.
    ///
    /// # Errors
    ///
    /// Reports every mismatch as a typed [`MaterialRegistryError`]; nothing is
    /// truncated or silently remapped.
    pub fn from_generated(
        table: &MaterialTable,
        blocks: &BlockRegistry,
    ) -> Result<Self, MaterialRegistryError> {
        if table.schema != 1 {
            return Err(MaterialRegistryError::InvalidSchema(table.schema));
        }
        let attempted = table.materials.len();
        if attempted > MAX_MATERIALS as usize {
            return Err(MaterialRegistryError::CapExceeded {
                attempted,
                cap: MAX_MATERIALS as usize,
            });
        }

        let mut by_id: BTreeMap<u8, Material> = BTreeMap::new();
        let mut by_key = HashMap::with_capacity(table.materials.len());
        for row in &table.materials {
            if row.id == 0 || row.id as usize > u8::MAX as usize {
                return Err(MaterialRegistryError::InvalidSlot(row.id));
            }
            let id = row.id as u8;
            let key = NamespacedKey::parse(&row.key)
                .map_err(|_| MaterialRegistryError::InvalidKey(row.key.clone()))?;
            if by_id.contains_key(&id) {
                return Err(MaterialRegistryError::DuplicateSlot(id));
            }
            if by_key.contains_key(&key) {
                return Err(MaterialRegistryError::DuplicateKey(row.key.clone()));
            }
            let material = Material {
                id: MaterialId::from_raw(id),
                key: key.clone(),
                alpha: row.alpha,
            };
            by_key.insert(key, material.id);
            by_id.insert(id, material);
        }

        let mut materials = Vec::with_capacity(by_id.len());
        for (expected, (found, material)) in (1u8..).zip(by_id) {
            if found != expected {
                return Err(MaterialRegistryError::SlotGap { expected, found });
            }
            materials.push(material);
        }
        let len = materials.len();
        if len == 0 {
            return Err(MaterialRegistryError::Empty);
        }
        if materials
            .iter()
            .any(|material| material.id.as_raw() as usize > len)
        {
            return Err(MaterialRegistryError::InvalidSlot(
                materials
                    .iter()
                    .find(|material| material.id.as_raw() as usize > len)
                    .map(|material| material.id.as_raw() as u16)
                    .unwrap_or(0u16),
            ));
        }

        let mut faces: Vec<Option<FaceMaterials>> = vec![None; blocks.len()];
        let mut seen_blocks = HashSet::with_capacity(table.faces.len());
        for row in &table.faces {
            let Some(block) = blocks.id_of(&row.block) else {
                return Err(MaterialRegistryError::UnusedManifestEntry(
                    row.block.clone(),
                ));
            };
            if blocks
                .get(block)
                .is_none_or(|props| props.textures.is_none())
            {
                return Err(MaterialRegistryError::UnusedManifestEntry(
                    row.block.clone(),
                ));
            }
            if !seen_blocks.insert(block) {
                return Err(MaterialRegistryError::DuplicateBlockRow(row.block.clone()));
            }
            let face = |slot: u16, face: &'static str| {
                if slot == 0 || slot as usize > len {
                    return Err(MaterialRegistryError::UnknownMaterial {
                        block: row.block.clone(),
                        face,
                        id: slot as u8,
                    });
                }
                Ok(MaterialId::from_raw(slot as u8))
            };
            let faces_row = FaceMaterials {
                top: face(row.top, "top")?,
                bottom: face(row.bottom, "bottom")?,
                north: face(row.north, "north")?,
                south: face(row.south, "south")?,
                east: face(row.east, "east")?,
                west: face(row.west, "west")?,
            };
            faces[block.index()] = Some(faces_row);
        }

        // Every drawing block needs a row; a block with no textures must not
        // have one. This catches both "art forgot a face" and "row for a
        // non-drawing block".
        for (block, props) in blocks.iter() {
            let key = props
                .textures
                .as_ref()
                .and_then(|_| blocks.key(block))
                .map(|key| key.to_string())
                .unwrap_or_else(|| format!("BlockId({})", block.as_raw()));
            if props.textures.is_some() && faces[block.index()].is_none() {
                return Err(MaterialRegistryError::MissingManifestEntry(key));
            }
            if props.textures.is_none() && faces[block.index()].is_some() {
                return Err(MaterialRegistryError::UnusedManifestEntry(key));
            }
        }

        Ok(Self {
            materials,
            faces,
            by_key,
        })
    }

    /// Number of drawable materials (air excluded).
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// One material by atlas slot.
    pub fn material(&self, id: MaterialId) -> Option<&Material> {
        if id.is_air() {
            return None;
        }
        self.materials.get(id.index() - 1)
    }

    /// The material id for a manifest key.
    pub fn material_id(&self, key: &NamespacedKey) -> Option<MaterialId> {
        self.by_key.get(key).copied()
    }

    /// The six atlas slots for a block, or `None` for air/non-drawing blocks.
    pub fn faces(&self, block: BlockId) -> Option<FaceMaterials> {
        self.faces.get(block.index()).copied().flatten()
    }

    /// All material rows, in slot order.
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    /// Iterate `(block, faces)` for every drawing block.
    pub fn iter_faces(&self) -> impl Iterator<Item = (BlockId, FaceMaterials)> + '_ {
        self.faces.iter().enumerate().filter_map(|(index, faces)| {
            faces.map(|faces| (BlockId::from_raw(index as u16), faces))
        })
    }
}

/// A material-registry failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialRegistryError {
    /// More than the atlas cap (256) material slots were requested.
    CapExceeded { attempted: usize, cap: usize },
    /// Two materials use the same key.
    DuplicateKey(String),
    /// Two materials use the same slot.
    DuplicateSlot(u8),
    /// Slots are not contiguous from 1.
    SlotGap { expected: u8, found: u8 },
    /// A block with textures has no face row.
    MissingManifestEntry(String),
    /// A face row names a block that is not in the registry or does not draw.
    UnusedManifestEntry(String),
    /// A block has more than one face row.
    DuplicateBlockRow(String),
    /// A face row uses a slot that is not in the material list.
    UnknownMaterial {
        block: String,
        face: &'static str,
        id: u8,
    },
    /// A material key is not a valid namespaced key.
    InvalidKey(String),
    /// A material slot is 0 or outside the atlas.
    InvalidSlot(u16),
    /// The material list is empty.
    Empty,
    /// The table schema is not the one this runtime understands.
    InvalidSchema(u32),
}

impl fmt::Display for MaterialRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapExceeded { attempted, cap } => write!(
                f,
                "attempted to register {attempted} materials, but the atlas cap is {cap}"
            ),
            Self::DuplicateKey(key) => write!(f, "duplicate material key `{key}`"),
            Self::DuplicateSlot(slot) => write!(f, "duplicate material slot {slot}"),
            Self::SlotGap { expected, found } => {
                write!(
                    f,
                    "material slots must be contiguous from 1: expected {expected}, found {found}"
                )
            }
            Self::MissingManifestEntry(key) => write!(f, "missing material face row for `{key}`"),
            Self::UnusedManifestEntry(key) => write!(
                f,
                "material face row for unknown or non-drawing block `{key}`"
            ),
            Self::DuplicateBlockRow(key) => write!(f, "duplicate material face row for `{key}`"),
            Self::UnknownMaterial { block, face, id } => {
                write!(f, "`{block}` face {face} uses unknown material slot {id}")
            }
            Self::InvalidKey(key) => write!(f, "invalid material key `{key}`"),
            Self::InvalidSlot(slot) => {
                write!(f, "material slot {slot} is outside 1..={MAX_MATERIALS}")
            }
            Self::Empty => f.write_str("the material list is empty"),
            Self::InvalidSchema(schema) => {
                write!(f, "material table schema {schema} is not supported")
            }
        }
    }
}

impl Error for MaterialRegistryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material_table::{FaceRow, MaterialRow};
    use crate::properties::{BlockProperties, FaceTextures};
    use crate::registry::BlockRegistryBuilder;

    fn material_row(id: u8) -> MaterialRow {
        MaterialRow {
            id: id as u16,
            key: format!("voxel:block/m{id}"),
            alpha: AlphaClass::Opaque,
        }
    }

    fn face_row(block: &str, id: u8) -> FaceRow {
        FaceRow {
            block: block.to_owned(),
            top: id as u16,
            bottom: id as u16,
            north: id as u16,
            south: id as u16,
            east: id as u16,
            west: id as u16,
        }
    }

    fn table(materials: usize, blocks: usize) -> MaterialTable {
        MaterialTable {
            schema: 1,
            manifest_blake3: String::new(),
            blocks_blake3: String::new(),
            materials: (1..=materials).map(|id| material_row(id as u8)).collect(),
            faces: (1..=blocks)
                .map(|index| {
                    face_row(
                        &format!("voxel:b{index}"),
                        ((index - 1) % materials + 1) as u8,
                    )
                })
                .collect(),
        }
    }

    fn block_builder(count: usize) -> BlockRegistryBuilder {
        let mut builder = BlockRegistryBuilder::with_capacity(count);
        for index in 1..count {
            builder
                .register(
                    NamespacedKey::builtin(&format!("b{index}")),
                    BlockProperties::new()
                        .with_render_type(crate::properties::RenderType::Solid)
                        .with_textures(FaceTextures::all(NamespacedKey::builtin("block/test"))),
                )
                .expect("unique block");
        }
        builder
    }

    #[test]
    fn material_registry_rejects_slot_256() {
        let mut table = table(1, 0);
        table.materials[0].id = 256;
        let mut builder = block_builder(1);
        builder.set_materials(table);
        assert!(matches!(
            builder.freeze(),
            Err(crate::registry::FreezeError::Material(
                MaterialRegistryError::InvalidSlot(256)
            ))
        ));
    }

    #[test]
    fn cap_256_material_registry() {
        let mut builder = block_builder(1);
        builder.set_materials(table(257, 0));
        let error = builder.freeze().expect_err("257 materials must fail");
        assert_eq!(
            error,
            crate::registry::FreezeError::Material(MaterialRegistryError::CapExceeded {
                attempted: 257,
                cap: 256,
            })
        );
    }

    #[test]
    fn material_registry_rejects_duplicate_slots() {
        let mut table = table(2, 0);
        table.materials[1].id = 1;
        let mut builder = block_builder(1);
        builder.set_materials(table);
        assert!(matches!(
            builder.freeze(),
            Err(crate::registry::FreezeError::Material(
                MaterialRegistryError::DuplicateSlot(1)
            ))
        ));
    }

    #[test]
    fn material_registry_rejects_slot_gaps() {
        let mut table = table(2, 0);
        table.materials[1].id = 3;
        let mut builder = block_builder(1);
        builder.set_materials(table);
        assert!(matches!(
            builder.freeze(),
            Err(crate::registry::FreezeError::Material(
                MaterialRegistryError::SlotGap {
                    expected: 2,
                    found: 3
                }
            ))
        ));
    }

    #[test]
    fn material_registry_rejects_missing_and_unused_rows() {
        let mut builder = block_builder(3);
        builder.set_materials(table(2, 1));
        assert!(matches!(
            builder.freeze(),
            Err(crate::registry::FreezeError::Material(
                MaterialRegistryError::MissingManifestEntry(_)
            ))
        ));

        let mut builder = block_builder(1);
        builder.set_materials(table(2, 1));
        assert!(matches!(
            builder.freeze(),
            Err(crate::registry::FreezeError::Material(
                MaterialRegistryError::UnusedManifestEntry(_)
            ))
        ));
    }

    #[test]
    fn fixture_64_blocks_freeze_after_m1_5_08() {
        let mut builder = block_builder(64);
        builder.set_materials(table(23, 63));
        let registry = builder.freeze().expect("64 blocks freeze");
        assert_eq!(registry.len(), 64);
        let block = registry.id_of("voxel:b1").expect("b1");
        let faces = registry.face_materials(block).expect("b1 has faces");
        assert_eq!(faces.top, MaterialId::from_raw(1));
        assert_eq!(faces.get(Face::Top), MaterialId::from_raw(1));
        assert_eq!(registry.material_registry().expect("attached").len(), 23);
    }

    #[test]
    fn material_registry_matches_committed_blocks() {
        let blocks = include_str!("../data/blocks.ron");
        let materials = include_str!("../data/materials.gen.ron");
        let mut builder = BlockRegistryBuilder::new();
        builder
            .load_ron_with_materials(blocks, materials)
            .expect("committed blocks and materials load");
        let registry = builder.freeze().expect("blocks freeze");
        assert_eq!(registry.len(), 20);

        let stone = registry.id_of("voxel:stone").expect("stone");
        assert_eq!(
            registry.face_materials(stone),
            Some(FaceMaterials::all(MaterialId::from_raw(1)))
        );
        let grass = registry.id_of("voxel:grass_block").expect("grass");
        assert_eq!(
            registry.face_materials(grass),
            Some(FaceMaterials::column(
                MaterialId::from_raw(3),
                MaterialId::from_raw(4),
                MaterialId::from_raw(2),
            ))
        );
        let sandstone = registry.id_of("voxel:sandstone").expect("sandstone");
        assert_eq!(
            registry.face_materials(sandstone),
            Some(FaceMaterials {
                top: MaterialId::from_raw(21),
                bottom: MaterialId::from_raw(22),
                north: MaterialId::from_raw(23),
                south: MaterialId::from_raw(23),
                east: MaterialId::from_raw(23),
                west: MaterialId::from_raw(23),
            })
        );
    }

    #[test]
    fn face_materials_pack_in_face_all_order() {
        let faces = FaceMaterials {
            top: MaterialId::from_raw(1),
            bottom: MaterialId::from_raw(2),
            north: MaterialId::from_raw(3),
            south: MaterialId::from_raw(4),
            east: MaterialId::from_raw(5),
            west: MaterialId::from_raw(6),
        };
        assert_eq!(faces.to_bytes(), [3, 4, 5, 6, 1, 2]);
        assert_eq!(faces.to_words(), [0x0605_0403, 0x0000_0201]);
        for face in Face::ALL {
            assert_eq!(
                faces.get(face).as_raw(),
                faces.as_array()[face.index()].as_raw()
            );
        }
    }
}
