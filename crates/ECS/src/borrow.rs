//! Internal borrow tracking.
//!
//! The map lives behind an `UnsafeCell` inside [`World`](crate::World). That is
//! sound because `World: !Sync`: at most one thread can ever hold a `&World`,
//! and the `&mut World`-shaped APIs cannot run while a query guard exists.
//! Public APIs never expose interior mutability.

use crate::component::ComponentId;
use crate::prelude::*;

#[derive(Clone, Copy, Default)]
struct ComponentBorrow {
    readers: u32,
    writer: bool,
}

/// Tracks read/write borrows per component id.
///
/// All conflict checks are always-on, in debug and release builds. The usual
/// path is a single hash lookup and an `unlikely`-style branch.
pub(crate) struct BorrowMap {
    per_component: FxHashMap<ComponentId, ComponentBorrow>,
}

impl BorrowMap {
    pub(crate) fn new() -> Self {
        Self {
            per_component: FxHashMap::default(),
        }
    }

    fn get(&self, id: ComponentId) -> ComponentBorrow {
        self.per_component.get(&id).copied().unwrap_or_default()
    }

    /// Checks that a read borrow would be legal, without acquiring it.
    pub(crate) fn check_read(&self, id: ComponentId) {
        let state = self.get(id);
        if state.writer {
            panic!("ecs: borrow conflict: {id:?} is already borrowed mutably");
        }
    }

    /// Checks that a write borrow would be legal, without acquiring it.
    pub(crate) fn check_write(&self, id: ComponentId) {
        let state = self.get(id);
        if state.writer {
            panic!("ecs: borrow conflict: {id:?} is already borrowed mutably");
        }
        if state.readers != 0 {
            panic!("ecs: borrow conflict: {id:?} is already borrowed immutably");
        }
    }

    /// Acquires a set of read and write borrows atomically.
    ///
    /// The whole set is validated before anything is recorded, so a panic can
    /// never leave a partially-acquired set behind.
    pub(crate) fn acquire_many(&mut self, reads: &[ComponentId], writes: &[ComponentId]) {
        for &id in reads {
            let state = self.get(id);
            if state.writer {
                panic!("ecs: borrow conflict: {id:?} is already borrowed mutably");
            }
            if state.readers == u32::MAX {
                panic!("ecs: reader overflow: too many simultaneous readers of {id:?}");
            }
        }
        for (position, &id) in writes.iter().enumerate() {
            let state = self.get(id);
            if state.writer {
                panic!("ecs: borrow conflict: {id:?} is already borrowed mutably");
            }
            if state.readers != 0 {
                panic!("ecs: borrow conflict: {id:?} is already borrowed immutably");
            }
            if reads.contains(&id) {
                panic!("ecs: borrow conflict: {id:?} is requested for both reading and writing");
            }
            if writes[..position].contains(&id) {
                panic!("ecs: borrow conflict: {id:?} is requested for writing twice");
            }
        }

        for &id in reads {
            self.per_component.entry(id).or_default().readers += 1;
        }
        for &id in writes {
            self.per_component.entry(id).or_default().writer = true;
        }
    }

    /// Releases a set previously acquired with [`Self::acquire_many`].
    pub(crate) fn release_many(&mut self, reads: &[ComponentId], writes: &[ComponentId]) {
        for &id in reads {
            let state = self.per_component.entry(id).or_default();
            debug_assert!(
                state.readers > 0,
                "ecs: flush invariant: unbalanced read release for {id:?}"
            );
            state.readers = state.readers.saturating_sub(1);
        }
        for &id in writes {
            let state = self.per_component.entry(id).or_default();
            debug_assert!(
                state.writer,
                "ecs: flush invariant: unbalanced write release for {id:?}"
            );
            state.writer = false;
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn is_balanced(&self) -> bool {
        self.per_component
            .values()
            .all(|state| state.readers == 0 && !state.writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> ComponentId {
        ComponentId(index)
    }

    #[test]
    fn read_read_is_allowed() {
        let mut map = BorrowMap::new();
        map.acquire_many(&[id(1)], &[]);
        map.acquire_many(&[id(1)], &[]);
        map.release_many(&[id(1)], &[]);
        map.release_many(&[id(1)], &[]);
        assert!(map.is_balanced());
    }

    #[test]
    #[should_panic(expected = "ecs: borrow conflict:")]
    fn read_write_conflicts() {
        let mut map = BorrowMap::new();
        map.acquire_many(&[id(1)], &[]);
        map.acquire_many(&[], &[id(1)]);
    }

    #[test]
    #[should_panic(expected = "ecs: borrow conflict:")]
    fn write_read_conflicts() {
        let mut map = BorrowMap::new();
        map.acquire_many(&[], &[id(1)]);
        map.acquire_many(&[id(1)], &[]);
    }

    #[test]
    #[should_panic(expected = "ecs: borrow conflict:")]
    fn write_write_conflicts() {
        let mut map = BorrowMap::new();
        map.acquire_many(&[], &[id(1)]);
        map.acquire_many(&[], &[id(1)]);
    }

    #[test]
    fn failed_acquisition_records_nothing() {
        let mut map = BorrowMap::new();
        map.acquire_many(&[], &[id(2)]);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            map.acquire_many(&[id(1)], &[id(2)]);
        }));
        assert!(result.is_err());
        // id(1) was never acquired; id(2) is still exclusively held.
        map.acquire_many(&[], &[id(1)]);
        map.release_many(&[], &[id(1)]);
        map.release_many(&[], &[id(2)]);
        assert!(map.is_balanced());
    }
}
