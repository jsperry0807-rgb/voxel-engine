//! Deferred structural edits, eager reservation, and flush/cancel semantics.

use core::any::{Any, TypeId};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::component::{Bundle, ComponentId};
use crate::entity::{Entity, SlotState};
use crate::location::EntityLocation;
use crate::prelude::*;
use crate::world::World;

type DropPanics = Vec<Box<dyn Any + Send>>;
type SpawnApply = Box<dyn FnOnce(&mut World, Entity) + Send>;
type CommandApply = Box<dyn FnOnce(&mut World, &mut DropPanics) + Send>;

/// The result of [`World::flush`].
#[derive(Default)]
pub struct FlushOutcome {
    /// Panic payloads from destructors that panicked while a command was being
    /// applied, in attempt order.
    ///
    /// The world remains structurally valid; the affected value is lost.
    pub drop_panics: Vec<Box<dyn Any + Send>>,
}

impl core::fmt::Debug for FlushOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FlushOutcome")
            .field("drop_panics", &self.drop_panics.len())
            .finish()
    }
}

enum Command {
    Spawn {
        entity: Entity,
        ids: SmallVec<[ComponentId; 16]>,
        apply: SpawnApply,
    },
    Insert {
        entity: Entity,
        key: TypeId,
        apply: CommandApply,
    },
    Remove {
        entity: Entity,
        key: TypeId,
        apply: CommandApply,
    },
    Despawn {
        entity: Entity,
    },
}

/// Records structural changes to be applied later with [`World::flush`].
///
/// # Reservation model (D1)
///
/// [`CommandBuffer::spawn`] eagerly reserves a real allocator slot and returns
/// a real [`Entity`]. The slot is `Reserved` (dead to every access API) until
/// the spawn is flushed, at which point it becomes `Alive`.
///
/// # Misuse warning
///
/// `#[must_use]` catches `let _ = CommandBuffer::new();` but **does not**
/// catch a buffer that is created, populated, and then dropped through a
/// variable. In release builds, dropping a buffer with un-flushed reservations
/// leaks those entity slots silently. Always call [`World::flush`] or
/// [`World::cancel`] before dropping a buffer.
#[must_use = "command buffers must be flushed or cancelled"]
pub struct CommandBuffer {
    world_id: Option<u64>,
    clear_epoch: Option<u64>,
    commands: Vec<Command>,
    reservations: SmallVec<[Entity; 8]>,
    despawned: SmallVec<[Entity; 8]>,
    poisoned: bool,
}

impl Default for CommandBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandBuffer {
    /// Creates an empty, unbound buffer.
    pub fn new() -> Self {
        Self {
            world_id: None,
            clear_epoch: None,
            commands: Vec::new(),
            reservations: SmallVec::new(),
            despawned: SmallVec::new(),
            poisoned: false,
        }
    }

    fn bind(&mut self, world: &World) {
        match self.world_id {
            Some(id) if id != world.id => {
                panic!(
                    "ecs: cross-world buffer: bound to world {id}, flushed into world {}",
                    world.id
                );
            }
            None => {
                self.world_id = Some(world.id);
                self.clear_epoch = Some(world.clear_epoch);
            }
            Some(_) => {}
        }
        if self.clear_epoch != Some(world.clear_epoch) {
            panic!("ecs: stale buffer: the world was cleared since this buffer was bound");
        }
    }

    /// Reserves a slot, records a spawn, and returns the entity.
    ///
    /// The entity is dead to all access APIs until the buffer is flushed.
    ///
    /// # Panics
    ///
    /// * `"ecs: duplicate component in bundle:"` / `"ecs: component not
    ///   registered:"` for an invalid bundle.
    /// * `"ecs: cross-world buffer:"` / `"ecs: stale buffer:"` if the buffer is
    ///   bound to another world or a pre-clear epoch.
    /// * `"ecs: entity index exhausted:"` if the allocator is full.
    pub fn spawn<B: Bundle>(&mut self, world: &mut World, bundle: B) -> Entity {
        self.bind(world);
        let ids = B::component_ids(world);
        world.validate_bundle_ids(&ids);
        let entity = world.reserve_entity();
        self.reservations.push(entity);
        debug_assert!(
            self.reservations.len() < 1_000_000,
            "ecs: command buffer leak: {} un-flushed reservations; did you forget to flush?",
            self.reservations.len()
        );
        let apply: SpawnApply = Box::new(move |world, entity| bundle.put(world, entity));
        self.commands.push(Command::Spawn { entity, ids, apply });
        entity
    }

    /// Records an insert. Registration is checked at flush time.
    ///
    /// # Panics
    ///
    /// `"ecs: buffer ordering:"` if `e` was despawned earlier in this buffer.
    pub fn insert<T: Send + 'static>(&mut self, e: Entity, value: T) {
        self.check_ordering(e);
        let key = TypeId::of::<T>();
        let apply: CommandApply = Box::new(move |world, panics| {
            let old = world.insert::<T>(e, value);
            if let Some(old) = old {
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(old))) {
                    panics.push(payload);
                }
            }
        });
        self.commands.push(Command::Insert {
            entity: e,
            key,
            apply,
        });
    }

    /// Records a remove. Registration is checked at flush time.
    ///
    /// # Panics
    ///
    /// `"ecs: buffer ordering:"` if `e` was despawned earlier in this buffer.
    pub fn remove<T: Send + 'static>(&mut self, e: Entity) {
        self.check_ordering(e);
        let key = TypeId::of::<T>();
        let apply: CommandApply = Box::new(move |world, panics| {
            if let Some(value) = world.remove::<T>(e) {
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
                    panics.push(payload);
                }
            }
        });
        self.commands.push(Command::Remove {
            entity: e,
            key,
            apply,
        });
    }

    /// Records a despawn.
    ///
    /// Despawning an entity that was already dead before this buffer started
    /// recording is a no-op at flush. Duplicate despawns are coalesced.
    ///
    /// # Panics
    ///
    /// Never; later `insert`/`remove` calls on `e` panic with
    /// `"ecs: buffer ordering:"`.
    pub fn despawn(&mut self, e: Entity) {
        if self.despawned.contains(&e) {
            return;
        }
        self.despawned.push(e);
        self.commands.push(Command::Despawn { entity: e });
    }

    fn check_ordering(&self, e: Entity) {
        if self.despawned.contains(&e) {
            panic!(
                "ecs: buffer ordering: {e:?} was despawned earlier in this buffer and cannot be touched again"
            );
        }
    }

    fn reset(&mut self) {
        self.commands.clear();
        self.reservations.clear();
        self.despawned.clear();
        self.world_id = None;
        self.clear_epoch = None;
        self.poisoned = false;
    }

    fn coalesce(&mut self) {
        let previous_capacity = self.commands.capacity();
        let mut spawned: SmallVec<[Entity; 8]> = SmallVec::new();
        for command in &self.commands {
            if let Command::Spawn { entity, .. } = command {
                spawned.push(*entity);
            }
        }
        let mut cancelled: SmallVec<[Entity; 8]> = SmallVec::new();
        for entity in &spawned {
            if self.despawned.contains(entity) {
                cancelled.push(*entity);
            }
        }
        self.reservations
            .retain(|entity| !cancelled.contains(entity));

        // Pass 1: drop spawn/despawn pairs and any insert/remove on an entity
        // that is despawned in this buffer.
        let mut kept: Vec<Command> = Vec::with_capacity(self.commands.len());
        for command in self.commands.drain(..) {
            match command {
                Command::Spawn { entity, .. } if cancelled.contains(&entity) => {}
                Command::Despawn { entity } if cancelled.contains(&entity) => {}
                Command::Insert { entity, .. } | Command::Remove { entity, .. }
                    if self.despawned.contains(&entity) => {}
                other => kept.push(other),
            }
        }

        // Pass 2: per (entity, component), the last insert/remove wins.
        let mut seen: SmallVec<[(Entity, TypeId); 16]> = SmallVec::new();
        let mut reversed: Vec<Command> = Vec::with_capacity(kept.len());
        for command in kept.into_iter().rev() {
            match &command {
                Command::Insert { entity, key, .. } | Command::Remove { entity, key, .. } => {
                    let pair = (*entity, *key);
                    if seen.contains(&pair) {
                        continue;
                    }
                    seen.push(pair);
                }
                _ => {}
            }
            reversed.push(command);
        }
        reversed.reverse();
        // Coalescing must not shrink the buffer's retained capacity.
        if reversed.capacity() < previous_capacity {
            reversed.reserve(previous_capacity.saturating_sub(reversed.len()));
        }
        self.commands = reversed;
    }
}

impl Drop for CommandBuffer {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            debug_assert!(
                self.reservations.is_empty(),
                "ecs: command buffer leak: dropping with {} un-flushed reservations; \
                 call World::flush or World::cancel before dropping",
                self.reservations.len()
            );
        }
        // Intentionally does not release slots: a buffer has no `World`
        // reference (D1). See docs/LIMITATIONS.md.
    }
}

impl World {
    /// Applies `cb`'s commands to this world.
    ///
    /// # Panics
    ///
    /// * `"ecs: buffer poisoned:"` if a previous flush panicked.
    /// * `"ecs: cross-world buffer:"` if the buffer belongs to another world.
    /// * `"ecs: stale buffer:"` if the world was cleared after binding.
    ///
    /// A panic while applying a command (for example an unregistered component
    /// in a deferred insert) poisons the buffer and propagates; already-applied
    /// commands stay applied. Destructor panics are collected in
    /// [`FlushOutcome::drop_panics`] instead.
    pub fn flush(&mut self, cb: &mut CommandBuffer) -> FlushOutcome {
        if cb.poisoned {
            panic!("ecs: buffer poisoned: call World::cancel before reusing this buffer");
        }
        cb.bind(self);
        cb.coalesce();

        // Free reservations whose spawn was coalesced away, and free any
        // reservation that no longer has an active spawn command.
        let mut active: SmallVec<[Entity; 8]> = SmallVec::new();
        for command in &cb.commands {
            if let Command::Spawn { entity, .. } = command {
                active.push(*entity);
            }
        }
        let mut stale: SmallVec<[Entity; 8]> = SmallVec::new();
        for entity in &cb.reservations {
            if !active.contains(entity) {
                stale.push(*entity);
            }
        }
        for entity in stale {
            if self.allocator.state(entity.index()) == SlotState::Reserved {
                self.purge_sparse(entity);
                self.allocator.free_and_bump(entity.index());
                self.locations[entity.index() as usize] = EntityLocation::INVALID;
            }
        }
        cb.reservations.retain(|entity| active.contains(entity));

        let mut commands = std::mem::take(&mut cb.commands);
        let mut drop_panics: DropPanics = Vec::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            for command in commands.drain(..) {
                apply_command(self, command, &mut drop_panics);
            }
        }));
        // Return the drained allocation so the buffer retains its capacity
        // across flushes (spec §3.10).
        cb.commands = commands;

        match result {
            Ok(()) => {
                cb.reservations.clear();
                cb.despawned.clear();
                #[cfg(debug_assertions)]
                crate::invariants::check_world(self);
                FlushOutcome { drop_panics }
            }
            Err(payload) => {
                cb.poisoned = true;
                for entity in cb.reservations.drain(..) {
                    if self.allocator.state(entity.index()) == SlotState::Reserved {
                        self.purge_sparse(entity);
                        self.allocator.free_and_bump(entity.index());
                        self.locations[entity.index() as usize] = EntityLocation::INVALID;
                    }
                }
                cb.despawned.clear();
                resume_unwind(payload);
            }
        }
    }

    /// Discards `cb`'s commands and releases its reserved slots.
    ///
    /// Idempotent. Resets the buffer to the same state as
    /// [`CommandBuffer::new`], including clearing the poisoned flag.
    ///
    /// Calling `cancel` with a different world than the one that recorded the
    /// spawns is unsupported; the reserved slots of the original world are not
    /// released.
    pub fn cancel(&mut self, cb: &mut CommandBuffer) {
        let same_world = cb.world_id == Some(self.id);
        let same_epoch = cb.clear_epoch == Some(self.clear_epoch);
        if same_world && same_epoch {
            for entity in cb.reservations.drain(..) {
                if self.allocator.state(entity.index()) == SlotState::Reserved {
                    self.purge_sparse(entity);
                    self.allocator.free_and_bump(entity.index());
                    self.locations[entity.index() as usize] = EntityLocation::INVALID;
                }
            }
        } else {
            // The world was cleared (reservations were already reclaimed) or
            // this is a different world (unsupported). Either way there is
            // nothing safe to free here.
            cb.reservations.clear();
        }
        cb.reset();
    }
}

fn apply_command(world: &mut World, command: Command, panics: &mut DropPanics) {
    match command {
        Command::Spawn { entity, ids, apply } => world.materialize_reserved(entity, &ids, apply),
        Command::Insert { apply, .. } => apply(world, panics),
        Command::Remove { apply, .. } => apply(world, panics),
        Command::Despawn { entity } => {
            world.despawn_inner(entity, Some(panics));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::Storage;

    struct A;

    #[test]
    fn flush_retains_command_capacity() {
        let mut world = World::new();
        world.register::<A>(Storage::Table);
        let mut cb = CommandBuffer::new();
        for _ in 0..64 {
            let _ = cb.spawn(&mut world, (A,));
        }
        let outcome = world.flush(&mut cb);
        assert!(outcome.drop_panics.is_empty());
        let retained = cb.commands.capacity();
        assert!(retained >= 64);

        let _ = cb.spawn(&mut world, (A,));
        world.flush(&mut cb);
        assert!(cb.commands.capacity() >= retained);
    }
}
