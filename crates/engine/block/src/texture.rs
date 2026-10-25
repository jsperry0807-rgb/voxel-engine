//! Resolved texture identity.
//!
//! Authoring code names a texture by its [`NamespacedKey`]
//! ("voxel:block/stone"). The registry resolves those names exactly once,
//! at startup, into dense [`TextureId`]s that index a per-registry
//! [`TextureTable`]. The resolved id is what per-voxel code carries: two
//! bytes and no pointer chase, where the key is a 24-byte `Arc<str>` handle.
//!
//! A resolved id is also the *slice index* of the texture in the renderer's
//! GPU `Texture2DArray`, not an offset into a flat atlas. Every texture is a
//! 256x256 array slice, so sampling, wrapping and mip generation stay inside
//! one slice and cannot bleed into a neighboring texture.
//!
//! Resolution runs one way. The table owns the mapping, the texture-array
//! builder (render crate) walks it to create GPU resources, and nothing
//! downstream of the resolution boundary needs the key again.
//!
//! Texture ids are session-local, exactly like material ids: they are handed out
//! in first-use order, so two sessions that register the same blocks in a
//! different order number their textures differently. They are therefore
//! *derived*, never persisted — a file names blocks by key, and the registry
//! resolves those blocks' textures when the file is loaded. See
//! `docs/save-format.md`.

use core::fmt;
use std::collections::HashMap;

use crate::id::NamespacedKey;
use voxel_core::Face;

/// Dense index of a texture within a [`TextureTable`], used as the slice index
/// of a 256x256 layer in the renderer's GPU `Texture2DArray`.
///
/// This is a slice index, not a coordinate in a flat atlas: the shader samples
/// `texture_2d_array[texture_id]`, so neighboring textures cannot bleed into
/// each other at their edges.
///
/// Session-local, like [`crate::MaterialId`]: ids are handed out in first-use
/// order, so they are only meaningful against the table that produced them.
/// Saves and network payloads carry [`NamespacedKey`]s, not ids.
///
/// `#[repr(transparent)]` keeps the layout identical to `u16`, so a resolved
/// face set can be reinterpreted without copying.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TextureId(u16);

impl TextureId {
    /// Builds an id from its raw representation.
    ///
    /// Prefer the id handed out by the [`TextureTable`]. This exists for
    /// reading back ids stored within the same session and for tests.
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// Raw representation.
    pub const fn as_raw(self) -> u16 {
        self.0
    }

    /// Index into the table's key vector.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for TextureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TextureId({})", self.0)
    }
}

impl fmt::Debug for TextureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The six face slots of a block, resolved to texture ids.
///
/// Mirrors [`crate::properties::FaceTextures`] slot for slot, except every
/// slot is a [`TextureId`] rather than a key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FaceTextureIds([TextureId; 6]);

impl FaceTextureIds {
    /// Builds a resolved face set from six ids, in [`Face::ALL`] order.
    pub(crate) fn new(ids: [TextureId; 6]) -> Self {
        Self(ids)
    }

    /// The texture for a given face.
    pub const fn get(&self, face: Face) -> TextureId {
        self.0[face.index()]
    }
}

/// The resolved texture names of one registry.
///
/// Built during the registry's one-time resolution pass: every key a block
/// refers to appears exactly once, and the table is dense — `TextureId(i)`
/// indexes `keys[i]`.
///
/// A texture id is the slice index into the GPU `Texture2DArray`, so every key
/// here corresponds to one 256x256 layer.
///
/// A table holds at most 65 536 distinct textures, because ids are `u16`.
/// That is the practical ceiling of the two registry caps (see the crate's
/// *Limits* section): six distinct faces per block exhaust it at about the
/// 10 923rd block, well inside the 65 536 blocks the id space allows.
#[derive(Debug, Default)]
pub struct TextureTable {
    /// `keys[i]` is the key behind `TextureId(i)`.
    keys: Vec<NamespacedKey>,
    /// Reverse lookup, so a key resolves without a linear scan.
    ids: HashMap<NamespacedKey, TextureId>,
}

impl TextureTable {
    /// The key behind an id.
    pub fn get(&self, id: TextureId) -> Option<&NamespacedKey> {
        self.keys.get(id.index())
    }

    /// The id of a key, if the table holds it.
    pub fn id(&self, key: &NamespacedKey) -> Option<TextureId> {
        self.ids.get(key).copied()
    }

    /// Number of distinct textures in the table.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The table's keys, in id order.
    pub fn keys(&self) -> &[NamespacedKey] {
        &self.keys
    }

    /// Iterates the table as `(id, key)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (TextureId, &NamespacedKey)> {
        self.keys
            .iter()
            .enumerate()
            .map(|(index, key)| (TextureId::from_raw(index as u16), key))
    }

    /// Adds a key to the table, returning its id.
    ///
    /// A key already in the table keeps the id it had, so a texture shared by
    /// many blocks costs one table entry rather than one per reference.
    ///
    /// Returns `None` when the table is full: a table holds at most 65 536
    /// distinct textures, because ids are `u16`. The caller turns that into
    /// a reported failure rather than a silently wrapped id that would
    /// render the wrong texture.
    pub(crate) fn insert(&mut self, key: &NamespacedKey) -> Option<TextureId> {
        if let Some(id) = self.ids.get(key) {
            return Some(*id);
        }
        const CAPACITY: usize = u16::MAX as usize + 1;
        if self.keys.len() >= CAPACITY {
            return None;
        }
        let id = TextureId::from_raw(self.keys.len() as u16);
        self.keys.push(key.clone());
        self.ids.insert(key.clone(), id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_id_is_layout_identical_to_u16() {
        assert_eq!(size_of::<TextureId>(), size_of::<u16>());
        assert_eq!(align_of::<TextureId>(), align_of::<u16>());
    }

    #[test]
    fn table_round_trips_key_to_id() {
        let mut table = TextureTable::default();
        let key = NamespacedKey::builtin("block/stone");

        let id = table.insert(&key).expect("the table has room");

        assert_eq!(table.get(id), Some(&key));
        assert_eq!(table.id(&key), Some(id));
        assert_eq!(table.len(), 1);
        assert!(!table.is_empty());
    }

    #[test]
    fn inserting_a_known_key_reuses_its_id() {
        let mut table = TextureTable::default();
        let key = NamespacedKey::builtin("block/stone");

        let first = table.insert(&key).expect("the table has room");
        let second = table.insert(&key).expect("the table has room");

        assert_eq!(first, second);
        assert_eq!(table.len(), 1, "a repeated key must not grow the table");
    }

    #[test]
    fn distinct_keys_get_distinct_ids() {
        let mut table = TextureTable::default();
        let stone = NamespacedKey::builtin("block/stone");
        let dirt = NamespacedKey::builtin("block/dirt");

        let a = table.insert(&stone).expect("the table has room");
        let b = table.insert(&dirt).expect("the table has room");

        assert_ne!(a, b);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(a), Some(&stone));
        assert_eq!(table.get(b), Some(&dirt));
    }

    #[test]
    fn empty_table_answers_nothing() {
        let table = TextureTable::default();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
        assert_eq!(table.get(TextureId::from_raw(0)), None);
        assert_eq!(table.id(&NamespacedKey::builtin("block/stone")), None);
    }

    #[test]
    fn face_texture_ids_answer_per_face() {
        let ids = [
            TextureId::from_raw(0),
            TextureId::from_raw(1),
            TextureId::from_raw(2),
            TextureId::from_raw(3),
            TextureId::from_raw(4),
            TextureId::from_raw(5),
        ];
        let faces = FaceTextureIds::new(ids);

        for (index, face) in Face::ALL.iter().enumerate() {
            assert_eq!(faces.get(*face), ids[index]);
        }
    }
}
