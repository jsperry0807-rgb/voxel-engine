//! Anchor-specific iteration loops.

use core::marker::PhantomData;

use crate::entity::SlotState;
use crate::query::QuerySpec;
use crate::query::plan::{AnchorMode, QueryPlan};
use crate::world::World;

pub(crate) enum Kind {
    Table { match_index: usize, row: u32 },
    Sparse { index: usize },
    Live { index: u32 },
}

fn kind_for(plan: &QueryPlan) -> Kind {
    match plan.anchor {
        AnchorMode::TableArchetypes => Kind::Table {
            match_index: 0,
            row: 0,
        },
        AnchorMode::SmallestSparseSet(_) => Kind::Sparse { index: 0 },
        AnchorMode::AllLiveEntities => Kind::Live { index: 0 },
    }
}

#[inline]
fn next_item<'w, 'q, Q: QuerySpec>(
    world: &'w World,
    plan: &'w QueryPlan,
    state: &Q::State,
    kind: &mut Kind,
) -> Option<Q::Item<'q>>
where
    'w: 'q,
{
    let world: &'q World = world;
    loop {
        match kind {
            Kind::Table { match_index, row } => {
                let matched = plan.matches.get(*match_index)?;
                if matched.skip {
                    *match_index += 1;
                    *row = 0;
                    continue;
                }
                let table = &world.tables[matched.table as usize];
                if *row as usize >= table.len() {
                    *match_index += 1;
                    *row = 0;
                    continue;
                }
                let entity = table.entity_rows()[*row as usize];
                let current_row = *row;
                *row += 1;
                let item = unsafe {
                    // SAFETY: the row is committed, the plan match asserts the
                    // archetype was filtered for table components, and the
                    // guard holds the plan's borrows. Per-row sparse checks are
                    // performed inside each fetch.
                    Q::fetch(world, entity, matched.archetype, current_row, state)
                };
                if item.is_some() {
                    return item;
                }
            }
            Kind::Sparse { index } => {
                let AnchorMode::SmallestSparseSet(id) = plan.anchor else {
                    return None;
                };
                let set = world.sparse_sets[id.0 as usize]
                    .as_ref()
                    .expect("ecs: flush invariant: sparse anchor set is missing");
                let entities = set.entities();
                while *index < entities.len() {
                    let entity = entities[*index];
                    *index += 1;
                    let location = world.locations[entity.index() as usize];
                    let item = unsafe {
                        // SAFETY: sparse sets only contain live entities and
                        // the guard holds the plan's borrows.
                        Q::fetch(world, entity, location.archetype, location.row, state)
                    };
                    if item.is_some() {
                        return item;
                    }
                }
                return None;
            }
            Kind::Live { index } => {
                while (*index as usize) < world.allocator.len() {
                    let slot = *index;
                    *index += 1;
                    if world.allocator.state(slot) != SlotState::Alive {
                        continue;
                    }
                    let entity = world.allocator.entity(slot);
                    let location = world.locations[slot as usize];
                    let item = unsafe {
                        // SAFETY: the slot is alive, its location is valid, and
                        // the guard holds the plan's borrows.
                        Q::fetch(world, entity, location.archetype, location.row, state)
                    };
                    if item.is_some() {
                        return item;
                    }
                }
                return None;
            }
        }
    }
}

/// Iterator returned by [`Query::iter`](crate::Query::iter).
pub struct QueryIter<'w, 'q, Q: QuerySpec>
where
    'w: 'q,
{
    world: &'w World,
    plan: &'w QueryPlan,
    state: Q::State,
    kind: Kind,
    _marker: PhantomData<(&'q (), Q)>,
}

impl<'w, 'q, Q: QuerySpec> QueryIter<'w, 'q, Q>
where
    'w: 'q,
{
    pub(crate) fn new(world: &'w World, plan: &'w QueryPlan, state: Q::State) -> Self {
        let kind = kind_for(plan);
        Self {
            world,
            plan,
            state,
            kind,
            _marker: PhantomData,
        }
    }
}

impl<'w, 'q, Q: QuerySpec> Iterator for QueryIter<'w, 'q, Q>
where
    'w: 'q,
{
    type Item = Q::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        next_item::<Q>(self.world, self.plan, &self.state, &mut self.kind)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

/// Iterator returned by [`QueryMut::iter_mut`](crate::QueryMut::iter_mut).
pub struct QueryIterMut<'w, 'q, Q: QuerySpec>
where
    'w: 'q,
{
    world: &'w World,
    plan: &'w QueryPlan,
    state: Q::State,
    kind: Kind,
    _not_send: PhantomData<*const ()>,
    _marker: PhantomData<(&'q mut (), Q)>,
}

impl<'w, 'q, Q: QuerySpec> QueryIterMut<'w, 'q, Q>
where
    'w: 'q,
{
    pub(crate) fn new(world: &'w World, plan: &'w QueryPlan, state: Q::State) -> Self {
        let kind = kind_for(plan);
        Self {
            world,
            plan,
            state,
            kind,
            _not_send: PhantomData,
            _marker: PhantomData,
        }
    }
}

impl<'w, 'q, Q: QuerySpec> Iterator for QueryIterMut<'w, 'q, Q>
where
    'w: 'q,
{
    type Item = Q::Item<'q>;

    fn next(&mut self) -> Option<Self::Item> {
        next_item::<Q>(self.world, self.plan, &self.state, &mut self.kind)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}
