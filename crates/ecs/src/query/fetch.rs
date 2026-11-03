//! Fetch implementations for the supported query terms.
//!
//! Supported yields: `&T`, `&mut T`, `Entity`, `Option<&T>`, `Option<&mut T>`.
//! Supported zero-yield filters: [`With<T>`], [`Without<T>`].
//!
//! Every fetch checks presence itself. Table filters are additionally pushed
//! into the plan to skip whole archetypes, but correctness never depends on
//! that optimization.

use core::marker::PhantomData;

use crate::component::{ComponentId, Storage};
use crate::entity::Entity;
use crate::query::plan::QueryDesc;
use crate::query::sealed::Sealed;
use crate::world::World;

/// Filter: the entity must have component `T`.
///
/// `With<T>` yields nothing; it only narrows the query.
pub struct With<T>(PhantomData<T>);

/// Filter: the entity must not have component `T`.
///
/// `Without<T>` yields nothing; it only narrows the query.
pub struct Without<T>(PhantomData<T>);

/// One element of a query tuple.
#[doc(hidden)]
pub trait Fetch: Sealed + 'static {
    type Item<'q>;
    type State: Copy + 'static;

    fn state(world: &World) -> Self::State;
    fn describe(world: &World, desc: &mut QueryDesc);

    /// # Safety
    ///
    /// `archetype` and `row` must describe a live, matching entity, and the
    /// caller must hold the borrows required by the query plan.
    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>>;

    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool;
}

/// Marker for fetch terms that never yield `&mut`.
#[doc(hidden)]
pub trait ReadOnlyFetch: Fetch {}

fn present(world: &World, entity: Entity, id: ComponentId) -> bool {
    match world.registry.storage(id) {
        Storage::Table => {
            let index = entity.index() as usize;
            match world.locations.get(index) {
                Some(location) if location.archetype != u32::MAX => {
                    world.tables[location.archetype as usize].contains(id)
                }
                _ => false,
            }
        }
        Storage::SparseSet => world.sparse_raw(id, entity).is_some(),
    }
}

fn table_ptr(world: &World, archetype: u32, row: u32, id: ComponentId) -> Option<*mut u8> {
    world
        .tables
        .get(archetype as usize)?
        .column(id)
        .map(|column| column.ptr_at(row as usize))
}

impl<T: Send + 'static> Sealed for &'static T {}
impl<T: Send + 'static> Fetch for &'static T {
    type Item<'q> = &'q T;
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        desc.read.push(id);
        desc.terms.push(id);
        match world.registry.storage(id) {
            Storage::Table => desc.required_table.push(id),
            Storage::SparseSet => desc.required_sparse.push(id),
        }
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        let ptr = match world.registry.storage(*state) {
            Storage::Table => table_ptr(world, archetype, row, *state)?,
            Storage::SparseSet => world.sparse_raw(*state, entity)?,
        };
        // SAFETY: the caller guarantees the entity matches and that the plan's
        // borrows are held. The table/sparse slot is initialized for T.
        Some(unsafe { &*ptr.cast::<T>() })
    }

    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool {
        present(world, entity, *state)
    }
}

impl<T: Send + 'static> ReadOnlyFetch for &'static T {}

impl<T: Send + 'static> Sealed for &'static mut T {}
impl<T: Send + 'static> Fetch for &'static mut T {
    type Item<'q> = &'q mut T;
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        desc.write.push(id);
        desc.terms.push(id);
        match world.registry.storage(id) {
            Storage::Table => desc.required_table.push(id),
            Storage::SparseSet => desc.required_sparse.push(id),
        }
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        let ptr = match world.registry.storage(*state) {
            Storage::Table => table_ptr(world, archetype, row, *state)?,
            Storage::SparseSet => world.sparse_raw(*state, entity)?,
        };
        // SAFETY: the caller guarantees exclusive access to this component for
        // `'q`; the plan acquired the write borrow and no other item in the
        // same query aliases it.
        Some(unsafe { &mut *ptr.cast::<T>() })
    }

    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool {
        present(world, entity, *state)
    }
}

impl<T: Send + 'static> Sealed for Option<&'static T> {}
impl<T: Send + 'static> Fetch for Option<&'static T> {
    type Item<'q> = Option<&'q T>;
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        desc.read.push(id);
        desc.terms.push(id);
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        let ptr = match world.registry.storage(*state) {
            Storage::Table => table_ptr(world, archetype, row, *state),
            Storage::SparseSet => world.sparse_raw(*state, entity),
        };
        // SAFETY: as for `&T`; absence yields `None`.
        Some(ptr.map(|ptr| unsafe { &*ptr.cast::<T>() }))
    }

    fn matches(_world: &World, _entity: Entity, _state: &Self::State) -> bool {
        true
    }
}

impl<T: Send + 'static> ReadOnlyFetch for Option<&'static T> {}

impl<T: Send + 'static> Sealed for Option<&'static mut T> {}
impl<T: Send + 'static> Fetch for Option<&'static mut T> {
    type Item<'q> = Option<&'q mut T>;
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        desc.write.push(id);
        desc.terms.push(id);
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        let ptr = match world.registry.storage(*state) {
            Storage::Table => table_ptr(world, archetype, row, *state),
            Storage::SparseSet => world.sparse_raw(*state, entity),
        };
        // SAFETY: as for `&mut T`; absence yields `None`.
        Some(ptr.map(|ptr| unsafe { &mut *ptr.cast::<T>() }))
    }

    fn matches(_world: &World, _entity: Entity, _state: &Self::State) -> bool {
        true
    }
}

impl Sealed for Entity {}
impl Fetch for Entity {
    type Item<'q> = Entity;
    type State = ();

    fn state(_world: &World) -> Self::State {}

    fn describe(_world: &World, desc: &mut QueryDesc) {
        desc.terms.push(ComponentId::ENTITY);
    }

    unsafe fn fetch<'q>(
        _world: &'q World,
        entity: Entity,
        _archetype: u32,
        _row: u32,
        _state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        Some(entity)
    }

    fn matches(_world: &World, _entity: Entity, _state: &Self::State) -> bool {
        true
    }
}

impl ReadOnlyFetch for Entity {}

impl<T: Send + 'static> Sealed for With<T> {}
impl<T: Send + 'static> Fetch for With<T> {
    type Item<'q> = ();
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        match world.registry.storage(id) {
            Storage::Table => desc.required_table.push(id),
            Storage::SparseSet => {
                // Sparse presence is checked per row, which reads the sparse
                // array and therefore counts as a read borrow.
                desc.read.push(id);
                desc.required_sparse.push(id);
            }
        }
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        _archetype: u32,
        _row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        if present(world, entity, *state) {
            Some(())
        } else {
            None
        }
    }

    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool {
        present(world, entity, *state)
    }
}

impl<T: Send + 'static> ReadOnlyFetch for With<T> {}

impl<T: Send + 'static> Sealed for Without<T> {}
impl<T: Send + 'static> Fetch for Without<T> {
    type Item<'q> = ();
    type State = ComponentId;

    fn state(world: &World) -> Self::State {
        world.require_component::<T>()
    }

    fn describe(world: &World, desc: &mut QueryDesc) {
        let id = world.require_component::<T>();
        match world.registry.storage(id) {
            Storage::Table => desc.excluded_table.push(id),
            Storage::SparseSet => {
                desc.read.push(id);
                desc.excluded_sparse.push(id);
            }
        }
    }

    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        _archetype: u32,
        _row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        if present(world, entity, *state) {
            None
        } else {
            Some(())
        }
    }

    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool {
        !present(world, entity, *state)
    }
}

impl<T: Send + 'static> ReadOnlyFetch for Without<T> {}

// Referenced by query/mod.rs for clarity in documentation.
const _: Option<fn() -> Storage> = None;
