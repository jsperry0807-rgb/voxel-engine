//! Block definitions for the voxel engine.
//!
//! The crate separates a block's *identity* from its *data*:
//!
//! - [`NamespacedKey`] is the stable, human-readable identity
//!   ("`mymod:copper_ore`") used by assets, data files and save games.
//! - [`MaterialId`] is a dense runtime index assigned at startup, small enough to
//!   store directly in chunk voxel arrays (`u16`). It is the voxel payload,
//!   deliberately distinct from the SVDAG's internal 64-bit `BlockId` node
//!   pointer. Id `0` is always air, so a zero-filled chunk means "empty" with
//!   no per-cell initialization.
//! - [`BlockProperties`] describes how a block renders ([`RenderType`],
//!   [`FaceTextures`]) and behaves ([`BlockFlags`], light, hardness), and
//!   optionally names the SVDAG root of its 16x16x16 sub-block template.
//! - The registry's runtime data is split by how it is read. [`HotRecord`] is
//!   8 bytes per block — render type, hot flags, light, and a
//!   [`TextureSetId`] — for per-voxel and per-hit loops
//!   ([`BlockRegistry::hot`]). Each distinct six-face texture set is stored
//!   once ([`BlockRegistry::texture_set`]), and template roots are sparse
//!   ([`BlockRegistry::template_root`]). [`BlockRecord`] is the whole block
//!   assembled from those pieces ([`BlockRegistry::resolved`]), for code that
//!   wants everything at once.
//! - [`BlockRegistry`] is the frozen key/id/properties mapping shared by every
//!   system; it is built once via [`BlockRegistryBuilder`], whose
//!   [`BlockRegistryBuilder::freeze`] is the one-time resolution boundary and
//!   reports [`FreezeError`] rather than failing silently.
//! - [`BlockKind`] optionally names the built-in blocks, and [`BlockTags`]
//!   groups blocks into namespaced-key-tagged sets for gameplay queries.
//! - [`Palette`] is the persisted shape of a block list — keys only — and
//!   [`BlockRegistry::resolve_palette`] turns one back into this session's
//!   ids.
//!
//! ```
//! use voxel_block::{BlockKind, BlockRegistryBuilder};
//!
//! let mut builder = BlockRegistryBuilder::new();
//! builder.register_kind(BlockKind::Stone).unwrap();
//! let registry = builder.freeze().unwrap();
//!
//! let stone = registry.id_of("voxel:stone").unwrap();
//! assert!(registry.get(stone).unwrap().is_solid());
//! ```
//!
//! # Limits
//!
//! A registry has two hard caps, both at 65 536, and they are reached in
//! different places:
//!
//! - **Material ids.** A registry holds at most 65 536 blocks, because ids are
//!   `u16`. The limit is hit while *registering*, and reported as
//!   [`RegistryError::Exhausted`].
//! - **Distinct textures.** A registry may name at most 65 536 distinct
//!   textures, because resolved texture ids are `u16` indices into the GPU
//!   `Texture2DArray`. The limit is hit while
//!   *freezing*, and reported as [`FreezeError::TooManyTextures`].
//!
//! The texture table is therefore the practical ceiling. A block may name six
//! distinct textures, so a registry that gives every block its own set runs
//! out of texture ids at about the 10 923rd block — far short of the 65 536
//! blocks the id space allows. Sharing textures between blocks is what keeps a
//! large registry inside the limit.
//!
//! # Boundaries
//!
//! Ids are session-local, so anything that outlives a session names blocks by
//! [`NamespacedKey`] instead and resolves them on the way in. Three rules hold
//! that line, and they are enforced by types rather than by convention:
//!
//! - **Parse, don't validate.** A [`NamespacedKey`] is either a valid key or
//!   it does not exist; a [`Palette`] holds only valid keys, and
//!   [`Palette::parse`] returns a palette or an error, never a palette
//!   holding half a key.
//! - **Total, not partial.** [`BlockRegistry::resolve_palette`] answers for
//!   *every* input with a [`Resolution`] or a typed [`PaletteError`]. It never
//!   panics and never substitutes silently. This is the counterpart to
//!   [`BlockRegistry::resolved`], which is partial by design: it panics on an
//!   unknown id, because it only ever sees ids the registry itself produced.
//! - **Palette remapping.** Resolution goes through keys, so a palette
//!   written by one session loads correctly into another, whatever order
//!   either registered its blocks in.
//!
//! Texture ids are session-local too, and are *derived* at
//! [`BlockRegistryBuilder::freeze`] rather than persisted. A file that names
//! a block gets the textures that block has in the session loading it. The
//! format contract is `docs/save-format.md`.
//!
//! # Atlas materials
//!
//! `materials.gen.ron` describes the atlas slots (`material::MaterialId`, a
//! `u8` that is also `tile_origin`) and each block's six face variants
//! ([`FaceMaterials`]). Attach it with
//! [`BlockRegistryBuilder::load_ron_with_materials`] or
//! [`BlockRegistryBuilder::set_materials`]; the registry validates it at
//! freeze and exposes [`BlockRegistry::face_materials`]. This is separate from
//! the older `TextureSetId` path, which remains for the texture-array
//! renderer until M1.5-12 switches the chunk shader to atlas slots.

pub mod hot;
pub mod id;
pub mod kind;
pub mod material;
pub mod material_table;
pub mod palette;
pub mod properties;
pub mod registry;
pub mod ron;
pub mod tags;
pub mod texture;
pub mod texture_manifest;

pub use hot::HotRecord;
pub use id::{AIR_ID, BUILTIN_NAMESPACE, KeyError, MaterialId, NamespacedKey};
pub use kind::BlockKind;
pub use material::{
    BlockId as AtlasBlockId, FaceMaterials, Material, MaterialId as AtlasMaterialId,
    MaterialRegistry, MaterialRegistryError,
};
pub use palette::{
    KeyContext, MissingBlockPolicy, MissingKey, Omission, Palette, PaletteError, Resolution,
};
pub use properties::{BlockFlags, BlockProperties, BlockRecord, FaceTextures, RenderType};
pub use registry::{BlockRegistry, BlockRegistryBuilder, FreezeError, RegistryError};
pub use tags::BlockTags;
pub use texture::{FaceTextureIds, TextureId, TextureSetId, TextureTable};
pub use voxel_core::face;
