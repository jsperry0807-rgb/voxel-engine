//! Block registration: a mutable [`BlockRegistryBuilder`] and the frozen
//! [`BlockRegistry`] it produces.
//!
//! The intended flow is:
//!
//! 1. During startup, systems and mods register blocks through the builder,
//!    collecting a [`MaterialId`] for each.
//! 2. The builder is frozen into a [`BlockRegistry`].
//! 3. The frozen registry is shared immutably (typically behind an `Arc`) by
//!    every system that needs block data.

use core::fmt;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::id::{AIR_ID, MaterialId, NamespacedKey};
use crate::kind::BlockKind;
use voxel_core::Face;

use crate::hot::HotRecord;
use crate::material::{FaceMaterials, MaterialRegistry, MaterialRegistryError};
use crate::material_table::MaterialTable;
use crate::properties::{BlockFlags, BlockProperties, BlockRecord};
use crate::texture::{FaceTextureIds, TextureId, TextureSetId, TextureTable};

/// Error returned when registration fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// A block with this key was already registered. Ids are never reused, so
    /// silently overwriting would invalidate saved chunk data.
    DuplicateKey(NamespacedKey),
    /// The first registered block must be air, so that `MaterialId 0` is a usable
    /// sentinel for "empty" in chunk storage.
    AirMustBeFirst,
    /// The `u16` id space is exhausted.
    ///
    /// A registry holds at most 65 536 blocks. This is the *block* cap; the
    /// other one — 65 536 distinct textures, reached at freeze time as
    /// [`FreezeError::TooManyTextures`] — is the practical ceiling, because a
    /// block may name six distinct textures.
    Exhausted,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::DuplicateKey(key) => write!(f, "block `{key}` is already registered"),
            RegistryError::AirMustBeFirst => {
                write!(f, "air must be the first registered block (MaterialId 0)")
            }
            RegistryError::Exhausted => write!(f, "material id space is exhausted"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Error returned when freezing a builder into a registry fails.
///
/// Freezing is the one-time resolution boundary, so this is where a resource
/// limit surfaces: a mod that references more distinct textures than a `u16`
/// id can address fails here — loudly and recoverably — instead of panicking
/// or silently handing out a duplicate id.
///
/// The material id space has its own, earlier limit: it runs out during
/// registration and reports [`RegistryError::Exhausted`]. Of the two caps the
/// texture table is the practical one — six distinct faces per block exhaust
/// it at about the 10 923rd block — so this is the error a large mod pack is
/// most likely to hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreezeError {
    /// The blocks refer to more distinct textures than the table can index.
    ///
    /// Texture ids are `u16`, so one registry may reference at most 65 536
    /// distinct textures. `count` is how many distinct textures the blocks
    /// referred to.
    TooManyTextures {
        /// Number of distinct textures the blocks referred to.
        count: usize,
    },
    /// The attached `materials.gen.ron` table is not consistent with the
    /// blocks (cap, duplicate/gap, missing or unused face rows).
    Material(MaterialRegistryError),
}

impl fmt::Display for FreezeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FreezeError::TooManyTextures { count } => write!(
                f,
                "the registry refers to {count} distinct textures, \
                 but at most 65536 can be indexed"
            ),
            FreezeError::Material(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FreezeError {}

/// Mutable block registry used during startup and data (re)loads.
///
/// `MaterialId 0` is always [`crate::AIR_ID`]: [`BlockRegistryBuilder::new`]
/// pre-registers air under "`voxel:air`", and [`BlockRegistryBuilder::register`]
/// refuses to place any other block at id 0. Chunk storage can therefore rely
/// on "all zeroes" meaning "empty", so allocating a chunk needs no per-cell
/// initialization.
///
/// [`BlockRegistryBuilder::default`] yields a *completely empty* builder (no
/// air yet); the first block you register must still be air. Use `new()` unless
/// you are building the registry from external data and want to control air's
/// properties yourself.
#[derive(Default, Clone)]
pub struct BlockRegistryBuilder {
    /// `entries[i]` describes `MaterialId(i)`; index is position, so ids stay
    /// dense without a lookup. The key is `None` for slots held by
    /// [`Self::reserve`], which deliberately have no identity.
    entries: Vec<(Option<NamespacedKey>, BlockProperties)>,
    /// Duplicate-check and `contains` in O(1) instead of scanning `entries`.
    /// Reserved slots are absent, by construction.
    index: HashMap<NamespacedKey, MaterialId>,
    /// Optional generated material table, resolved into the registry at freeze.
    materials: Option<MaterialTable>,
}

impl BlockRegistryBuilder {
    /// A builder with air already registered at [`AIR_ID`].
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    /// A builder with air pre-registered and space reserved for `capacity`
    /// blocks in total (air included).
    ///
    /// Reserve when the block count is known up front — a data-driven load or
    /// a fixed mod list — to avoid the repeated reallocation that otherwise
    /// happens as `entries` and `index` grow. Both containers are reserved
    /// together, since they are appended to in lockstep by [`Self::register`].
    pub fn with_capacity(capacity: usize) -> Self {
        let mut builder = Self {
            entries: Vec::with_capacity(capacity.max(1)),
            index: HashMap::with_capacity(capacity.max(1)),
            materials: None,
        };
        // Must succeed: a fresh builder cannot contain a duplicate, and the
        // capacity was reserved above.
        builder
            .register(BlockKind::Air.key(), BlockKind::Air.default_properties())
            .expect("fresh builder cannot contain a duplicate key");
        debug_assert_eq!(builder.len(), 1);
        builder
    }

    /// Attaches a generated material table. It is validated at
    /// [`Self::freeze`] against the blocks registered here.
    pub fn set_materials(&mut self, materials: MaterialTable) {
        self.materials = Some(materials);
    }

    /// Builder-style [`Self::set_materials`].
    pub fn with_materials(mut self, materials: MaterialTable) -> Self {
        self.set_materials(materials);
        self
    }

    /// Registers a block, returning its dense id.
    ///
    /// Fails with [`RegistryError::AirMustBeFirst`] if the very first block is
    /// not air, preserving `MaterialId 0` as the "empty" sentinel.
    ///
    /// O(1): a single `entry` probe replaces the old `contains_key` + `insert`
    /// pair, so the key is hashed once rather than twice per registration.
    pub fn register(
        &mut self,
        key: NamespacedKey,
        mut properties: BlockProperties,
    ) -> Result<MaterialId, RegistryError> {
        if self.entries.is_empty() && key != BlockKind::Air.key() {
            return Err(RegistryError::AirMustBeFirst);
        }
        if self.entries.len() > MaterialId::MAX.as_raw() as usize {
            return Err(RegistryError::Exhausted);
        }

        let id = MaterialId::from_raw(self.entries.len() as u16);
        // One hash, not two: `entry` probes the map and hands back the slot to
        // fill. Deriving the display name inside `Vacant` keeps it off the
        // duplicate-error path, where it would be wasted work.
        match self.index.entry(key.clone()) {
            Entry::Occupied(_) => Err(RegistryError::DuplicateKey(key)),
            Entry::Vacant(slot) => {
                if properties.display_name.is_empty() {
                    properties.display_name = derive_display_name(key.path());
                }
                slot.insert(id);
                self.entries.push((Some(key), properties));
                Ok(id)
            }
        }
    }

    /// Registers a built-in [`BlockKind`] with its default properties.
    pub fn register_kind(&mut self, kind: BlockKind) -> Result<MaterialId, RegistryError> {
        // `register` enforces the air-at-zero invariant, so no extra check
        // is needed here.
        self.register(kind.key(), kind.default_properties())
    }

    /// True if a block with this key is already registered.
    ///
    /// O(1).
    pub fn contains(&self, key: &NamespacedKey) -> bool {
        self.index.contains_key(key)
    }

    /// Resolves an already-registered key to its id, in O(1).
    pub fn id(&self, key: &NamespacedKey) -> Option<MaterialId> {
        self.index.get(key).copied()
    }

    /// Number of blocks registered so far.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Holds the next `count` ids without assigning them to a key, returning
    /// the id the reservation starts at.
    ///
    /// Use this to keep a span of ids free — for future built-ins, a mod
    /// loader, or a fixed engine/mod split — so that filling it later *cannot*
    /// renumber blocks a save already references by raw `u16`. Without a
    /// reservation, inserting any block before an existing one shifts every id
    /// after it and silently corrupts stored chunk data.
    ///
    /// Reserved slots still occupy their `Vec` position (with no key and
    /// invisible properties), because dense indexing is what keeps `get()` an
    /// O(1) array access; a literal hole would cost a range check on every
    /// voxel lookup. They are simply absent from the key index, so
    /// [`Self::contains`] and [`Self::id`] never see them.
    ///
    /// Reserving `0` returns the next id and changes nothing.
    pub fn reserve(&mut self, count: usize) -> Result<MaterialId, RegistryError> {
        // Air must already hold id 0, or the reserved span would push it off
        // the sentinel.
        if self.entries.is_empty() {
            return Err(RegistryError::AirMustBeFirst);
        }

        // Total slot count: ids run 0..=65535.
        const SLOTS: usize = MaterialId::MAX.as_raw() as usize + 1;

        let start = self.entries.len();
        // A full registry has no "next id", so even `reserve(0)` must fail
        // rather than wrap `start as u16` back to 0 (air's sentinel).
        if start >= SLOTS {
            return Err(RegistryError::Exhausted);
        }
        let end = start.checked_add(count).ok_or(RegistryError::Exhausted)?;
        if end > SLOTS {
            return Err(RegistryError::Exhausted);
        }

        self.entries.resize_with(end, || {
            (None, BlockProperties::new().with_display_name(RESERVED))
        });

        Ok(MaterialId::from_raw(start as u16))
    }

    /// Freezes the builder into an immutable registry.
    ///
    /// Moves both collections out as-is — the index built during registration
    /// becomes the registry's lookup table, so no keys are re-hashed or
    /// cloned.
    ///
    /// This is also where the one-time resolution boundary sits: the
    /// authoring data collected during registration (texture *keys*) is
    /// resolved into runtime data (texture *ids*) exactly here, and never
    /// again. Both shapes are kept: the authoring one for tooling and mods,
    /// the resolved one for per-voxel code.
    ///
    /// # Errors
    ///
    /// Fails with [`FreezeError::TooManyTextures`] if the blocks refer to
    /// more distinct textures than a `u16` id can address. The failure is
    /// reported, not panicked: a mod that pushes the registry past the limit
    /// is a recoverable startup error, and the caller decides what to do
    /// about it.
    pub fn freeze(self) -> Result<BlockRegistry, FreezeError> {
        let BlockRegistryBuilder {
            entries,
            index,
            materials,
        } = self;
        let (keys, properties): (Vec<Option<NamespacedKey>>, Vec<BlockProperties>) =
            entries.into_iter().unzip();
        // `OPAQUE` is the authoritative "blocks light" flag: normalize the
        // authoring value so the cold properties, the hot record and the docs
        // ("15 for a full blocker") agree even when the data omits it.
        let properties: Vec<BlockProperties> = properties
            .into_iter()
            .map(|mut properties| {
                if properties.flags.contains(BlockFlags::OPAQUE) {
                    properties.light_opacity = 15;
                }
                properties
            })
            .collect();
        let Resolved {
            hot,
            templates,
            texture_sets,
            textures,
        } = resolve(&properties)?;
        let mut registry = BlockRegistry {
            keys,
            properties,
            hot,
            templates,
            texture_sets,
            textures,
            materials: None,
            by_key: index,
        };
        if let Some(table) = materials {
            let material_registry = MaterialRegistry::from_generated(&table, &registry)
                .map_err(FreezeError::Material)?;
            registry.materials = Some(material_registry);
        }
        Ok(registry)
    }
}

/// The runtime tables produced by [`resolve`].
struct Resolved {
    hot: Vec<HotRecord>,
    templates: HashMap<MaterialId, u64>,
    texture_sets: Vec<FaceTextureIds>,
    textures: TextureTable,
}

/// One-time resolution of authoring data into runtime tables.
///
/// Walks the blocks in id order, so a texture's id (and a texture set's id)
/// is a deterministic function of the registration order: the same blocks
/// registered in the same order always resolve to the same ids.
///
/// Every key a block refers to lands in the texture table exactly once, and
/// every distinct six-face combination lands in the set table exactly once,
/// no matter how many blocks or faces reference it.
fn resolve(properties: &[BlockProperties]) -> Result<Resolved, FreezeError> {
    let mut table = TextureTable::default();
    let mut hot = Vec::with_capacity(properties.len());
    let mut templates = HashMap::new();
    let mut texture_sets: Vec<FaceTextureIds> = Vec::new();
    let mut set_ids: HashMap<FaceTextureIds, TextureSetId> = HashMap::new();

    for (index, props) in properties.iter().enumerate() {
        let set = match &props.textures {
            None => None,
            Some(faces) => {
                let mut ids = [TextureId::default(); 6];
                for face in Face::ALL {
                    ids[face.index()] =
                        table
                            .insert(faces.get(face))
                            .ok_or(FreezeError::TooManyTextures {
                                count: table.len() + 1,
                            })?;
                }
                let ids = FaceTextureIds::new(ids);
                // At most one set per block, and a registry holds at most
                // 65 536 blocks, so the set index always fits in a u16.
                let id = match set_ids.entry(ids) {
                    Entry::Occupied(slot) => *slot.get(),
                    Entry::Vacant(slot) => {
                        let id = TextureSetId::from_raw(texture_sets.len() as u16);
                        texture_sets.push(ids);
                        *slot.insert(id)
                    }
                };
                Some(id)
            }
        };

        if let Some(root) = props.template_root {
            templates.insert(MaterialId::from_raw(index as u16), root);
        }

        hot.push(HotRecord::pack(
            props.render_type,
            props.flags,
            props.light_emission,
            props.light_opacity,
            set,
        ));
    }

    Ok(Resolved {
        hot,
        templates,
        texture_sets,
        textures: table,
    })
}

/// Immutable mapping between [`NamespacedKey`]s, [`MaterialId`]s and
/// [`BlockProperties`].
///
/// Has no mutating methods by construction: the only way to build one is to
/// freeze a [`BlockRegistryBuilder`]. All lookups are `O(1)`.
///
/// The registry holds two views of the same blocks, one per domain:
///
/// - the *authoring* view ([`Self::get`]) — keys and strings, kept whole,
///   for mods, tooling and the texture-array builder;
/// - the *runtime* view — split by access pattern: an 8-byte [`HotRecord`]
///   per block ([`Self::hot`]), deduplicated texture sets read on a surface
///   hit ([`Self::texture_set`]), and sparse template roots read at
///   placement ([`Self::template_root`]). [`Self::resolved`] assembles all
///   of it for code that wants the whole block.
#[derive(Debug)]
pub struct BlockRegistry {
    /// `keys[i]` and `properties[i]` describe `MaterialId(i)`.
    ///
    /// `keys[i]` is `None` for a reserved slot: it occupies its position so
    /// indexing stays dense, but has no identity to look up.
    keys: Vec<Option<NamespacedKey>>,
    /// Authoring-shape data, kept whole. The domain mods and tooling read.
    properties: Vec<BlockProperties>,
    /// Hot runtime data, indexed like `properties`: 8 bytes per block.
    hot: Vec<HotRecord>,
    /// Template roots, sparse: most blocks are flat materials with none.
    templates: HashMap<MaterialId, u64>,
    /// Each distinct six-face texture set, once; `TextureSetId` indexes it.
    texture_sets: Vec<FaceTextureIds>,
    /// The resolved texture names behind the ids in `texture_sets`.
    textures: TextureTable,
    /// Atlas-slot material variants, when a `materials.gen.ron` table was
    /// attached to the builder.
    materials: Option<MaterialRegistry>,
    by_key: HashMap<NamespacedKey, MaterialId>,
}

impl BlockRegistry {
    /// Properties for an id, or `None` if the id was never registered.
    ///
    /// Reserved slots return their (invisible) placeholder properties, so a
    /// meshing loop does not need a special case for them.
    ///
    /// This is the authoring view: keys and strings, kept whole, for mods,
    /// tooling and the texture-array builder.
    ///
    /// # Why an `Option`
    ///
    /// Ids reach this accessor from *outside* the registry — a data file, a
    /// save, a command, user input — where "no such block" is a real answer
    /// the caller has to handle, not a bug. Returning `None` keeps that
    /// decision with the caller instead of hiding it behind a panic.
    ///
    /// # The runtime view
    ///
    /// Per-voxel code wants [`Self::resolved`] instead: the same block with
    /// its textures already resolved to ids, in a record small enough to read
    /// without a pointer chase.
    pub fn get(&self, id: MaterialId) -> Option<&BlockProperties> {
        self.properties.get(id.index())
    }

    /// The hot runtime record for an id: 8 bytes, returned by value.
    ///
    /// This is the per-voxel / per-hit accessor. It is one indexed load from a
    /// dense table that is 512 KiB for a full 65 536-block registry.
    ///
    /// # Why not an `Option`
    ///
    /// Ids reach this accessor from the registry's *own* dense id space — a
    /// chunk stores the ids the registry handed out — so an id that is not
    /// registered is a bug, not a case to handle. A plain value says so in the
    /// type: the hot path has no `Option` to unwrap on every voxel, and an
    /// out-of-range id fails loudly instead of silently becoming "no block".
    ///
    /// Absence is still *reported*, just not here: [`Self::get`] and
    /// [`Self::id`] are the paths that answer "is this block registered?"
    ///
    /// # Panics
    ///
    /// Panics if the id was never registered — a corrupted save or a bug in
    /// the caller.
    #[inline]
    pub fn hot(&self, id: MaterialId) -> HotRecord {
        match self.hot.get(id.index()) {
            Some(record) => *record,
            None => self.unregistered(id),
        }
    }

    #[cold]
    #[inline(never)]
    fn unregistered(&self, id: MaterialId) -> ! {
        panic!(
            "material id {id} is outside the registry's {} blocks",
            self.hot.len()
        )
    }

    /// The whole hot table, indexed by [`MaterialId`], for bulk work.
    pub fn hot_table(&self) -> &[HotRecord] {
        &self.hot
    }

    /// A block's six resolved face textures, or `None` if it never draws.
    ///
    /// Two dependent loads (hot record, then set). Read this on a surface hit,
    /// not per voxel.
    ///
    /// # Panics
    ///
    /// Panics if the id was never registered.
    #[inline]
    pub fn face_textures(&self, id: MaterialId) -> Option<FaceTextureIds> {
        self.hot(id).texture_set().map(|set| self.texture_set(set))
    }

    /// The atlas material variants for a block, when a material table was
    /// attached with [`BlockRegistryBuilder::set_materials`].
    ///
    /// `None` means either "no table attached" or "this block never draws".
    #[inline]
    pub fn face_materials(&self, id: MaterialId) -> Option<FaceMaterials> {
        self.materials
            .as_ref()
            .and_then(|materials| materials.faces(id))
    }

    /// The attached material registry, if any.
    pub fn material_registry(&self) -> Option<&MaterialRegistry> {
        self.materials.as_ref()
    }

    /// A texture set by id.
    ///
    /// # Panics
    ///
    /// Panics if the id did not come from this registry.
    #[inline]
    pub fn texture_set(&self, set: TextureSetId) -> FaceTextureIds {
        self.texture_sets[set.index()]
    }

    /// Every distinct texture set, in [`TextureSetId`] order.
    pub fn texture_sets(&self) -> &[FaceTextureIds] {
        &self.texture_sets
    }

    /// The SVDAG root of a block's sub-block template, or `None` for a flat
    /// material (or an id that was never registered).
    ///
    /// Placement-time data: a hash lookup, deliberately kept out of the hot
    /// table.
    pub fn template_root(&self, id: MaterialId) -> Option<u64> {
        self.templates.get(&id).copied()
    }

    /// The whole block, assembled from the hot record, texture sets and
    /// template table. Flags are the full 32 bits from the authoring data.
    ///
    /// For tooling, placement and tests. Per-voxel code wants [`Self::hot`].
    ///
    /// # Panics
    ///
    /// Panics if the id was never registered.
    pub fn resolved(&self, id: MaterialId) -> BlockRecord {
        let hot = self.hot(id);
        BlockRecord {
            textures: hot.texture_set().map(|set| self.texture_set(set)),
            materials: self.face_materials(id),
            template_root: self.template_root(id),
            render_type: hot.render_type(),
            flags: self.properties[id.index()].flags,
            light_emission: hot.light_emission(),
            light_opacity: hot.light_opacity(),
        }
    }

    /// The hot table as raw `u32` words (two per block), ready to upload to a
    /// GPU storage buffer. Layout: see [`crate::hot`].
    pub fn gpu_hot_words(&self) -> Vec<u32> {
        self.hot.iter().flat_map(|h| h.to_words()).collect()
    }

    /// The texture sets as raw `u32` words (three per set), ready to upload to
    /// a GPU storage buffer. Layout: see [`FaceTextureIds::to_words`].
    pub fn gpu_texture_set_words(&self) -> Vec<u32> {
        self.texture_sets
            .iter()
            .flat_map(|s| s.to_words())
            .collect()
    }

    /// The resolved texture table: `TextureId` to key.
    ///
    /// The texture-array builder walks this to turn texture ids into layers of
    /// the GPU `Texture2DArray`; nothing else needs the mapping back to keys.
    pub fn textures(&self) -> &TextureTable {
        &self.textures
    }

    /// Key for an id, or `None` if the id was never registered or is a
    /// reserved slot.
    pub fn key(&self, id: MaterialId) -> Option<&NamespacedKey> {
        self.keys.get(id.index()).and_then(Option::as_ref)
    }

    /// True if this id is held by a reservation: present in the dense id space
    /// but deliberately unassigned.
    ///
    /// `keys` and `properties` are built together by [`BlockRegistryBuilder::freeze`]
    /// and are always the same length, so "the key slot exists but is empty"
    /// is exactly "reserved".
    pub fn is_reserved(&self, id: MaterialId) -> bool {
        // Outer `Some`: in bounds. Inner `None`: no key was ever assigned.
        matches!(self.keys.get(id.index()), Some(None))
    }

    /// Resolves a key to its id.
    pub fn id(&self, key: &NamespacedKey) -> Option<MaterialId> {
        self.by_key.get(key).copied()
    }

    /// Resolves a `namespace:path` string to its id.
    pub fn id_of(&self, key: &str) -> Option<MaterialId> {
        self.by_key.get(&NamespacedKey::parse(key).ok()?).copied()
    }

    /// The reserved air id; always present.
    pub fn air_id(&self) -> MaterialId {
        AIR_ID
    }

    /// Number of id slots in the dense id space, including reserved ones.
    ///
    /// This equals the size of the id space a chunk can address, which is why
    /// reserved slots are counted: they occupy real ids even though they have
    /// no key. Use [`Self::is_reserved`] to tell assigned from held slots.
    pub fn len(&self) -> usize {
        self.properties.len()
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    /// Iterates the dense id space as `(id, properties)` in id order,
    /// including reserved slots (which carry invisible placeholder
    /// properties).
    pub fn iter(&self) -> impl Iterator<Item = (MaterialId, &BlockProperties)> {
        self.properties
            .iter()
            .enumerate()
            .map(|(index, props)| (MaterialId::from_raw(index as u16), props))
    }

    /// Iterates all registered keys in id order, skipping reserved slots.
    pub fn keys(&self) -> impl Iterator<Item = (MaterialId, &NamespacedKey)> {
        self.keys.iter().enumerate().filter_map(|(index, key)| {
            key.as_ref()
                .map(|key| (MaterialId::from_raw(index as u16), key))
        })
    }
}

/// Display name given to slots held by [`BlockRegistryBuilder::reserve`].
///
/// Reserved slots have no key, so this is the only way they are distinguishable
/// in tooling that iterates [`BlockRegistry::iter`].
const RESERVED: &str = "Reserved";

/// `"deep_copper_ore"` -> `"Deep copper ore"`.
fn derive_display_name(path: &str) -> String {
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    let mut name = String::with_capacity(last_segment.len());
    let mut chars = last_segment.chars();
    if let Some(first) = chars.next() {
        name.extend(first.to_uppercase());
    }
    for c in chars {
        name.push(if c == '_' { ' ' } else { c });
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::BUILTIN_NAMESPACE;
    use crate::properties::{BlockFlags, FaceTextures, RenderType};

    #[test]
    fn air_is_always_id_zero() {
        let registry = BlockRegistryBuilder::new().freeze().unwrap();
        assert_eq!(registry.air_id(), AIR_ID);
        assert_eq!(registry.id(&BlockKind::Air.key()), Some(AIR_ID));
        assert!(
            registry
                .get(AIR_ID)
                .unwrap()
                .flags
                .contains(BlockFlags::REPLACEABLE)
        );
    }

    #[test]
    fn air_id_is_the_sentinel_zero() {
        assert!(AIR_ID.is_air());
        assert!(MaterialId::from_raw(0).is_air());
        assert!(!MaterialId::from_raw(1).is_air());

        // The registry agrees: id 0 is a real, registered air block, not just
        // a magic out-of-band value.
        let registry = BlockRegistryBuilder::new().freeze().unwrap();
        assert!(registry.get(AIR_ID).is_some());
        assert!(!registry.get(AIR_ID).unwrap().is_solid());
    }

    #[test]
    fn builder_duplicate_check_is_index_backed() {
        // Registers many blocks in a pattern that would be pathologically slow
        // if duplicate detection scanned the entry list each time. Guards the
        // O(1) `index` map, not the timing itself.
        let mut builder = BlockRegistryBuilder::new();
        for i in 0..5_000u32 {
            let key = NamespacedKey::new("bench", &format!("block_{i}")).unwrap();
            let id = builder
                .register(key.clone(), BlockProperties::new())
                .expect("unique keys register");
            assert_eq!(builder.id(&key), Some(id));
            assert!(builder.contains(&key));
        }
        assert_eq!(builder.len(), 5_001);

        // And the last-registered key is still found, not just the first.
        let last = NamespacedKey::new("bench", "block_4999").unwrap();
        assert_eq!(builder.id(&last), Some(MaterialId::from_raw(5_000)));
    }

    #[test]
    fn builder_refuses_non_air_at_id_zero() {
        let mut builder = BlockRegistryBuilder::default();
        let err = builder
            .register_kind(BlockKind::Stone)
            .expect_err("stone must not be allowed to take id 0");
        assert_eq!(err, RegistryError::AirMustBeFirst);

        // Air is still accepted as the first block on a fresh builder.
        assert!(builder.register_kind(BlockKind::Air).is_ok());
        assert!(builder.register_kind(BlockKind::Stone).is_ok());
    }

    #[test]
    fn ids_are_dense_and_lookup_by_key() {
        let mut builder = BlockRegistryBuilder::new();
        let stone = builder
            .register_kind(BlockKind::Stone)
            .expect("stone registers");
        let water = builder
            .register_kind(BlockKind::Water)
            .expect("water registers");

        assert_eq!(stone.as_raw(), 1);
        assert_eq!(water.as_raw(), 2);

        let registry = builder.freeze().unwrap();
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.id(&BlockKind::Stone.key()), Some(stone));
        assert_eq!(registry.key(stone).unwrap().path(), "stone");
        assert_eq!(registry.id_of("voxel:water"), Some(water));
        assert_eq!(registry.id_of("mymod:nonexistent"), None);
        assert!(registry.get(MaterialId::from_raw(99)).is_none());
    }

    /// Registration order *is* id assignment (`id = entries.len()`), so an id
    /// is session-local: the same key earns different ids when the order
    /// changes. This is the invariant that forbids persisting a bare id — a
    /// save written by the first registry below would silently re-point every
    /// block after `dirt` if read by the second.
    #[test]
    fn ids_are_session_local_because_registration_order_assigns_them() {
        let stone = BlockKind::Stone.key();
        let dirt = BlockKind::Dirt.key();

        let mut first = BlockRegistryBuilder::new();
        first.register_kind(BlockKind::Stone).expect("stone");
        first.register_kind(BlockKind::Dirt).expect("dirt");

        let mut second = BlockRegistryBuilder::new();
        second.register_kind(BlockKind::Dirt).expect("dirt");
        second.register_kind(BlockKind::Stone).expect("stone");

        let (a, b) = (first.freeze().unwrap(), second.freeze().unwrap());

        // Neither registry is wrong — both are dense, both keep air at 0.
        assert_eq!(a.id(&NamespacedKey::builtin("air")), Some(AIR_ID));
        assert_eq!(b.id(&NamespacedKey::builtin("air")), Some(AIR_ID));

        // But the same key maps to a different number in each.
        assert_ne!(a.id(&stone), b.id(&stone));
        assert_ne!(a.id(&dirt), b.id(&dirt));
        assert_eq!(a.id(&stone), Some(MaterialId::from_raw(1)));
        assert_eq!(b.id(&stone), Some(MaterialId::from_raw(2)));

        // The key, by contrast, is the same identity in both — which is why
        // save data must round-trip through it (or an explicit palette).
        assert_eq!(a.key(MaterialId::from_raw(1)), Some(&stone));
        assert_eq!(b.key(MaterialId::from_raw(2)), Some(&stone));
    }

    /// A save is a set of ids plus the key each id stood for. Loading it into
    /// a registry that registered the same blocks in a *different* order has
    /// to remap through those keys — and the remap has to reach the textures
    /// too, because texture ids are as session-local as material ids.
    #[test]
    fn a_save_remaps_through_keys_into_a_reverse_ordered_registry() {
        let mut writer = BlockRegistryBuilder::new();
        let stone = writer.register_kind(BlockKind::Stone).unwrap();
        let dirt = writer.register_kind(BlockKind::Dirt).unwrap();
        let grass = writer.register_kind(BlockKind::Grass).unwrap();
        let sand = writer.register_kind(BlockKind::Sand).unwrap();
        let written = writer.freeze().unwrap();

        // What the save file holds: the ids it saw, and the key behind each.
        let save: Vec<(MaterialId, NamespacedKey)> = [stone, dirt, grass, sand]
            .into_iter()
            .map(|id| (id, written.key(id).unwrap().clone()))
            .collect();

        // The next session registers the same blocks in the opposite order.
        let mut reader = BlockRegistryBuilder::new();
        let sand2 = reader.register_kind(BlockKind::Sand).unwrap();
        let grass2 = reader.register_kind(BlockKind::Grass).unwrap();
        let dirt2 = reader.register_kind(BlockKind::Dirt).unwrap();
        let stone2 = reader.register_kind(BlockKind::Stone).unwrap();
        let loaded = reader.freeze().unwrap();

        // Every id really did move: reversing four blocks moves every one of
        // them. (Reversing three would leave the middle block in place, which
        // is why this uses four.)
        assert_ne!(stone, stone2);
        assert_ne!(dirt, dirt2);
        assert_ne!(grass, grass2);
        assert_ne!(sand, sand2);

        for (old_id, key) in &save {
            // The remap: key -> the id this session uses.
            let new_id = loaded.id(key).expect("the key is in the new registry");
            assert_eq!(loaded.key(new_id), Some(key));

            // The remapped block is the same block, textures included. The
            // texture *ids* are this session's, so they are compared by key
            // and never by number.
            let old_faces = written
                .resolved(*old_id)
                .textures
                .expect("the block has textures");
            let new_faces = loaded
                .resolved(new_id)
                .textures
                .expect("the block has textures");
            for face in Face::ALL {
                let old_key = written
                    .textures()
                    .get(old_faces.get(face))
                    .expect("in the table");
                let new_key = loaded
                    .textures()
                    .get(new_faces.get(face))
                    .expect("in the table");
                assert_eq!(old_key, new_key, "the same texture, under another id");
            }
        }

        // Stone was registered first when written and last when read, so even
        // its texture id moved with it.
        assert_ne!(
            written.resolved(stone).textures.unwrap().get(Face::North),
            loaded.resolved(stone2).textures.unwrap().get(Face::North),
        );

        // The save carries *namespaced* keys, and the namespace is part of
        // the identity: a key from another namespace is a different block,
        // not another spelling of the same one. A namespace rename therefore
        // cannot silently change what an old save means.
        for (_, key) in &save {
            assert_eq!(key.namespace(), BUILTIN_NAMESPACE);
        }
        // "minecraft" is the namespace this crate *used* to use, deliberately
        // kept here as the negative case: a save written before the rename
        // must not resolve against the renamed builtins.
        assert!(
            loaded
                .id(&NamespacedKey::new("minecraft", "block/stone").expect("valid key"))
                .is_none(),
            "a key in another namespace must not resolve to the same block"
        );
    }

    /// The texture id space is `u16`, so one registry may reference at most
    /// 65 536 distinct textures. A mod that pushes past that must be
    /// *reported*, not panic the process: the caller can drop the mod and
    /// carry on.
    #[test]
    fn freezing_reports_a_texture_table_overflow_instead_of_panicking() {
        let mut builder = BlockRegistryBuilder::new();

        // Six distinct textures per block, so the table fills long before the
        // u16 material id space does.
        for i in 0..11_000u32 {
            let textures = FaceTextures {
                top: NamespacedKey::builtin(&format!("block/t{i}_top")),
                bottom: NamespacedKey::builtin(&format!("block/t{i}_bottom")),
                north: NamespacedKey::builtin(&format!("block/t{i}_north")),
                south: NamespacedKey::builtin(&format!("block/t{i}_south")),
                east: NamespacedKey::builtin(&format!("block/t{i}_east")),
                west: NamespacedKey::builtin(&format!("block/t{i}_west")),
            };
            builder
                .register(
                    NamespacedKey::builtin(&format!("block/b{i}")),
                    BlockProperties::new().with_textures(textures),
                )
                .unwrap();
        }

        let err = builder
            .freeze()
            .expect_err("65 536 textures cannot be indexed by a u16");
        assert_eq!(err, FreezeError::TooManyTextures { count: 65_536 + 1 });
    }

    #[test]
    fn frozen_registry_stores_each_key_allocation_once() {
        // `register` keeps a key in both `entries` (-> `keys`) and `index`
        // (-> `by_key`). With `Arc<str>` those must be two handles to ONE
        // string allocation, not two copies of the string.
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let id = registry.id(&BlockKind::Stone.key()).expect("stone");

        // Compare the two *independent* retained copies: the Vec element that
        // backs `key(id)`, and the key stored inside the lookup HashMap.
        let in_vec = registry.key(id).expect("key for id");
        let in_map = registry
            .by_key
            .get_key_value(&BlockKind::Stone.key())
            .expect("key present in lookup map")
            .0;

        assert_eq!(
            in_vec.arc_ptr(),
            in_map.arc_ptr(),
            "keys Vec and by_key map must alias the same allocation",
        );
        // Both registry handles are live; the caller's temporary from
        // `BlockKind::Stone.key()` has already been dropped.
        assert!(
            in_vec.refcount() >= 2,
            "expected >= 2 live handles, got {}",
            in_vec.refcount(),
        );
    }

    #[test]
    fn with_capacity_preserves_sentinel_and_dense_ids() {
        // Reserving must not change observable behaviour: air still at 0,
        // subsequent ids still dense and in registration order.
        let mut builder = BlockRegistryBuilder::with_capacity(3);
        assert_eq!(builder.len(), 1, "air is pre-registered");
        assert_eq!(builder.register_kind(BlockKind::Stone).unwrap().as_raw(), 1);
        assert_eq!(builder.register_kind(BlockKind::Dirt).unwrap().as_raw(), 2);

        let registry = builder.freeze().unwrap();
        assert_eq!(registry.air_id(), AIR_ID);
        assert_eq!(registry.len(), 3);
        assert_eq!(
            registry.id(&BlockKind::Stone.key()),
            Some(MaterialId::from_raw(1))
        );
    }

    #[test]
    fn with_capacity_zero_still_accepts_air() {
        // `new()` delegates to `with_capacity(0)`; air must still fit.
        let builder = BlockRegistryBuilder::with_capacity(0);
        assert_eq!(builder.len(), 1);
    }

    #[test]
    fn with_capacity_does_not_over_reserve_observable_state() {
        // Reserving more than used must not inflate len() or create phantom
        // entries that iterate as registered blocks.
        let mut builder = BlockRegistryBuilder::with_capacity(64);
        builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        assert_eq!(registry.len(), 2, "only air + stone are registered");
        assert!(registry.get(MaterialId::from_raw(63)).is_none());
    }

    #[test]
    fn reserve_holds_ids_and_register_lands_after_the_span() {
        let mut builder = BlockRegistryBuilder::new();
        let start = builder.reserve(4).unwrap();
        assert_eq!(start.as_raw(), 1, "reservation starts after air");

        // The next real registration must land *after* the held span, not in it.
        let stone = builder.register_kind(BlockKind::Stone).unwrap();
        assert_eq!(stone.as_raw(), 5);

        let registry = builder.freeze().unwrap();
        assert_eq!(registry.len(), 6, "air + 4 reserved + stone");
        assert_eq!(registry.id(&BlockKind::Stone.key()), Some(stone));
        // Air's sentinel is untouched by the reservation.
        assert_eq!(registry.air_id(), AIR_ID);
        assert!(!registry.is_reserved(AIR_ID));
    }

    #[test]
    fn reserved_slots_are_invisible_and_keyless() {
        let mut builder = BlockRegistryBuilder::new();
        builder.reserve(2).unwrap();
        let registry = builder.freeze().unwrap();

        let reserved = MaterialId::from_raw(1);
        assert!(registry.is_reserved(reserved));
        assert!(!registry.is_reserved(AIR_ID));

        // No key: invisible to key lookup and to the keys() iterator, so
        // nothing can resolve to a reserved slot by name.
        assert!(registry.key(reserved).is_none());
        assert_eq!(registry.keys().count(), 1, "only air has a key");

        // But present in the dense id space with placeholder properties, so a
        // meshing loop's get() never panics on a reserved id.
        let props = registry
            .get(reserved)
            .expect("reserved slot occupies its id");
        assert_eq!(props.render_type, RenderType::Invisible);
        assert!(!props.is_solid());
        assert_eq!(props.display_name, RESERVED);
    }

    #[test]
    fn reserve_zero_is_a_noop_that_returns_the_next_id() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let before = builder.len();

        let next = builder.reserve(0).unwrap();
        assert_eq!(next.as_raw(), before as u16);
        assert_eq!(builder.len(), before, "reserve(0) must not grow the space");
    }

    #[test]
    fn reserve_requires_air_first() {
        // Reserving on a builder with no air would push air off id 0.
        let mut builder = BlockRegistryBuilder::default();
        let err = builder
            .reserve(4)
            .expect_err("reserve before air must fail");
        assert_eq!(err, RegistryError::AirMustBeFirst);
    }

    #[test]
    fn reserve_detects_exhaustion() {
        let mut builder = BlockRegistryBuilder::new();
        // One past the last valid id: `start + count` exceeds the u16 space.
        let err = builder
            .reserve(MaterialId::MAX.as_raw() as usize + 1)
            .expect_err("reserving past the id space must fail");
        assert_eq!(err, RegistryError::Exhausted);
        assert_eq!(builder.len(), 1, "a failed reserve must not grow the space");
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let err = builder
            .register_kind(BlockKind::Stone)
            .expect_err("duplicate registration must fail");
        assert!(matches!(err, RegistryError::DuplicateKey(_)));
        // The failed registration must not have consumed an id.
        assert_eq!(builder.len(), 2);
    }

    #[test]
    fn custom_blocks_register_with_derived_display_name() {
        let mut builder = BlockRegistryBuilder::new();
        let key = NamespacedKey::new("mymod", "deep_copper_ore").unwrap();
        let id = builder
            .register(
                key.clone(),
                BlockProperties::new().with_render_type(RenderType::Solid),
            )
            .unwrap();

        let registry = builder.freeze().unwrap();
        assert_eq!(registry.id(&key), Some(id));
        assert_eq!(registry.get(id).unwrap().display_name, "Deep copper ore");
    }

    #[test]
    fn iter_covers_every_id_in_order() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let ids: Vec<MaterialId> = registry.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![AIR_ID, MaterialId::from_raw(1)]);
        for (id, props) in registry.iter() {
            assert!(registry.key(id).is_some());
            assert!(!props.display_name.is_empty());
        }
    }

    // --- the resolution boundary -----------------------------------------

    #[test]
    fn the_resolved_record_mirrors_the_authoring_properties() {
        let mut builder = BlockRegistryBuilder::new();
        let stone = builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let props = registry.get(stone).expect("stone");
        let record = registry.resolved(stone);

        assert_eq!(record.render_type, props.render_type);
        assert_eq!(record.flags, props.flags);
        assert_eq!(record.light_emission, props.light_emission);
        assert_eq!(record.light_opacity, props.light_opacity);
        assert_eq!(record.template_root, props.template_root);
        assert!(record.textures.is_some());
    }

    #[test]
    fn a_template_root_is_resolved_into_the_record() {
        let mut builder = BlockRegistryBuilder::new();
        let stairs = builder
            .register(
                NamespacedKey::builtin("stairs"),
                BlockProperties::new().with_template_root(0xDEAD_BEEF),
            )
            .unwrap();
        let registry = builder.freeze().unwrap();

        assert_eq!(
            registry.get(stairs).unwrap().template_root,
            Some(0xDEAD_BEEF)
        );
        assert_eq!(registry.resolved(stairs).template_root, Some(0xDEAD_BEEF));
        assert_eq!(registry.resolved(AIR_ID).template_root, None);
    }

    #[test]
    fn one_texture_on_every_face_costs_one_table_entry() {
        let mut builder = BlockRegistryBuilder::new();
        let stone = builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let faces = registry
            .resolved(stone)
            .textures
            .expect("stone has textures");

        let first = faces.get(Face::North);
        for face in Face::ALL {
            assert_eq!(faces.get(face), first, "{face} should use one texture");
        }

        assert_eq!(
            registry.textures().len(),
            1,
            "six references to one texture, one table entry",
        );
    }

    #[test]
    fn a_column_resolves_to_two_distinct_ids() {
        let mut builder = BlockRegistryBuilder::new();
        let log = builder.register_kind(BlockKind::OakLog).unwrap();
        let registry = builder.freeze().unwrap();

        let faces = registry.resolved(log).textures.expect("log has textures");

        let cap = faces.get(Face::Top);
        let side = faces.get(Face::North);

        assert_eq!(cap, faces.get(Face::Bottom), "a log's caps match");
        assert_ne!(cap, side, "the side is a different texture");
        assert_eq!(registry.textures().len(), 2);
    }

    #[test]
    fn every_distinct_face_texture_gets_its_own_id() {
        // Grass has three: a top cap, a side, and a dirt bottom cap. The
        // resolution must not collapse the two caps into one, because they
        // are different textures.
        let mut builder = BlockRegistryBuilder::new();
        let grass = builder.register_kind(BlockKind::Grass).unwrap();
        let registry = builder.freeze().unwrap();

        let faces = registry
            .resolved(grass)
            .textures
            .expect("grass has textures");

        let top = faces.get(Face::Top);
        let bottom = faces.get(Face::Bottom);
        let side = faces.get(Face::North);

        assert_ne!(top, bottom);
        assert_ne!(top, side);
        assert_ne!(bottom, side);
        assert_eq!(registry.textures().len(), 3);
    }

    #[test]
    fn a_texture_shared_by_two_blocks_resolves_to_one_id() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let copy = builder
            .register(
                NamespacedKey::builtin("stone_copy"),
                BlockProperties::new()
                    .with_render_type(RenderType::Solid)
                    .with_textures(FaceTextures::all(NamespacedKey::builtin("block/stone"))),
            )
            .unwrap();
        let registry = builder.freeze().unwrap();

        let table = registry.textures();
        assert_eq!(table.len(), 1, "one texture, two blocks");

        let stone = registry.resolved(MaterialId::from_raw(1));
        let copy = registry.resolved(copy);
        assert_eq!(
            stone.textures.unwrap().get(Face::North),
            copy.textures.unwrap().get(Face::North),
        );
    }

    #[test]
    fn a_block_without_textures_resolves_to_none() {
        let registry = BlockRegistryBuilder::new().freeze().unwrap();

        let air = registry.resolved(AIR_ID);
        assert!(air.textures.is_none());
        assert_eq!(air.render_type, RenderType::Invisible);
    }

    /// `resolved()` trusts the id: an id that was never registered is a bug,
    /// not a case to handle, and it stops the process rather than rendering
    /// air. The authoring accessor [`BlockRegistry::get`] is the one that
    /// reports absence.
    #[test]
    #[should_panic(expected = "outside the registry")]
    fn resolving_an_unregistered_id_panics() {
        let registry = BlockRegistryBuilder::new().freeze().unwrap();
        let _ = registry.resolved(MaterialId::from_raw(99));
    }

    #[test]
    fn a_reserved_slot_resolves_to_a_texture_less_record() {
        let mut builder = BlockRegistryBuilder::new();
        builder.reserve(2).unwrap();
        let registry = builder.freeze().unwrap();

        let record = registry.resolved(MaterialId::from_raw(1));
        assert!(record.textures.is_none());
        assert_eq!(record.render_type, RenderType::Invisible);
        assert!(!record.flags.contains(BlockFlags::SOLID));
    }

    #[test]
    fn records_stay_parallel_to_properties() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        builder.reserve(3).unwrap();
        builder.register_kind(BlockKind::Dirt).unwrap();
        let registry = builder.freeze().unwrap();

        assert_eq!(registry.hot.len(), registry.properties.len());
        for (id, props) in registry.iter() {
            let record = registry.resolved(id);
            assert_eq!(record.render_type, props.render_type, "{id}");
            assert_eq!(record.flags, props.flags, "{id}");
        }
    }

    #[test]
    fn texture_ids_are_deterministic_for_a_given_registration_order() {
        let build = || {
            let mut builder = BlockRegistryBuilder::new();
            builder.register_kind(BlockKind::Stone).unwrap();
            builder.register_kind(BlockKind::Grass).unwrap();
            builder.freeze().unwrap()
        };

        let first = build();
        let second = build();

        assert_eq!(first.textures().len(), second.textures().len());
        for (id, _) in first.iter() {
            assert_eq!(
                first.resolved(id).textures,
                second.resolved(id).textures,
                "texture ids must not drift between registries",
            );
        }
    }

    #[test]
    fn the_texture_table_maps_ids_back_to_keys() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let key = NamespacedKey::builtin("block/stone");
        let table = registry.textures();

        let id = table.id(&key).expect("stone's texture is in the table");
        assert_eq!(table.get(id), Some(&key));
    }

    #[test]
    fn the_hot_record_stays_eight_bytes() {
        // Canary for "hot data minimized": this is what a per-voxel or per-hit
        // loop touches, so eight of them must share one 64-byte cache line.
        assert_eq!(size_of::<HotRecord>(), 8);
        assert_eq!(64 / size_of::<HotRecord>(), 8);
        // The assembled view is a convenience, never stored; it is allowed to
        // be larger, but should stay well below the authoring shape.
        assert!(size_of::<BlockRecord>() * 4 <= size_of::<BlockProperties>());
    }

    #[test]
    fn hot_records_mirror_the_authoring_properties_for_every_builtin() {
        let mut builder = BlockRegistryBuilder::new();
        for kind in BlockKind::ALL {
            if kind != BlockKind::Air {
                builder.register_kind(kind).unwrap();
            }
        }
        let registry = builder.freeze().unwrap();

        for (id, props) in registry.iter() {
            let hot = registry.hot(id);
            assert_eq!(hot.render_type(), props.render_type, "{id}");
            assert_eq!(hot.flags(), props.flags & BlockFlags::HOT_MASK, "{id}");
            assert_eq!(hot.light_emission(), props.light_emission, "{id}");
            assert_eq!(hot.light_opacity(), props.light_opacity, "{id}");
            assert_eq!(
                hot.texture_set().is_some(),
                props.textures.is_some(),
                "{id}"
            );

            // The assembled view agrees with the pieces.
            let whole = registry.resolved(id);
            assert_eq!(whole.render_type, hot.render_type(), "{id}");
            assert_eq!(whole.flags, props.flags, "{id}");
            assert_eq!(whole.textures, registry.face_textures(id), "{id}");
            assert_eq!(whole.template_root, props.template_root, "{id}");
        }
    }

    #[test]
    fn blocks_with_the_same_six_textures_share_one_texture_set() {
        let mut builder = BlockRegistryBuilder::new();
        let stone = builder.register_kind(BlockKind::Stone).unwrap();
        let copy = builder
            .register(
                NamespacedKey::builtin("stone_copy"),
                BlockProperties::new()
                    .with_render_type(RenderType::Solid)
                    .with_textures(FaceTextures::all(NamespacedKey::builtin("block/stone"))),
            )
            .unwrap();
        let dirt = builder.register_kind(BlockKind::Dirt).unwrap();
        let registry = builder.freeze().unwrap();

        assert_eq!(
            registry.hot(stone).texture_set(),
            registry.hot(copy).texture_set()
        );
        assert_ne!(
            registry.hot(stone).texture_set(),
            registry.hot(dirt).texture_set()
        );
        assert_eq!(
            registry.texture_sets().len(),
            2,
            "stone/stone_copy share a set; dirt has its own"
        );
        assert_eq!(registry.face_textures(stone), registry.face_textures(copy));
    }

    #[test]
    fn many_blocks_over_few_sets_keeps_the_set_table_small() {
        let mut builder = BlockRegistryBuilder::new();
        for i in 0..5_000u32 {
            // 5 000 blocks, 8 distinct textures (so at most 8 distinct sets).
            let tex = NamespacedKey::builtin(&format!("block/t{}", i % 8));
            builder
                .register(
                    NamespacedKey::builtin(&format!("b{i}")),
                    BlockProperties::new()
                        .with_render_type(RenderType::Solid)
                        .with_textures(FaceTextures::all(tex)),
                )
                .unwrap();
        }
        let registry = builder.freeze().unwrap();
        assert_eq!(registry.len(), 5_001);
        assert_eq!(registry.texture_sets().len(), 8);
        assert_eq!(registry.hot_table().len(), 5_001);
    }

    #[test]
    fn template_roots_are_sparse_and_cold() {
        let mut builder = BlockRegistryBuilder::new();
        let flat = builder.register_kind(BlockKind::Stone).unwrap();
        let stairs = builder
            .register(
                NamespacedKey::builtin("stairs"),
                BlockProperties::new().with_template_root(0),
            )
            .unwrap();
        let registry = builder.freeze().unwrap();

        // Root 0 is a legal root and must stay distinguishable from "none".
        assert_eq!(registry.template_root(stairs), Some(0));
        assert_eq!(registry.template_root(flat), None);
        assert_eq!(registry.template_root(AIR_ID), None);
        assert_eq!(registry.templates.len(), 1, "only one block has a template");
    }

    #[test]
    fn flag_bits_above_the_hot_mask_are_cold_only() {
        let cold = BlockFlags::from_bits(1 << 20);
        let mut builder = BlockRegistryBuilder::new();
        let id = builder
            .register(
                NamespacedKey::builtin("odd"),
                BlockProperties::new().with_flags(BlockFlags::SOLID | cold),
            )
            .unwrap();
        let registry = builder.freeze().unwrap();

        assert!(registry.hot(id).flags().contains(BlockFlags::SOLID));
        assert!(!registry.hot(id).flags().contains(cold));
        assert!(
            registry.resolved(id).flags.contains(cold),
            "still in the full view"
        );
        assert!(registry.get(id).unwrap().flags.contains(cold));
    }

    #[test]
    fn gpu_words_match_the_typed_tables() {
        let mut builder = BlockRegistryBuilder::new();
        builder.register_kind(BlockKind::Grass).unwrap();
        builder.register_kind(BlockKind::Stone).unwrap();
        let registry = builder.freeze().unwrap();

        let hot = registry.gpu_hot_words();
        assert_eq!(hot.len(), registry.len() * 2);
        for (id, _) in registry.iter() {
            assert_eq!(
                &hot[id.index() * 2..id.index() * 2 + 2],
                registry.hot(id).to_words()
            );
        }

        let sets = registry.gpu_texture_set_words();
        assert_eq!(sets.len(), registry.texture_sets().len() * 3);

        // Decode a face the way the WGSL in `crate::hot` does.
        let grass = registry.id_of("voxel:grass_block").unwrap();
        let set = registry.hot(grass).texture_set().unwrap().index();
        for face in Face::ALL {
            let f = face.index();
            let word = sets[3 * set + f / 2];
            let tex = (word >> (16 * (f & 1))) & 0xFFFF;
            assert_eq!(
                tex,
                registry.face_textures(grass).unwrap().get(face).as_raw() as u32
            );
        }
    }

    #[test]
    #[should_panic(expected = "outside the registry")]
    fn hot_lookup_of_an_unregistered_id_panics() {
        let registry = BlockRegistryBuilder::new().freeze().unwrap();
        let _ = registry.hot(MaterialId::from_raw(99));
    }
}
