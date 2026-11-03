//! `EntityRef` and `EntityMut` — whole-entity borrow views.

use core::marker::PhantomData;

use crate::component::ComponentId;
use crate::entity::Entity;
use crate::prelude::*;
use crate::query::plan::AccessSet;
use crate::world::World;

fn collect_holds(world: &World, entity: Entity) -> SmallVec<[ComponentId; 8]> {
    let location = world.locations[entity.index() as usize];
    let mut holds: SmallVec<[ComponentId; 8]> = SmallVec::new();
    for &id in world.archetypes[location.archetype as usize].components() {
        holds.push(id);
    }
    for (slot, set) in world.sparse_sets.iter().enumerate() {
        if let Some(set) = set {
            if set.contains(entity) {
                holds.push(ComponentId(slot as u32));
            }
        }
    }
    holds
}

fn read_set(ids: &[ComponentId]) -> AccessSet {
    let mut set = AccessSet {
        read: SmallVec::new(),
        write: SmallVec::new(),
    };
    set.read.extend(ids.iter().copied());
    set
}

fn write_set(ids: &[ComponentId]) -> AccessSet {
    let mut set = AccessSet {
        read: SmallVec::new(),
        write: SmallVec::new(),
    };
    set.write.extend(ids.iter().copied());
    set
}

/// A read-only borrow of every component on one entity.
///
/// Created by [`World::entity_ref`]. While the view is alive, no conflicting
/// mutable borrow of any of the entity's components may be acquired.
#[must_use = "entity views hold borrows until dropped"]
pub struct EntityRef<'w> {
    world: &'w World,
    entity: Entity,
    holds: SmallVec<[ComponentId; 8]>,
}

impl<'w> EntityRef<'w> {
    pub(crate) fn new(world: &'w World, entity: Entity) -> Self {
        world.require_alive(entity);
        let holds = collect_holds(world, entity);
        world.acquire_borrows(&read_set(&holds));
        Self {
            world,
            entity,
            holds,
        }
    }

    /// The viewed entity.
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Returns the component of type `T` on the viewed entity, if present.
    ///
    /// Returns `None` for unregistered types as well as absent components.
    pub fn get<T: Send + 'static>(&self) -> Option<&T> {
        let id = self.world.registry.id_of::<T>()?;
        if !self.holds.contains(&id) {
            return None;
        }
        // SAFETY: the view holds a read borrow for every component on the
        // entity, the entity is alive, and `T` is present.
        unsafe { self.world.raw_get::<T>(self.entity) }
    }
}

impl Drop for EntityRef<'_> {
    fn drop(&mut self) {
        // Release in reverse acquisition order (spec §15.3).
        let mut access = read_set(&self.holds);
        access.read.reverse();
        self.world.release_borrows(&access);
    }
}

/// An exclusive borrow of every component on one entity.
///
/// Created by [`World::entity_mut`]. While the view is alive, no other borrow
/// of any of the entity's components may be acquired. Use [`EntityMut::get_mut`]
/// for mutation; a second call requires `&mut self` so two mutable references
/// to the same component cannot coexist.
///
/// The view is explicitly `!Send` even though `&mut World` would otherwise be
/// `Send`:
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<voxel_ecs::EntityMut<'static>>();
/// ```
#[must_use = "entity views hold borrows until dropped"]
pub struct EntityMut<'w> {
    _not_send: PhantomData<*const ()>,
    world: &'w mut World,
    entity: Entity,
    holds: SmallVec<[ComponentId; 8]>,
}

impl<'w> EntityMut<'w> {
    pub(crate) fn new(world: &'w mut World, entity: Entity) -> Self {
        world.require_alive(entity);
        let holds = {
            let shared: &World = world;
            collect_holds(shared, entity)
        };
        world.acquire_borrows(&write_set(&holds));
        Self {
            _not_send: PhantomData,
            world,
            entity,
            holds,
        }
    }

    /// The viewed entity.
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Returns the component of type `T` on the viewed entity, if present.
    ///
    /// Returns `None` for unregistered types as well as absent components.
    pub fn get<T: Send + 'static>(&self) -> Option<&T> {
        let id = self.world.registry.id_of::<T>()?;
        if !self.holds.contains(&id) {
            return None;
        }
        // SAFETY: the view holds write borrows for every component on the
        // entity, which subsume reads.
        unsafe { self.world.raw_get::<T>(self.entity) }
    }

    /// Returns a mutable reference to `T`, if present.
    ///
    /// Takes `&mut self` so that two simultaneous `&mut T` values cannot be
    /// produced by this view.
    pub fn get_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        let id = self.world.registry.id_of::<T>()?;
        if !self.holds.contains(&id) {
            return None;
        }
        // SAFETY: the view holds the write borrow, and `&mut self` prevents
        // aliasing calls while the reference lives.
        unsafe { self.world.raw_get_mut::<T>(self.entity) }
    }
}

impl Drop for EntityMut<'_> {
    fn drop(&mut self) {
        // Release in reverse acquisition order (spec §15.3).
        let mut access = write_set(&self.holds);
        access.write.reverse();
        self.world.release_borrows(&access);
    }
}
