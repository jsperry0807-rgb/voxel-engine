//! Query surface: guard types, tuple `QuerySpec` impls, and plan wiring.

mod fetch;
mod iter;
pub(crate) mod plan;

use core::marker::PhantomData;
use std::ops::ControlFlow;

use crate::entity::Entity;
use crate::query::plan::{validate_desc, QueryPlan};
use crate::world::World;
#[doc(hidden)]
pub use fetch::{Fetch, ReadOnlyFetch};
pub use fetch::{With, Without};
pub use iter::{QueryIter, QueryIterMut};

pub(crate) mod sealed {
    /// Seals [`QuerySpec`](super::QuerySpec) and the fetch traits.
    pub trait Sealed {}
}

/// A statically known query.
///
/// Implemented for `()` and tuples of one through sixteen supported terms:
/// `&T`, `&mut T`, `Entity`, `Option<&T>`, `Option<&mut T>`, `With<T>`, and
/// `Without<T>`. The trait is sealed.
pub trait QuerySpec: sealed::Sealed + 'static {
    /// The item yielded for one matching entity.
    type Item<'q>
    where
        Self: 'q;

    /// Per-query resolved component ids, computed once at construction.
    #[doc(hidden)]
    type State: Copy + 'static;

    #[doc(hidden)]
    fn state(world: &World) -> Self::State;

    #[doc(hidden)]
    fn describe(world: &World) -> plan::QueryDesc;

    #[doc(hidden)]
    /// # Safety
    ///
    /// The caller must pass a live, matching entity, the archetype/row that
    /// contains it, and must hold the plan's borrows for `'q`. Violating these
    /// conditions is undefined behavior.
    unsafe fn fetch<'q>(
        world: &'q World,
        entity: Entity,
        archetype: u32,
        row: u32,
        state: &Self::State,
    ) -> Option<Self::Item<'q>>;

    #[doc(hidden)]
    fn matches(world: &World, entity: Entity, state: &Self::State) -> bool;
}

/// A [`QuerySpec`] that contains no `&mut T` terms.
///
/// Required by [`World::query`]. Writing through such a query is impossible at
/// compile time, which is what makes `World::query` safe to call on `&World`.
pub trait ReadOnlyQuery: QuerySpec {}

impl sealed::Sealed for () {}

impl QuerySpec for () {
    type Item<'q> = ();

    type State = ();

    fn state(_world: &World) -> Self::State {}

    fn describe(_world: &World) -> plan::QueryDesc {
        plan::QueryDesc::new()
    }

    unsafe fn fetch<'q>(
        _world: &'q World,
        _entity: Entity,
        _archetype: u32,
        _row: u32,
        _state: &Self::State,
    ) -> Option<Self::Item<'q>> {
        Some(())
    }

    fn matches(_world: &World, _entity: Entity, _state: &Self::State) -> bool {
        true
    }
}

impl ReadOnlyQuery for () {}

macro_rules! impl_query_tuple {
    ($(($T:ident, $idx:tt)),+) => {
        impl<$($T: Fetch),+> sealed::Sealed for ($($T,)+) {}

        impl<$($T: Fetch),+> QuerySpec for ($($T,)+) {
            type Item<'q> = ($($T::Item<'q>,)+) where Self: 'q;

            type State = ($($T::State,)+);

            fn state(world: &World) -> Self::State {
                ($($T::state(world),)+)
            }

            fn describe(world: &World) -> plan::QueryDesc {
                let mut desc = plan::QueryDesc::new();
                $( $T::describe(world, &mut desc); )+
                desc
            }

            unsafe fn fetch<'q>(
                world: &'q World,
                entity: Entity,
                archetype: u32,
                row: u32,
                state: &Self::State,
            ) -> Option<Self::Item<'q>> {
                Some((
                    $(
                        // SAFETY: the caller's contract is forwarded unchanged.
                        unsafe { $T::fetch(world, entity, archetype, row, &state.$idx) }?,
                    )+
                ))
            }

            fn matches(world: &World, entity: Entity, state: &Self::State) -> bool {
                true $(&& $T::matches(world, entity, &state.$idx))+
            }
        }

        impl<$($T: ReadOnlyFetch),+> ReadOnlyQuery for ($($T,)+) {}
    };
}

impl_query_tuple!((A, 0));
impl_query_tuple!((A, 0), (B, 1));
impl_query_tuple!((A, 0), (B, 1), (C, 2));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10),
    (L, 11)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10),
    (L, 11),
    (M, 12)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10),
    (L, 11),
    (M, 12),
    (N, 13)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10),
    (L, 11),
    (M, 12),
    (N, 13),
    (O, 14)
);
impl_query_tuple!(
    (A, 0),
    (B, 1),
    (C, 2),
    (D, 3),
    (E, 4),
    (F, 5),
    (G, 6),
    (H, 7),
    (I, 8),
    (J, 9),
    (K, 10),
    (L, 11),
    (M, 12),
    (N, 13),
    (O, 14),
    (P, 15)
);

/// A read-only query guard.
///
/// Construct with [`World::query`]. Holds read borrows for the components in
/// `Q` until dropped.
///
/// Queries are `!Send` because they hold `&World` and `World` is `!Sync`:
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<my_ecs::Query<'static, (&'static u32,)>>();
/// ```
#[must_use = "queries hold borrows until dropped"]
pub struct Query<'w, Q: QuerySpec> {
    world: &'w World,
    plan: &'w QueryPlan,
    state: Q::State,
    _marker: PhantomData<Q>,
}

impl<'w, Q: QuerySpec> Query<'w, Q> {
    pub(crate) fn new(world: &'w World) -> Self {
        let state = Q::state(world);
        let desc = Q::describe(world);
        validate_desc(&desc);
        let plan = world.plan_for::<Q>(&desc);
        world.acquire_borrows(&plan.access);
        Self {
            world,
            plan,
            state,
            _marker: PhantomData,
        }
    }

    /// Iterates all matching entities.
    ///
    /// The iteration order is unspecified and may change after any structural
    /// mutation.
    pub fn iter<'q>(&'q self) -> QueryIter<'w, 'q, Q>
    where
        'w: 'q,
    {
        QueryIter::new(self.world, self.plan, self.state)
    }

    /// Runs `f` for every matching entity.
    pub fn for_each<'q>(&'q self, mut f: impl FnMut(Q::Item<'q>))
    where
        'w: 'q,
    {
        for item in self.iter() {
            f(item);
        }
    }

    /// Runs `f` for every matching entity until `f` returns `Break`.
    pub fn try_for_each<'q, R>(
        &'q self,
        mut f: impl FnMut(Q::Item<'q>) -> ControlFlow<R>,
    ) -> ControlFlow<R>
    where
        'w: 'q,
    {
        for item in self.iter() {
            f(item)?;
        }
        ControlFlow::Continue(())
    }

    /// Returns the `T` of `e` if `e` is alive, matches the query, and has `T`.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if `T` was never registered.
    /// * `"ecs: query access violation:"` if `T` is not in `Q`'s read set.
    ///
    /// # Examples
    ///
    /// ```
    /// use my_ecs::{Storage, World};
    /// struct A(u32);
    /// let mut world = World::new();
    /// world.register::<A>(Storage::Table);
    /// let e = world.spawn((A(3),));
    /// let q = world.query::<(&A,)>();
    /// assert_eq!(q.get::<A>(e).map(|a| a.0), Some(3));
    /// ```
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T> {
        let id = self.world.require_component::<T>();
        if !self.plan.access.allows_read(id) {
            panic!(
                "ecs: query access violation: `{}` is not in this query's read set",
                core::any::type_name::<T>()
            );
        }
        if !self.world.is_alive(e) {
            return None;
        }
        if !Q::matches(self.world, e, &self.state) {
            return None;
        }
        // SAFETY: the entity is alive and matches the query, the type is
        // registered, and the guard holds the component's read borrow.
        unsafe { self.world.raw_get::<T>(e) }
    }

    /// Returns whether `e` is alive and matches the query.
    pub fn contains(&self, e: Entity) -> bool {
        self.world.is_alive(e) && Q::matches(self.world, e, &self.state)
    }
}

impl<Q: QuerySpec> Drop for Query<'_, Q> {
    fn drop(&mut self) {
        self.world.release_borrows(&self.plan.access);
    }
}

/// A mutable query guard.
///
/// Construct with [`World::query_mut`]. Holds write borrows for every `&mut T`
/// term and read borrows for the rest until dropped.
#[must_use = "queries hold borrows until dropped"]
pub struct QueryMut<'w, Q: QuerySpec> {
    _not_send: PhantomData<*const ()>,
    world: &'w World,
    plan: &'w QueryPlan,
    state: Q::State,
    _exclusive: PhantomData<&'w mut World>,
}

impl<'w, Q: QuerySpec> QueryMut<'w, Q> {
    pub(crate) fn new(world: &'w mut World) -> Self {
        let world: &'w World = world;
        let state = Q::state(world);
        let desc = Q::describe(world);
        validate_desc(&desc);
        let plan = world.plan_for::<Q>(&desc);
        world.acquire_borrows(&plan.access);
        Self {
            _not_send: PhantomData,
            world,
            plan,
            state,
            _exclusive: PhantomData,
        }
    }

    /// Iterates all matching entities, yielding mutable component references.
    pub fn iter_mut<'q>(&'q mut self) -> QueryIterMut<'w, 'q, Q>
    where
        'w: 'q,
    {
        QueryIterMut::new(self.world, self.plan, self.state)
    }

    /// Runs `f` for every matching entity.
    pub fn for_each<'q>(&'q mut self, mut f: impl FnMut(Q::Item<'q>))
    where
        'w: 'q,
    {
        for item in self.iter_mut() {
            f(item);
        }
    }

    /// Runs `f` for every matching entity until `f` returns `Break`.
    pub fn try_for_each<'q, R>(
        &'q mut self,
        mut f: impl FnMut(Q::Item<'q>) -> ControlFlow<R>,
    ) -> ControlFlow<R>
    where
        'w: 'q,
    {
        for item in self.iter_mut() {
            f(item)?;
        }
        ControlFlow::Continue(())
    }

    /// Returns the `T` of `e` if `e` is alive, matches the query, and has `T`.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if `T` was never registered.
    /// * `"ecs: query access violation:"` if `T` is in neither the read nor the
    ///   write set of `Q`.
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T> {
        let id = self.world.require_component::<T>();
        if !self.plan.access.allows_read(id) {
            panic!(
                "ecs: query access violation: `{}` is not in this query's read or write set",
                core::any::type_name::<T>()
            );
        }
        if !self.world.is_alive(e) || !Q::matches(self.world, e, &self.state) {
            return None;
        }
        // SAFETY: the entity is alive and matches the query; the guard holds at
        // least a read borrow (or a write borrow, which subsumes reads).
        unsafe { self.world.raw_get::<T>(e) }
    }

    /// Returns a mutable `T` of `e`.
    ///
    /// # Panics
    ///
    /// * `"ecs: component not registered:"` if `T` was never registered.
    /// * `"ecs: query access violation:"` if `T` is not in the write set.
    pub fn get_mut<T: Send + 'static>(&mut self, e: Entity) -> Option<&mut T> {
        let id = self.world.require_component::<T>();
        if !self.plan.access.allows_write(id) {
            panic!(
                "ecs: query access violation: `{}` is not in this query's write set",
                core::any::type_name::<T>()
            );
        }
        if !self.world.is_alive(e) || !Q::matches(self.world, e, &self.state) {
            return None;
        }
        let ptr = self.world.raw_get_mut_ptr::<T>(e)?;
        // SAFETY: the guard holds the write borrow, and `&mut self` prevents
        // another call while the returned reference lives.
        Some(unsafe { &mut *ptr })
    }

    /// Returns whether `e` is alive and matches the query.
    pub fn contains(&self, e: Entity) -> bool {
        self.world.is_alive(e) && Q::matches(self.world, e, &self.state)
    }
}

impl<Q: QuerySpec> Drop for QueryMut<'_, Q> {
    fn drop(&mut self) {
        self.world.release_borrows(&self.plan.access);
    }
}
