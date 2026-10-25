//! Named sets of blocks for gameplay queries, identified by [`NamespacedKey`]s.
//!
//! Tags answer questions like "can this block be mined with a pickaxe?" or
//! "does this block burn?" without hardcoding id lists into systems. Tag names
//! are namespaced keys — "`mymod:mineable/pickaxe`" — so mods and data packs
//! can extend them without colliding with each other's tags, and a typo
//! ("`MyMod:pickaxe`") is rejected when the key is built rather than silently
//! spawning a second tag no query will ever match.

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};

use crate::id::{BlockId, NamespacedKey};

/// A collection of named [`BlockId`] sets, with a reverse index.
///
/// Uses sorted sets internally, so iteration order is deterministic and
/// tests don't depend on hashing.
#[derive(Debug, Default, Clone)]
pub struct BlockTags {
    /// tag key -> member ids
    tags: HashMap<NamespacedKey, BTreeSet<BlockId>>,
    /// member id -> tag keys, maintained in lockstep with `tags`
    by_block: HashMap<BlockId, BTreeSet<NamespacedKey>>,
}

impl BlockTags {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tag set with both maps pre-allocated for the expected load.
    ///
    /// `distinct_tags` is the number of tag keys (the `tags` map);
    /// `tagged_blocks` is the number of distinct blocks that will carry at
    /// least one tag (the `by_block` map). They usually differ, so they are
    /// sized separately rather than guessing one for both.
    ///
    /// Reserve when loading tags from data files, where both counts are known
    /// before the first [`Self::insert`].
    pub fn with_capacity(distinct_tags: usize, tagged_blocks: usize) -> Self {
        Self {
            tags: HashMap::with_capacity(distinct_tags),
            by_block: HashMap::with_capacity(tagged_blocks),
        }
    }

    /// Adds `id` to `tag`. The tag is created if it does not exist yet.
    ///
    /// Returns `true` if the membership is new.
    pub fn insert(&mut self, tag: &NamespacedKey, id: BlockId) -> bool {
        let new = self.tags.entry(tag.clone()).or_default().insert(id);
        if new {
            self.by_block.entry(id).or_default().insert(tag.clone());
        }
        new
    }

    /// Adds every id in `ids` to `tag`, returning how many memberships were
    /// new.
    ///
    /// The bulk counterpart to [`Self::insert`], for the load path: resolve a
    /// tag's member keys once (see
    /// [`crate::BlockRegistry::resolve_tag_members`]) and hand the ids over
    /// in one call.
    pub fn insert_all(
        &mut self,
        tag: &NamespacedKey,
        ids: impl IntoIterator<Item = BlockId>,
    ) -> usize {
        let mut added = 0;
        for id in ids {
            let new = self.tags.entry(tag.clone()).or_default().insert(id);
            if new {
                self.by_block.entry(id).or_default().insert(tag.clone());
                added += 1;
            }
        }
        added
    }

    /// Removes `id` from `tag`, returning whether it was a member.
    ///
    /// Each map is probed once via `entry` rather than the old `get_mut` +
    /// `remove` pair, so a removal hashes the tag (and the id) a single time
    /// instead of twice per map.
    pub fn remove(&mut self, tag: &NamespacedKey, id: BlockId) -> bool {
        // Forward index: one probe; drop the tag entirely if it empties.
        let Entry::Occupied(mut tag_slot) = self.tags.entry(tag.clone()) else {
            return false;
        };
        if !tag_slot.get_mut().remove(&id) {
            return false;
        }
        if tag_slot.get().is_empty() {
            tag_slot.remove();
        }

        // Reverse index: same single-probe pattern for the id's tag set.
        if let Entry::Occupied(mut id_slot) = self.by_block.entry(id) {
            id_slot.get_mut().remove(tag);
            if id_slot.get().is_empty() {
                id_slot.remove();
            }
        }
        true
    }

    /// True if `id` is a member of `tag`.
    pub fn contains(&self, tag: &NamespacedKey, id: BlockId) -> bool {
        self.tags
            .get(tag)
            .is_some_and(|members| members.contains(&id))
    }

    /// All ids in `tag`, in ascending id order.
    pub fn ids(&self, tag: &NamespacedKey) -> impl Iterator<Item = BlockId> + '_ {
        self.tags.get(tag).into_iter().flatten().copied()
    }

    /// All tags `id` belongs to, in lexicographic key order.
    pub fn tags_of(&self, id: BlockId) -> impl Iterator<Item = &NamespacedKey> + '_ {
        self.by_block.get(&id).into_iter().flatten()
    }

    /// Every tag key that has at least one member, in lexicographic order.
    pub fn names(&self) -> impl Iterator<Item = &NamespacedKey> + '_ {
        self.tags.keys()
    }

    /// Number of distinct tags.
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Number of (tag, block) memberships.
    pub fn membership_count(&self) -> usize {
        self.tags.values().map(BTreeSet::len).sum()
    }

    /// Drops every tag and membership.
    pub fn clear(&mut self) {
        self.tags.clear();
        self.by_block.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: BlockId = BlockId::from_raw(1);
    const DIRT: BlockId = BlockId::from_raw(2);

    /// Tags are parsed once and passed by reference, mirroring how a data
    /// loader would hold a table of pre-validated keys.
    fn key(raw: &str) -> NamespacedKey {
        NamespacedKey::parse(raw).expect("test tag names must be valid keys")
    }

    fn pickaxe() -> NamespacedKey {
        key("mymod:mineable/pickaxe")
    }

    fn shovel() -> NamespacedKey {
        key("mymod:mineable/shovel")
    }

    #[test]
    fn with_capacity_behaves_like_new() {
        // Reserving must not change observable behaviour: same insertions,
        // same membership, same iteration order.
        let mut reserved = BlockTags::with_capacity(4, 8);
        let mut plain = BlockTags::new();

        reserved.insert(&pickaxe(), STONE);
        reserved.insert(&shovel(), STONE);
        plain.insert(&pickaxe(), STONE);
        plain.insert(&shovel(), STONE);

        assert_eq!(reserved.membership_count(), plain.membership_count());
        assert_eq!(reserved.len(), plain.len());
        assert!(reserved.contains(&pickaxe(), STONE));
        assert_eq!(
            reserved.tags_of(STONE).cloned().collect::<Vec<_>>(),
            vec![pickaxe(), shovel()]
        );
    }

    #[test]
    fn with_capacity_zero_is_valid() {
        let mut tags = BlockTags::with_capacity(0, 0);
        assert!(tags.insert(&pickaxe(), STONE));
        assert!(tags.contains(&pickaxe(), STONE));
    }

    #[test]
    fn insert_and_query_membership() {
        let mut tags = BlockTags::new();
        assert!(tags.insert(&pickaxe(), STONE));
        assert!(tags.insert(&shovel(), DIRT));

        assert!(tags.contains(&pickaxe(), STONE));
        assert!(!tags.contains(&pickaxe(), DIRT));
        assert_eq!(tags.ids(&pickaxe()).collect::<Vec<_>>(), vec![STONE]);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags.membership_count(), 2);
    }

    #[test]
    fn tag_names_must_be_valid_namespaced_keys() {
        // The validation lives in `NamespacedKey`, so an unnamespaced or
        // malformed tag cannot even be constructed to be inserted.
        assert!(NamespacedKey::parse("pickaxe").is_err());
        assert!(NamespacedKey::parse("MyMod:mineable/pickaxe").is_err());
        assert!(NamespacedKey::parse("mymod:Mineable/Pickaxe").is_err());

        // A namespaced tag with a directory-style path is fine.
        assert!(NamespacedKey::parse("mymod:mineable/pickaxe").is_ok());
    }

    #[test]
    fn duplicate_inserts_are_idempotent() {
        let mut tags = BlockTags::new();
        assert!(tags.insert(&pickaxe(), STONE));
        assert!(!tags.insert(&pickaxe(), STONE));
        assert_eq!(tags.membership_count(), 1);
    }

    #[test]
    fn reverse_index_lists_tags_of_a_block() {
        let mut tags = BlockTags::new();
        tags.insert(&pickaxe(), STONE);
        tags.insert(&shovel(), STONE);
        tags.insert(&shovel(), DIRT);

        assert_eq!(
            tags.tags_of(STONE).cloned().collect::<Vec<_>>(),
            vec![pickaxe(), shovel()]
        );
        assert_eq!(
            tags.tags_of(DIRT).cloned().collect::<Vec<_>>(),
            vec![shovel()]
        );
    }

    #[test]
    fn remove_cleans_up_empty_tags() {
        let mut tags = BlockTags::new();
        tags.insert(&pickaxe(), STONE);
        assert!(tags.remove(&pickaxe(), STONE));
        assert!(!tags.remove(&pickaxe(), STONE));
        assert!(!tags.contains(&pickaxe(), STONE));
        assert!(tags.tags_of(STONE).next().is_none());
        assert!(tags.is_empty());
    }

    #[test]
    fn ids_iterate_in_ascending_order() {
        let mut tags = BlockTags::new();
        tags.insert(&pickaxe(), DIRT);
        tags.insert(&pickaxe(), STONE);
        assert_eq!(tags.ids(&pickaxe()).collect::<Vec<_>>(), vec![STONE, DIRT]);
    }
}
