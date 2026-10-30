//! Cached per-world query plans.

use core::any::TypeId;

use crate::component::ComponentId;
use crate::prelude::*;
use crate::query::QuerySpec;
use crate::world::World;

/// The access set of a query: which components it reads and writes.
///
/// `read` includes sparse `With`/`Without` filters, because those filters read
/// the sparse array. Table filters are resolved at plan-build time and acquire
/// no borrow.
pub(crate) struct AccessSet {
    pub read: SmallVec<[ComponentId; 8]>,
    pub write: SmallVec<[ComponentId; 8]>,
}

impl AccessSet {
    fn from_desc(desc: &QueryDesc) -> Self {
        let mut read = desc.read.clone();
        read.sort_unstable();
        read.dedup();
        let mut write = desc.write.clone();
        write.sort_unstable();
        write.dedup();
        Self { read, write }
    }

    pub(crate) fn allows_read(&self, id: ComponentId) -> bool {
        self.read.binary_search(&id).is_ok() || self.write.binary_search(&id).is_ok()
    }

    pub(crate) fn allows_write(&self, id: ComponentId) -> bool {
        self.write.binary_search(&id).is_ok()
    }
}

/// A normalized description of a query type, produced by
/// [`QuerySpec::describe`].
///
/// This type is an implementation detail of the sealed query traits and is not
/// reachable through the public API.
pub(crate) struct QueryDesc {
    pub read: SmallVec<[ComponentId; 8]>,
    pub write: SmallVec<[ComponentId; 8]>,
    pub terms: SmallVec<[ComponentId; 16]>,
    pub required_table: SmallVec<[ComponentId; 8]>,
    pub excluded_table: SmallVec<[ComponentId; 8]>,
    pub required_sparse: SmallVec<[ComponentId; 8]>,
    pub excluded_sparse: SmallVec<[ComponentId; 8]>,
}

impl QueryDesc {
    pub(crate) fn new() -> Self {
        Self {
            read: SmallVec::new(),
            write: SmallVec::new(),
            terms: SmallVec::new(),
            required_table: SmallVec::new(),
            excluded_table: SmallVec::new(),
            required_sparse: SmallVec::new(),
            excluded_sparse: SmallVec::new(),
        }
    }
}

/// How a query plan chooses the outer iteration order.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AnchorMode {
    /// Iterate archetypes, then rows.
    TableArchetypes,
    /// Iterate the entities of the smallest required sparse set.
    SmallestSparseSet(ComponentId),
    /// Scan every allocator slot.
    AllLiveEntities,
}

/// One archetype matched by a plan.
#[allow(dead_code)]
pub(crate) struct ArchetypeMatch {
    pub archetype: u32,
    pub table: u32,
    /// Precomputed table column indices, aligned with the archetype's sorted
    /// component list.
    pub table_columns: SmallVec<[u16; 4]>,
    /// Sparse components checked per row by this match.
    pub sparse_components: SmallVec<[ComponentId; 8]>,
    pub with_sparse: SmallVec<[ComponentId; 8]>,
    pub without_sparse: SmallVec<[ComponentId; 8]>,
    pub skip: bool,
}

/// A fully resolved query plan.
pub(crate) struct QueryPlan {
    pub anchor: AnchorMode,
    pub matches: SmallVec<[ArchetypeMatch; 4]>,
    pub access: AccessSet,
}

/// Plan cache keyed by `TypeId::<Q>`.
pub(crate) struct PlanCache {
    map: FxHashMap<TypeId, Option<Box<QueryPlan>>>,
}

impl PlanCache {
    pub(crate) fn new() -> Self {
        Self {
            map: FxHashMap::default(),
        }
    }
}

/// Validates query-level invariants that must be checked on every query
/// construction, even when a plan is cached.
pub(crate) fn validate_desc(desc: &QueryDesc) {
    let mut terms = desc.terms.clone();
    terms.sort_unstable();
    for pair in terms.windows(2) {
        if pair[0] == pair[1] {
            panic!(
                "ecs: duplicate component in query: {:?} appears more than once",
                pair[0]
            );
        }
    }

    let mut read = desc.read.clone();
    read.sort_unstable();
    read.dedup();
    for id in &read {
        if desc.write.contains(id) {
            panic!("ecs: borrow conflict: {id:?} is requested for both reading and writing");
        }
    }
}

fn sorted_copy(ids: &[ComponentId]) -> SmallVec<[ComponentId; 8]> {
    let mut out: SmallVec<[ComponentId; 8]> = ids.iter().copied().collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub(crate) fn build_plan(world: &World, desc: &QueryDesc) -> QueryPlan {
    let access = AccessSet::from_desc(desc);
    let required_table = sorted_copy(&desc.required_table);
    let excluded_table = sorted_copy(&desc.excluded_table);
    let required_sparse = sorted_copy(&desc.required_sparse);
    let excluded_sparse = sorted_copy(&desc.excluded_sparse);

    let mut matches: SmallVec<[ArchetypeMatch; 4]> = SmallVec::new();
    for (index, archetype) in world.archetypes.iter().enumerate() {
        let components = archetype.components();
        let mut skip = false;
        for required in &required_table {
            if components.binary_search(required).is_err() {
                skip = true;
                break;
            }
        }
        if !skip {
            for excluded in &excluded_table {
                if components.binary_search(excluded).is_ok() {
                    skip = true;
                    break;
                }
            }
        }
        let mut table_columns: SmallVec<[u16; 4]> = SmallVec::new();
        for &component in components {
            let column = world.tables[index]
                .column_index(component)
                .expect("ecs: flush invariant: archetype component missing from table");
            table_columns.push(column as u16);
        }
        matches.push(ArchetypeMatch {
            archetype: index as u32,
            table: index as u32,
            table_columns,
            sparse_components: required_sparse.clone(),
            with_sparse: required_sparse.clone(),
            without_sparse: excluded_sparse.clone(),
            skip,
        });
    }

    let anchor = if !required_table.is_empty() {
        AnchorMode::TableArchetypes
    } else if !required_sparse.is_empty() {
        let mut smallest = required_sparse[0];
        let mut smallest_len = world.sparse_sets[smallest.0 as usize]
            .as_ref()
            .map_or(usize::MAX, |set| set.len());
        for &candidate in &required_sparse[1..] {
            let len = world.sparse_sets[candidate.0 as usize]
                .as_ref()
                .map_or(usize::MAX, |set| set.len());
            if len < smallest_len {
                smallest = candidate;
                smallest_len = len;
            }
        }
        AnchorMode::SmallestSparseSet(smallest)
    } else {
        AnchorMode::AllLiveEntities
    };

    QueryPlan {
        anchor,
        matches,
        access,
    }
}

impl World {
    /// Returns the cached plan for `Q`, building it if necessary.
    ///
    /// # Panics
    ///
    /// Panics with `"ecs: flush invariant:"` if the cache is internally
    /// inconsistent. This is a bug, not user error.
    pub(crate) fn plan_for<Q: QuerySpec>(&self, desc: &QueryDesc) -> &QueryPlan {
        let key = TypeId::of::<Q>();
        // SAFETY: `World: !Sync`, so no other thread can touch the cache. A
        // `Query` guard keeps `&World` alive; plans are only invalidated by
        // `&mut World` APIs, which cannot run while a guard exists. Inserting a
        // new boxed plan never moves the contents of existing boxes, so
        // references handed out earlier stay valid.
        let cache = self.plans.get();
        // SAFETY: shared access to the plan cache; no other reference to the
        // cache is live at this point.
        let present = unsafe { (*cache).map.get(&key).is_some_and(|entry| entry.is_some()) };
        if !present {
            let plan = Box::new(build_plan(self, desc));
            // SAFETY: as above; existing plans live in separate heap boxes.
            unsafe {
                (*cache).map.insert(key, Some(plan));
            }
        }
        // SAFETY: an entry was either present or just inserted.
        let entry = unsafe {
            (*cache)
                .map
                .get(&key)
                .expect("ecs: flush invariant: query plan missing after insertion")
        };
        entry
            .as_ref()
            .expect("ecs: flush invariant: query plan is stale")
    }

    /// Marks every cached plan stale. Must only be called through `&mut
    /// World` (directly or transitively).
    pub(crate) fn invalidate_plans(&self) {
        // SAFETY: see `plan_for`; invalidation happens only while the world is
        // exclusively borrowed, so no plan references are alive.
        let cache = self.plans.get();
        // SAFETY: exclusive logical access to the cache (`&mut World` above).
        unsafe {
            for entry in (*cache).map.values_mut() {
                *entry = None;
            }
        }
    }
}
