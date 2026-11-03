//! Entity identifiers and the generational slot allocator.

use core::fmt;

use crate::prelude::*;

/// A generational entity identifier.
///
/// An `Entity` is 8 bytes, `#[repr(C)]`, and opaque: it exposes no accessors
/// and can only be compared, hashed, or printed. Identifiers are produced by
/// [`World::spawn`](crate::World::spawn),
/// [`CommandBuffer::spawn`](crate::CommandBuffer::spawn), or a query that
/// yields `Entity`.
///
/// An `Entity` is only valid for the `World` that produced it. Using it with a
/// different world may access an unrelated entity; use `CommandBuffer`'s
/// cross-world check (which panics with `"ecs: cross-world buffer:"`) to catch
/// the common case.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl Entity {
    pub(crate) const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    pub(crate) const fn index(self) -> u32 {
        self.index
    }

    pub(crate) const fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}

/// The lifecycle state of an allocator slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotState {
    /// The slot is not in use and may be handed out again.
    Free,
    /// The slot is owned by an un-flushed `CommandBuffer` spawn.
    Reserved,
    /// The slot is live.
    Alive,
    /// The generation counter overflowed; the slot will never be reused.
    Retired,
}

/// Generational slot allocator.
///
/// `generations` is the source of truth for the generation of every slot. The
/// allocator never hands out a generation that is currently `Free`; freeing a
/// slot bumps its generation (or retires the slot) first.
pub(crate) struct EntityAllocator {
    generations: Vec<u32>,
    states: Vec<SlotState>,
    free_list: Vec<u32>,
    #[cfg(any(test, debug_assertions))]
    last_handed_out: Vec<u32>,
}

impl EntityAllocator {
    pub(crate) fn new() -> Self {
        Self {
            generations: Vec::new(),
            states: Vec::new(),
            free_list: Vec::new(),
            #[cfg(any(test, debug_assertions))]
            last_handed_out: Vec::new(),
        }
    }

    /// Number of allocator slots, alive or not.
    pub(crate) fn len(&self) -> usize {
        self.generations.len()
    }

    pub(crate) fn state(&self, index: u32) -> SlotState {
        self.states[index as usize]
    }

    #[allow(dead_code)]
    pub(crate) fn generation(&self, index: u32) -> u32 {
        self.generations[index as usize]
    }

    pub(crate) fn entity(&self, index: u32) -> Entity {
        Entity::new(index, self.generations[index as usize])
    }

    pub(crate) fn is_alive(&self, e: Entity) -> bool {
        let index = e.index as usize;
        index < self.generations.len()
            && self.states[index] == SlotState::Alive
            && self.generations[index] == e.generation
    }

    fn allocate(&mut self, state: SlotState) -> Entity {
        let index = match self.free_list.pop() {
            Some(index) => {
                debug_assert_eq!(self.states[index as usize], SlotState::Free);
                index
            }
            None => {
                let index = self.generations.len();
                if index >= u32::MAX as usize {
                    panic!(
                        "ecs: entity index exhausted: all {} entity indices are in use",
                        u32::MAX
                    );
                }
                self.generations.push(1);
                self.states.push(SlotState::Free);
                #[cfg(any(test, debug_assertions))]
                self.last_handed_out.push(1);
                index as u32
            }
        };
        self.states[index as usize] = state;
        let generation = self.generations[index as usize];
        #[cfg(any(test, debug_assertions))]
        {
            self.last_handed_out[index as usize] = generation;
        }
        Entity::new(index, generation)
    }

    /// Allocates a live slot.
    pub(crate) fn spawn(&mut self) -> Entity {
        self.allocate(SlotState::Alive)
    }

    /// Allocates a reserved slot. Reserved slots are dead for all access APIs
    /// until [`Self::materialize`] is called.
    pub(crate) fn reserve(&mut self) -> Entity {
        self.allocate(SlotState::Reserved)
    }

    /// Turns a reserved slot into a live one.
    pub(crate) fn materialize(&mut self, e: Entity) {
        let index = e.index as usize;
        debug_assert_eq!(self.states[index], SlotState::Reserved);
        debug_assert_eq!(self.generations[index], e.generation);
        self.states[index] = SlotState::Alive;
    }

    /// The only path into [`SlotState::Free`].
    ///
    /// Bumps the slot's generation. If the generation was `u32::MAX`, the slot
    /// becomes [`SlotState::Retired`] instead and is not pushed to the free
    /// list. Returns `true` if the slot became `Free`.
    pub(crate) fn free_and_bump(&mut self, index: u32) -> bool {
        let i = index as usize;
        if self.states[i] == SlotState::Retired {
            return false;
        }
        let generation = self.generations[i];
        if generation == u32::MAX {
            self.states[i] = SlotState::Retired;
            false
        } else {
            self.generations[i] = generation + 1;
            self.states[i] = SlotState::Free;
            self.free_list.push(index);
            true
        }
    }

    /// Releases every live or reserved slot. Used by `World::clear`.
    pub(crate) fn clear(&mut self) {
        for index in 0..self.generations.len() {
            if matches!(self.states[index], SlotState::Alive | SlotState::Reserved) {
                self.free_and_bump(index as u32);
            }
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn check(&self) {
        assert_eq!(self.generations.len(), self.states.len());
        assert_eq!(self.generations.len(), self.last_handed_out.len());
        for (index, state) in self.states.iter().enumerate() {
            assert!(
                !matches!(state, SlotState::Free) || self.generations[index] > 0,
                "ecs: flush invariant: free slot with generation 0"
            );
            assert!(
                self.generations[index] != 0,
                "ecs: flush invariant: generation 0 handed out"
            );
            match state {
                SlotState::Alive | SlotState::Reserved => {
                    assert_eq!(
                        self.last_handed_out[index], self.generations[index],
                        "ecs: flush invariant: live slot generation does not match its handout"
                    );
                }
                SlotState::Free => {
                    assert!(
                        self.last_handed_out[index] < self.generations[index],
                        "ecs: flush invariant: free slot was not generation-bumped"
                    );
                }
                SlotState::Retired => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_despawn_recycles_with_bumped_generation() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.spawn();
        assert!(alloc.is_alive(a));
        assert!(alloc.free_and_bump(a.index));
        let b = alloc.spawn();
        assert_eq!(a.index, b.index);
        assert_ne!(a.generation, b.generation);
        assert!(!alloc.is_alive(a));
        assert!(alloc.is_alive(b));
    }

    #[test]
    fn reserved_is_not_alive() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.reserve();
        assert!(!alloc.is_alive(e));
        assert_eq!(alloc.state(e.index), SlotState::Reserved);
        alloc.materialize(e);
        assert!(alloc.is_alive(e));
    }

    #[test]
    fn generation_overflow_retires_slot() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.spawn();
        alloc.generations[e.index as usize] = u32::MAX;
        assert!(!alloc.free_and_bump(e.index));
        assert_eq!(alloc.state(e.index), SlotState::Retired);
        // Retired slots are never handed out again.
        let other = alloc.spawn();
        assert_ne!(other.index, e.index);
    }
}
