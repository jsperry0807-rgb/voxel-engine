//! The archetype graph.
//!
//! In v1 an archetype is identified one-to-one with a `TableId` (the sorted
//! set of table-stored component ids). Sparse components never affect
//! archetype identity (spec §3.2, §9.1).
//!
//! Add/remove edges are cached so that repeated migrations between the same
//! component sets do not re-hash the component list.

use crate::component::ComponentId;
use crate::prelude::*;
use crate::table::Table;
use crate::world::World;

pub(crate) struct Archetype {
    pub(crate) components: SmallVec<[ComponentId; 8]>,
    pub(crate) add_edges: FxHashMap<ComponentId, u32>,
    pub(crate) remove_edges: FxHashMap<ComponentId, u32>,
}

impl Archetype {
    pub(crate) fn new(components: SmallVec<[ComponentId; 8]>) -> Self {
        Self {
            components,
            add_edges: FxHashMap::default(),
            remove_edges: FxHashMap::default(),
        }
    }

    /// Sorted, duplicate-free table component ids.
    pub(crate) fn components(&self) -> &[ComponentId] {
        &self.components
    }
}

impl World {
    /// Returns the archetype id for `ids`, creating the table and archetype if
    /// it does not exist yet.
    ///
    /// `ids` must contain only table-stored components. They may be unsorted.
    pub(crate) fn ensure_archetype(&mut self, mut ids: SmallVec<[ComponentId; 8]>) -> u32 {
        ids.sort_unstable();
        ids.dedup();
        if let Some(&id) = self.archetype_index.get(&ids) {
            return id;
        }
        let id = self.archetypes.len() as u32;
        let table = Table::new(&ids, &self.registry);
        self.tables.push(table);
        self.archetypes.push(Archetype::new(ids.clone()));
        self.archetype_index.insert(ids, id);
        // A new archetype means existing query plans are incomplete.
        self.invalidate_plans();
        id
    }

    /// Follows (or creates) the add edge for `cid`.
    pub(crate) fn archetype_add(&mut self, archetype: u32, cid: ComponentId) -> u32 {
        if let Some(&target) = self.archetypes[archetype as usize].add_edges.get(&cid) {
            return target;
        }
        let mut ids = self.archetypes[archetype as usize].components.clone();
        ids.push(cid);
        ids.sort_unstable();
        let target = self.ensure_archetype(ids);
        self.archetypes[archetype as usize]
            .add_edges
            .insert(cid, target);
        target
    }

    /// Follows (or creates) the remove edge for `cid`.
    pub(crate) fn archetype_remove(&mut self, archetype: u32, cid: ComponentId) -> u32 {
        if let Some(&target) = self.archetypes[archetype as usize].remove_edges.get(&cid) {
            return target;
        }
        let mut ids = self.archetypes[archetype as usize].components.clone();
        if let Some(position) = ids.iter().position(|existing| *existing == cid) {
            ids.remove(position);
        }
        let target = self.ensure_archetype(ids);
        self.archetypes[archetype as usize]
            .remove_edges
            .insert(cid, target);
        target
    }
}
