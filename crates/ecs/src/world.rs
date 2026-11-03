//! The [`World`]: allocator, registry, archetypes, tables, sparse sets, plans.

use core::any::Any;
use core::ptr;
use std::cell::UnsafeCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::archetype::Archetype;
use crate::borrow::BorrowMap;
use crate::component::{Bundle, ComponentId, Storage};
use crate::entity::{Entity, SlotState};
use crate::location::EntityLocation;
use crate::prelude::*;
use crate::query::plan::{AccessSet, PlanCache};
use crate::query::{Query, QueryMut, QuerySpec, ReadOnlyQuery};
use crate::sparse::{SparseSet, SparseSetErased};
use crate::table::Table;
use crate::views::{EntityMut, EntityRef};

static NEXT_WORLD_ID: AtomicU64 = AtomicU64::new(1);

fn next_world_id() -> u64 {
    NEXT_WORLD_ID.fetch_add(1, Ordering::Relaxed)
}

/// Owns all entities and component storage.
///
/// # Threading
///
/// A `World` is `Send` but not `Sync`: it can be moved between threads, but it
/// cannot be shared. All component types must be registered with
/// [`World::register`] before use.
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<voxel_ecs::World>();
/// ```
///
/// # Panics
///
/// Misuse is reported with the stable `"ecs: "` prefixes documented in
/// `docs/SPEC.md` §19.2 and in each method's documentation.
pub struct World {
    pub(crate) id: u64,
    pub(crate) clear_epoch: u64,
    pub(crate) allocator: crate::entity::EntityAllocator,
    pub(crate) registry: crate::component::Registry,
    pub(crate) archetypes: Vec<Archetype>,
    pub(crate) tables: Vec<Table>,
    pub(crate) sparse_sets: Vec<Option<SparseSetErased>>,
    pub(crate) locations: Vec<EntityLocation>,
    pub(crate) archetype_index: HashMap<SmallVec<[ComponentId; 8]>, u32>,
    pub(crate) borrows: UnsafeCell<BorrowMap>,
    pub(crate) plans: UnsafeCell<PlanCache>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        let registry = crate::component::Registry::new();
        let empty: SmallVec<[ComponentId; 8]> = SmallVec::new();
        let mut archetype_index = HashMap::new();
        archetype_index.insert(empty.clone(), 0);
        let empty_table = Table::new(&[], &registry);
        Self {
            id: next_world_id(),
            clear_epoch: 0,
            allocator: crate::entity::EntityAllocator::new(),
            registry,
            archetypes: vec![Archetype::new(empty)],
            tables: vec![empty_table],
            sparse_sets: vec![None],
            locations: Vec::new(),
            archetype_index,
            borrows: UnsafeCell::new(BorrowMap::new()),
            plans: UnsafeCell::new(PlanCache::new()),
        }
    }

    // ───────────────────────────── Registration ─────────────────────────────

    /// Registers component type `T` with the requested storage.
    ///
    /// Returns the [`ComponentId`]. Calling this again with the same type and
    /// storage is idempotent and returns the existing id.
    ///
    /// # Panics
    ///
    /// * `"ecs: storage mismatch:"` if `T` was already registered with a
    ///   different [`Storage`].
    /// * `"ecs: component id exhausted:"` if all `u32` component ids are used.
    pub fn register<T: Send + 'static>(&mut self, storage: Storage) -> ComponentId {
        let (id, is_new) = self.registry.register::<T>(storage);
        if is_new {
            while self.sparse_sets.len() < self.registry.len() {
                self.sparse_sets.push(None);
            }
            if storage == Storage::SparseSet {
                self.sparse_sets[id.0 as usize] = Some(SparseSetErased::new::<T>());
            }
            self.invalidate_plans();
        }
        id
    }

    // ───────────────────────────── Lifecycle ─────────────────────────────

    /// Spawns an entity with the components in `bundle`.
    ///
    /// # Panics
    ///
    /// * `"ecs: duplicate component in bundle:"` if a component type appears
    ///   twice.
    /// * `"ecs: component not registered:"` if any component was never
    ///   registered.
    /// * `"ecs: entity index exhausted:"` if the allocator is full.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let ids = B::component_ids(self);
        self.validate_bundle_ids(&ids);
        self.spawn_with_ids(bundle, &ids)
    }

    /// Despawns `e`, dropping its components.
    ///
    /// Returns `false` (and does nothing) if `e` is dead.
    pub fn despawn(&mut self, e: Entity) -> bool {
        self.despawn_inner(e, None)
    }

    /// Inserts `value` into `e`, returning the previous value if present.
    ///
    /// # Panics
    ///
    /// * `"ecs: dead entity:"` if `e` is not alive.
    /// * `"ecs: component not registered:"` if `T` was never registered.
    pub fn insert<T: Send + 'static>(&mut self, e: Entity, value: T) -> Option<T> {
        let id = self.require_component::<T>();
        if !self.is_alive(e) {
            panic!("ecs: dead entity: {e:?} cannot be inserted into");
        }
        match self.registry.storage(id) {
            Storage::SparseSet => self.sparse_typed_mut::<T>(id).insert(e, value),
            Storage::Table => {
                let location = self.locations[e.index() as usize];
                if self.tables[location.archetype as usize].contains(id) {
                    let column = self.tables[location.archetype as usize]
                        .column(id)
                        .expect("ecs: flush invariant: table column disappeared");
                    let ptr = column.ptr_at(location.row as usize).cast::<T>();
                    // SAFETY: the slot holds an initialized T and `&mut World`
                    // guarantees exclusive access.
                    Some(unsafe { ptr::replace(ptr, value) })
                } else {
                    self.migrate_add::<T>(e, value, id, location);
                    None
                }
            }
        }
    }

    /// Removes `T` from `e`, returning it if present.
    ///
    /// # Panics
    ///
    /// * `"ecs: dead entity:"` if `e` is not alive.
    /// * `"ecs: component not registered:"` if `T` was never registered.
    pub fn remove<T: Send + 'static>(&mut self, e: Entity) -> Option<T> {
        let id = self.require_component::<T>();
        if !self.is_alive(e) {
            panic!("ecs: dead entity: {e:?} cannot be removed from");
        }
        match self.registry.storage(id) {
            Storage::SparseSet => self.sparse_typed_mut::<T>(id).remove(e),
            Storage::Table => {
                let location = self.locations[e.index() as usize];
                if self.tables[location.archetype as usize].contains(id) {
                    Some(self.migrate_remove::<T>(e, id, location))
                } else {
                    None
                }
            }
        }
    }

    // ───────────────────────────── Reset ─────────────────────────────

    /// Despawns every entity, empties every table and sparse set, and retains
    /// registrations, archetypes, capacities, and the plan cache (now stale).
    ///
    /// All command buffers bound before this call become stale.
    ///
    /// Destructor panics are caught and discarded so the world remains
    /// structurally valid. The drop order of cleared components is
    /// unspecified.
    pub fn clear(&mut self) {
        self.allocator.clear();
        self.locations.fill(EntityLocation::INVALID);
        for table in &mut self.tables {
            table.clear();
        }
        for set in self.sparse_sets.iter_mut().flatten() {
            set.clear();
        }
        self.invalidate_plans();
        self.clear_epoch = self.clear_epoch.wrapping_add(1);
        #[cfg(debug_assertions)]
        crate::invariants::check_world(self);
    }

    // ───────────────────────────── Safe access ─────────────────────────────

    /// Returns a shared reference to `T` on `e`, if present.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if `T` was never registered.
    /// * `"ecs: borrow conflict:"` if `T` is currently mutably borrowed.
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T> {
        let id = self.require_component::<T>();
        // SAFETY: `World: !Sync`, and `entries` is only accessed through the
        // world; this is a short-lived conflict check.
        unsafe { &*self.borrows.get() }.check_read(id);
        if !self.is_alive(e) {
            return None;
        }
        // SAFETY: the entity is alive; the type is registered; no conflicting
        // borrow is held (checked above).
        unsafe { self.raw_get::<T>(e) }
    }

    /// Returns an exclusive reference to `T` on `e`, if present.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if `T` was never registered.
    /// * `"ecs: borrow conflict:"` if `T` is currently borrowed.
    pub fn get_mut<T: Send + 'static>(&mut self, e: Entity) -> Option<&mut T> {
        let id = self.require_component::<T>();
        // SAFETY: `&mut self` guarantees no other world API is active.
        unsafe { &*self.borrows.get() }.check_write(id);
        if !self.is_alive(e) {
            return None;
        }
        // SAFETY: as above, plus exclusive access to the world.
        unsafe { self.raw_get_mut::<T>(e) }
    }

    /// Unchecked shared access.
    ///
    /// # Panics
    ///
    /// `"ecs: component not registered:"` if `T` was never registered. This
    /// safe check happens before the caller's safety contract applies.
    ///
    /// # Safety
    ///
    /// The caller guarantees all of the following:
    ///
    /// * `e` is [`SlotState::Alive`](crate::World) (allocated and not
    ///   despawned, cleared, or reserved);
    /// * `T` is present on `e`;
    /// * no `&mut T` to the same component on `e` exists simultaneously.
    ///
    /// Violating any of these is undefined behavior.
    pub unsafe fn get_unchecked<T: Send + 'static>(&self, e: Entity) -> &T {
        let _ = self.require_component::<T>();
        // SAFETY: the caller guarantees the entity is alive and has T.
        let ptr = unsafe { self.component_ptr::<T>(e).unwrap_unchecked() };
        // SAFETY: the caller guarantees no conflicting reference exists.
        unsafe { &*ptr }
    }

    /// Unchecked exclusive access.
    ///
    /// # Panics
    ///
    /// `"ecs: component not registered:"` if `T` was never registered. This
    /// safe check happens before the caller's safety contract applies.
    ///
    /// # Safety
    ///
    /// Same as [`Self::get_unchecked`], plus exclusive access to `T` on `e`.
    /// Violating the contract is undefined behavior.
    pub unsafe fn get_unchecked_mut<T: Send + 'static>(&mut self, e: Entity) -> &mut T {
        let _ = self.require_component::<T>();
        // SAFETY: the caller guarantees the entity is alive and has T.
        let ptr = unsafe { self.component_ptr::<T>(e).unwrap_unchecked() };
        // SAFETY: the caller guarantees exclusive access.
        unsafe { &mut *ptr }
    }

    // ───────────────────────────── Entity views ─────────────────────────────

    /// Returns a read-only view borrowing every component of `e`.
    ///
    /// # Panics
    ///
    /// * `"ecs: dead entity:"` if `e` is dead, retired, or reserved.
    /// * `"ecs: borrow conflict:"` if any component of `e` is mutably
    ///   borrowed.
    pub fn entity_ref(&self, e: Entity) -> EntityRef<'_> {
        EntityRef::new(self, e)
    }

    /// Returns an exclusive view borrowing every component of `e`.
    ///
    /// # Panics
    ///
    /// * `"ecs: dead entity:"` if `e` is dead, retired, or reserved.
    /// * `"ecs: borrow conflict:"` if any component of `e` is borrowed.
    pub fn entity_mut(&mut self, e: Entity) -> EntityMut<'_> {
        EntityMut::new(self, e)
    }

    // ───────────────────────────── Queries ─────────────────────────────

    /// Creates a read-only query guard.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if a queried type was never
    ///   registered.
    /// * `"ecs: duplicate component in query:"` if a component appears twice.
    /// * `"ecs: borrow conflict:"` if a required borrow is unavailable.
    pub fn query<Q: ReadOnlyQuery>(&self) -> Query<'_, Q> {
        Query::new(self)
    }

    /// Creates a mutable query guard.
    ///
    /// # Panics
    ///
    /// Same as [`Self::query`].
    pub fn query_mut<Q: QuerySpec>(&mut self) -> QueryMut<'_, Q> {
        QueryMut::new(self)
    }

    // ───────────────────────────── Internal helpers ─────────────────────────────

    pub(crate) fn require_component<T: Send + 'static>(&self) -> ComponentId {
        self.registry.require::<T>()
    }

    pub(crate) fn is_alive(&self, e: Entity) -> bool {
        self.allocator.is_alive(e)
    }

    pub(crate) fn acquire_borrows(&self, access: &AccessSet) {
        // SAFETY: `World: !Sync`; guards are exclusive within the thread.
        let borrows = unsafe { &mut *self.borrows.get() };
        borrows.acquire_many(&access.read, &access.write);
    }

    pub(crate) fn release_borrows(&self, access: &AccessSet) {
        // SAFETY: mirrors `acquire_borrows`.
        let borrows = unsafe { &mut *self.borrows.get() };
        borrows.release_many(&access.read, &access.write);
    }

    pub(crate) fn ensure_locations(&mut self, index: u32) {
        if self.locations.len() <= index as usize {
            self.locations
                .resize(index as usize + 1, EntityLocation::INVALID);
        }
    }

    pub(crate) fn reserve_entity(&mut self) -> Entity {
        let entity = self.allocator.reserve();
        self.ensure_locations(entity.index());
        entity
    }

    pub(crate) fn validate_bundle_ids(&self, ids: &[ComponentId]) {
        let mut sorted: SmallVec<[ComponentId; 16]> = ids.iter().copied().collect();
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            if pair[0] == pair[1] {
                panic!(
                    "ecs: duplicate component in bundle: {:?} appears more than once",
                    pair[0]
                );
            }
        }
    }

    fn spawn_with_ids<B: Bundle>(&mut self, bundle: B, ids: &[ComponentId]) -> Entity {
        let mut table_ids: SmallVec<[ComponentId; 8]> = SmallVec::new();
        for &id in ids {
            if self.registry.storage(id) == Storage::Table {
                table_ids.push(id);
            }
        }
        let archetype = self.ensure_archetype(table_ids);
        let row = self.tables[archetype as usize].begin_row();
        let entity = self.allocator.spawn();
        self.ensure_locations(entity.index());
        self.locations[entity.index() as usize] = EntityLocation {
            generation: entity.generation(),
            archetype,
            row,
        };
        bundle.put(self, entity);
        self.tables[archetype as usize].commit_row(entity, row);
        entity
    }

    /// Writes a freshly-spawned component into an entity's current row.
    pub(crate) fn put_new<T: Send + 'static>(&mut self, e: Entity, value: T) {
        let id = self.registry.require::<T>();
        match self.registry.storage(id) {
            Storage::Table => {
                let location = self.locations[e.index() as usize];
                let table = &mut self.tables[location.archetype as usize];
                let column = table
                    .column_mut(id)
                    .expect("ecs: flush invariant: missing column for bundle component");
                let ptr = column.ptr_at(location.row as usize).cast::<T>();
                // SAFETY: the row was reserved for this entity and the slot is
                // uninitialized.
                unsafe { ptr::write(ptr, value) };
            }
            Storage::SparseSet => {
                self.sparse_typed_mut::<T>(id).insert(e, value);
            }
        }
    }

    pub(crate) fn sparse_typed_mut<T: Send + 'static>(
        &mut self,
        id: ComponentId,
    ) -> &mut SparseSet<T> {
        let set = self.sparse_sets[id.0 as usize]
            .as_mut()
            .expect("ecs: flush invariant: sparse component has no sparse set");
        set.as_any_mut()
            .downcast_mut::<SparseSet<T>>()
            .expect("ecs: flush invariant: sparse set type mismatch")
    }

    pub(crate) fn sparse_raw(&self, id: ComponentId, e: Entity) -> Option<*mut u8> {
        let set = self.sparse_sets.get(id.0 as usize)?.as_ref()?;
        set.get_ptr(e).map(|ptr| ptr.as_ptr())
    }

    pub(crate) fn component_ptr<T: Send + 'static>(&self, e: Entity) -> Option<*mut T> {
        let id = self.registry.id_of::<T>()?;
        let location = *self.locations.get(e.index() as usize)?;
        if location.archetype == u32::MAX {
            return None;
        }
        match self.registry.storage(id) {
            Storage::Table => {
                let column = self.tables.get(location.archetype as usize)?.column(id)?;
                Some(column.ptr_at(location.row as usize).cast::<T>())
            }
            Storage::SparseSet => self.sparse_raw(id, e).map(|ptr| ptr.cast::<T>()),
        }
    }

    /// # Safety
    ///
    /// The caller guarantees that `e` is alive, has `T`, and that no
    /// conflicting reference exists.
    pub(crate) unsafe fn raw_get<T: Send + 'static>(&self, e: Entity) -> Option<&T> {
        let ptr = self.component_ptr::<T>(e)?;
        // SAFETY: upheld by the caller.
        Some(unsafe { &*ptr })
    }

    /// # Safety
    ///
    /// The caller guarantees that `e` is alive, has `T`, and that no
    /// conflicting reference exists.
    pub(crate) unsafe fn raw_get_mut<T: Send + 'static>(&mut self, e: Entity) -> Option<&mut T> {
        let ptr = self.component_ptr::<T>(e)?;
        // SAFETY: upheld by the caller.
        Some(unsafe { &mut *ptr })
    }

    pub(crate) fn raw_get_mut_ptr<T: Send + 'static>(&self, e: Entity) -> Option<*mut T> {
        self.component_ptr::<T>(e)
    }

    pub(crate) fn require_alive(&self, e: Entity) {
        if !self.is_alive(e) {
            panic!("ecs: dead entity: {e:?} is not alive");
        }
    }

    /// Drops every sparse value belonging to `e` without touching its table
    /// row. Used by flush cleanup after a partially-applied spawn.
    pub(crate) fn purge_sparse(&mut self, e: Entity) {
        for set in self.sparse_sets.iter_mut().flatten() {
            if set.contains(e) {
                if let Some(value) = set.remove_boxed(e) {
                    let _ = catch_unwind(AssertUnwindSafe(|| drop(value)));
                }
            }
        }
    }

    pub(crate) fn despawn_inner(
        &mut self,
        e: Entity,
        panics: Option<&mut Vec<Box<dyn Any + Send>>>,
    ) -> bool {
        if !self.is_alive(e) {
            return false;
        }
        let location = self.locations[e.index() as usize];
        let mut scratch = self.tables[location.archetype as usize].take_row(location.row);

        let mut sparse_values: SmallVec<[Box<dyn Any>; 4]> = SmallVec::new();
        for slot in 0..self.sparse_sets.len() {
            if let Some(set) = self.sparse_sets[slot].as_mut() {
                if set.contains(e) {
                    if let Some(value) = set.remove_boxed(e) {
                        sparse_values.push(value);
                    }
                }
            }
        }

        let moved = self.tables[location.archetype as usize].swap_remove_nodrop(location.row);
        if let Some(moved) = moved {
            self.locations[moved.index() as usize].row = location.row;
        }
        self.allocator.free_and_bump(e.index());
        self.locations[e.index() as usize] = EntityLocation::INVALID;

        match panics {
            Some(sink) => {
                scratch.drop_items(Some(sink));
                for value in sparse_values {
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
                        sink.push(payload);
                    }
                }
            }
            None => {
                scratch.drop_items(None);
                drop(sparse_values);
            }
        }
        true
    }

    fn remove_source_row(&mut self, archetype: u32, row: u32) {
        let moved = self.tables[archetype as usize].swap_remove_nodrop(row);
        if let Some(moved) = moved {
            self.locations[moved.index() as usize].row = row;
        }
    }

    fn migrate_add<T: Send + 'static>(
        &mut self,
        e: Entity,
        value: T,
        id: ComponentId,
        location: EntityLocation,
    ) {
        let source = location.archetype;
        let target = self.archetype_add(source, id);
        let target_row = self.tables[target as usize].begin_row();

        {
            let (source_table, target_table) = two_tables_mut(&mut self.tables, source, target);
            copy_shared_columns(source_table, location.row, target_table, target_row);
            let column = target_table
                .column(id)
                .expect("ecs: flush invariant: migrated component has no target column");
            let ptr = column.ptr_at(target_row as usize).cast::<T>();
            // SAFETY: the target row is uninitialized.
            unsafe { ptr::write(ptr, value) };
        }

        self.locations[e.index() as usize] = EntityLocation {
            generation: location.generation,
            archetype: target,
            row: target_row,
        };
        self.tables[target as usize].commit_row(e, target_row);
        self.remove_source_row(source, location.row);
    }

    fn migrate_remove<T: Send + 'static>(
        &mut self,
        e: Entity,
        id: ComponentId,
        location: EntityLocation,
    ) -> T {
        let source = location.archetype;
        let target = self.archetype_remove(source, id);
        let target_row = self.tables[target as usize].begin_row();

        let old = {
            let (source_table, target_table) = two_tables_mut(&mut self.tables, source, target);
            copy_shared_columns_except(source_table, location.row, target_table, target_row, id);
            let column = source_table
                .column(id)
                .expect("ecs: flush invariant: removed component has no source column");
            let ptr = column.ptr_at(location.row as usize).cast::<T>();
            // SAFETY: the source slot holds an initialized T that is moved out;
            // the source row is subsequently overwritten by swap-remove.
            unsafe { ptr::read(ptr) }
        };

        self.locations[e.index() as usize] = EntityLocation {
            generation: location.generation,
            archetype: target,
            row: target_row,
        };
        self.tables[target as usize].commit_row(e, target_row);
        self.remove_source_row(source, location.row);
        old
    }

    /// Materializes a reserved spawn. Used by `CommandBuffer::flush`.
    pub(crate) fn materialize_reserved<F>(&mut self, e: Entity, ids: &[ComponentId], apply: F)
    where
        F: FnOnce(&mut World, Entity),
    {
        if self.allocator.state(e.index()) != SlotState::Reserved {
            panic!("ecs: flush invariant: spawn slot {e:?} is not reserved");
        }
        let mut table_ids: SmallVec<[ComponentId; 8]> = SmallVec::new();
        for &id in ids {
            if self.registry.storage(id) == Storage::Table {
                table_ids.push(id);
            }
        }
        let archetype = self.ensure_archetype(table_ids);
        let row = self.tables[archetype as usize].begin_row();
        self.locations[e.index() as usize] = EntityLocation {
            generation: e.generation(),
            archetype,
            row,
        };
        apply(self, e);
        self.tables[archetype as usize].commit_row(e, row);
        self.allocator.materialize(e);
    }
}

fn two_tables_mut(tables: &mut [Table], a: u32, b: u32) -> (&mut Table, &mut Table) {
    assert_ne!(a, b, "ecs: flush invariant: migration source equals target");
    let ptr = tables.as_mut_ptr();
    // SAFETY: the indices are distinct and in bounds; the two references do not
    // alias.
    unsafe { (&mut *ptr.add(a as usize), &mut *ptr.add(b as usize)) }
}

fn copy_shared_columns(source: &Table, source_row: u32, target: &Table, target_row: u32) {
    for (id, source_column) in source.columns() {
        let target_column = target
            .column(*id)
            .expect("ecs: flush invariant: target table lost a shared column");
        let size = source_column.layout().size();
        if size != 0 {
            // SAFETY: both rows are within their table's length (the target row
            // is reserved but uninitialized) and the columns are disjoint.
            unsafe {
                ptr::copy_nonoverlapping(
                    source_column.ptr_at(source_row as usize),
                    target_column.ptr_at(target_row as usize),
                    size,
                );
            }
        }
    }
}

fn copy_shared_columns_except(
    source: &Table,
    source_row: u32,
    target: &Table,
    target_row: u32,
    removed: ComponentId,
) {
    for (id, source_column) in source.columns() {
        if *id == removed {
            continue;
        }
        let target_column = target
            .column(*id)
            .expect("ecs: flush invariant: target table lost a shared column");
        let size = source_column.layout().size();
        if size != 0 {
            // SAFETY: as in `copy_shared_columns`.
            unsafe {
                ptr::copy_nonoverlapping(
                    source_column.ptr_at(source_row as usize),
                    target_column.ptr_at(target_row as usize),
                    size,
                );
            }
        }
    }
}
