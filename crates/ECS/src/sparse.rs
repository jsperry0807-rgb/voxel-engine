//! Sparse-set storage for components that never affect archetype identity.
//!
//! Invariants S1–S7 from spec §11.1 are maintained by every operation:
//!
//! * S1: `dense.len() == entities.len() == len`
//! * S2: `sparse[entities[i].index] == i`
//! * S3: non-sentinel sparse entries are valid dense indices that point back
//! * S4: dense entity indices are distinct
//! * S5: `sparse` is long enough for every dense entity; ordinary removal does
//!   not truncate it (only [`SparseSet::clear`] does)
//! * S6: no valid dense index equals [`SENTINEL`]
//! * S7: capacity never decreases except in `Drop`

use core::alloc::Layout;
use core::any::Any;
use core::marker::PhantomData;
use core::mem::size_of;
use core::ptr::{self, NonNull};

use crate::entity::Entity;
use crate::prelude::*;

/// Sentinel stored in the sparse array for "not present".
pub(crate) const SENTINEL: u32 = u32::MAX;

/// Sparse-set storage for one component type.
pub(crate) struct SparseSet<T> {
    sparse: Vec<u32>,
    entities: Vec<Entity>,
    dense: NonNull<T>,
    len: usize,
    cap: usize,
    _marker: PhantomData<T>,
}

// SAFETY: every value in the dense array is a `T`, and registration requires
// `T: Send`. Moving the set to another thread moves the values with it.
unsafe impl<T: Send> Send for SparseSet<T> {}

impl<T> SparseSet<T> {
    pub(crate) fn new() -> Self {
        Self {
            sparse: Vec::new(),
            entities: Vec::new(),
            dense: NonNull::dangling(),
            len: 0,
            cap: 0,
            _marker: PhantomData,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[allow(dead_code)]
    pub(crate) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn sparse_slice(&self) -> &[u32] {
        &self.sparse
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn capacity(&self) -> usize {
        self.cap
    }

    fn dense_layout(&self, cap: usize) -> Layout {
        let size = size_of::<T>()
            .checked_mul(cap)
            .expect("ecs: table row overflow: sparse dense byte length overflowed usize");
        Layout::from_size_align(size, core::mem::align_of::<T>())
            .expect("ecs: table row overflow: invalid sparse layout")
    }

    fn reserve_dense(&mut self, needed: usize) {
        if needed <= self.cap {
            return;
        }
        let new_cap = (self.cap + self.cap / 2).max(4).max(needed);
        if size_of::<T>() == 0 {
            self.cap = new_cap;
            return;
        }
        let new_layout = self.dense_layout(new_cap);
        let ptr = if self.cap == 0 {
            // SAFETY: the layout is non-zero because T is not a ZST.
            unsafe { std::alloc::alloc(new_layout) }
        } else {
            let old_layout = self.dense_layout(self.cap);
            // SAFETY: the current pointer was allocated with `old_layout` for
            // `self.cap` elements; the new layout has the same alignment and a
            // larger size.
            unsafe {
                std::alloc::realloc(
                    self.dense.as_ptr().cast::<u8>(),
                    old_layout,
                    new_layout.size(),
                )
            }
        };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(new_layout);
        }
        // SAFETY: the allocator returned a non-null pointer with `T` alignment.
        self.dense = unsafe { NonNull::new_unchecked(ptr.cast::<T>()) };
        self.cap = new_cap;
    }

    fn dense_ptr(&self, index: usize) -> *mut T {
        if size_of::<T>() == 0 {
            self.dense.as_ptr()
        } else {
            // SAFETY: `index < self.len` for all callers.
            unsafe { self.dense.as_ptr().add(index) }
        }
    }

    fn dense_index(&self, e: Entity) -> Option<usize> {
        let index = *self.sparse.get(e.index as usize)?;
        if index == SENTINEL {
            None
        } else {
            Some(index as usize)
        }
    }

    /// Inserts or replaces the value for `e`, returning the previous value.
    pub(crate) fn insert(&mut self, e: Entity, value: T) -> Option<T> {
        let entity_index = e.index as usize;
        if entity_index >= self.sparse.len() {
            self.sparse.resize(entity_index + 1, SENTINEL);
        }
        if let Some(existing) = self.dense_index(e) {
            let ptr = self.dense_ptr(existing);
            // SAFETY: the slot holds an initialized T.
            unsafe {
                let old = ptr::read(ptr);
                ptr::write(ptr, value);
                return Some(old);
            }
        }

        if self.len >= u32::MAX as usize {
            panic!("ecs: entity index exhausted: sparse set reached u32::MAX entries");
        }
        self.reserve_dense(self.len + 1);
        // Reserve the entity list up front so the insertion below cannot fail
        // with an allocation panic after the dense value was written.
        self.entities.reserve(1);
        // SAFETY: capacity for `self.len` elements exists and the slot is
        // uninitialized.
        unsafe { ptr::write(self.dense_ptr(self.len), value) };
        self.entities.push(e);
        self.sparse[entity_index] = self.len as u32;
        self.len += 1;
        None
    }

    #[allow(dead_code)]
    pub(crate) fn get(&self, e: Entity) -> Option<&T> {
        let index = self.dense_index(e)?;
        debug_assert!(index < self.len);
        // SAFETY: `dense_index` guarantees a valid initialized slot.
        Some(unsafe { &*self.dense_ptr(index) })
    }

    /// Returns a raw mutable pointer to the value for `e`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee exclusive access to the component value,
    /// either through `&mut self` or through the world's borrow map.
    pub(crate) unsafe fn get_ptr(&self, e: Entity) -> Option<*mut T> {
        let index = self.dense_index(e)?;
        debug_assert!(index < self.len);
        Some(self.dense_ptr(index))
    }

    /// Removes and returns the value for `e`.
    pub(crate) fn remove(&mut self, e: Entity) -> Option<T> {
        let index = self.dense_index(e)?;
        let entity_index = e.index as usize;
        let last = self.len - 1;
        // SAFETY: `index < self.len` and the slot holds an initialized T.
        let old = unsafe { ptr::read(self.dense_ptr(index)) };
        if index != last {
            let moved = self.entities[last];
            // SAFETY: both indices are within the dense array. `copy` handles
            // ZSTs and potential (impossible) overlap.
            unsafe {
                ptr::copy(self.dense_ptr(last), self.dense_ptr(index), 1);
            }
            self.entities[index] = moved;
            self.sparse[moved.index as usize] = index as u32;
        }
        self.entities.pop();
        self.sparse[entity_index] = SENTINEL;
        self.len = last;
        Some(old)
    }

    /// Returns whether `e` is present.
    #[allow(dead_code)]
    pub(crate) fn contains(&self, e: Entity) -> bool {
        self.dense_index(e).is_some()
    }

    /// Empties the set, dropping every value. Destructor panics are caught and
    /// discarded so a `World::clear` remains structurally valid.
    pub(crate) fn clear(&mut self) {
        let len = self.len;
        self.len = 0;
        self.entities.clear();
        self.sparse.clear();
        for index in 0..len {
            let ptr = self.dense_ptr(index);
            // SAFETY: every slot below `len` was initialized and is dropped
            // exactly once. The length is already zero, so an unwinding
            // destructor cannot cause a double drop.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                ptr::drop_in_place(ptr)
            }));
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn check(&self) {
        crate::invariants::check_sparse_set(self);
    }
}

impl<T> Drop for SparseSet<T> {
    fn drop(&mut self) {
        for index in 0..self.len {
            let ptr = self.dense_ptr(index);
            // SAFETY: every slot below `len` is initialized and this is the
            // final use of the allocation.
            unsafe { ptr::drop_in_place(ptr) };
        }
        if size_of::<T>() != 0 && self.cap > 0 {
            let layout = self.dense_layout(self.cap);
            // SAFETY: allocated in `reserve_dense` with this layout.
            unsafe { std::alloc::dealloc(self.dense.as_ptr().cast::<u8>(), layout) };
        }
    }
}

/// Object-safe view of a sparse set.
pub(crate) trait ErasedSparse {
    fn get_ptr(&self, e: Entity) -> Option<NonNull<u8>>;
    fn contains(&self, e: Entity) -> bool;
    fn len(&self) -> usize;
    fn entities(&self) -> &[Entity];
    fn remove_boxed(&mut self, e: Entity) -> Option<Box<dyn Any>>;
    fn clear(&mut self);
    #[allow(dead_code)]
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    #[cfg(any(test, debug_assertions))]
    fn check_erased(&self);
}

impl<T: Send + 'static> ErasedSparse for SparseSet<T> {
    fn get_ptr(&self, e: Entity) -> Option<NonNull<u8>> {
        // SAFETY: this is a raw accessor; callers uphold the world's borrow
        // discipline.
        let ptr = unsafe { SparseSet::get_ptr(self, e) }?;
        // SAFETY: `get_ptr` never returns a null pointer.
        Some(unsafe { NonNull::new_unchecked(ptr.cast::<u8>()) })
    }

    fn contains(&self, e: Entity) -> bool {
        self.dense_index(e).is_some()
    }

    fn len(&self) -> usize {
        self.len
    }

    fn entities(&self) -> &[Entity] {
        &self.entities
    }

    fn remove_boxed(&mut self, e: Entity) -> Option<Box<dyn Any>> {
        self.remove(e).map(|value| Box::new(value) as Box<dyn Any>)
    }

    fn clear(&mut self) {
        SparseSet::clear(self);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    #[cfg(any(test, debug_assertions))]
    fn check_erased(&self) {
        self.check();
    }
}

/// Type-erased sparse set stored by the world.
pub(crate) struct SparseSetErased {
    inner: Box<dyn ErasedSparse>,
}

impl SparseSetErased {
    pub(crate) fn new<T: Send + 'static>() -> Self {
        Self {
            inner: Box::new(SparseSet::<T>::new()),
        }
    }

    pub(crate) fn get_ptr(&self, e: Entity) -> Option<NonNull<u8>> {
        self.inner.get_ptr(e)
    }

    pub(crate) fn contains(&self, e: Entity) -> bool {
        self.inner.contains(e)
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn entities(&self) -> &[Entity] {
        self.inner.entities()
    }

    pub(crate) fn remove_boxed(&mut self, e: Entity) -> Option<Box<dyn Any>> {
        self.inner.remove_boxed(e)
    }

    pub(crate) fn clear(&mut self) {
        self.inner.clear();
    }

    #[allow(dead_code)]
    pub(crate) fn as_any(&self) -> &dyn Any {
        self.inner.as_any()
    }

    pub(crate) fn as_any_mut(&mut self) -> &mut dyn Any {
        self.inner.as_any_mut()
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn check(&self) {
        self.inner.check_erased();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::new(index, 1)
    }

    #[test]
    #[cfg_attr(miri, ignore = "property fuzzing is too slow under Miri")]
    fn s1_to_s7_hold_across_random_operations() {
        let mut set: SparseSet<u64> = SparseSet::new();
        let mut next = 0u32;
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut live: Vec<Entity> = Vec::new();

        for _ in 0..2_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if seed & 3 == 0 && !live.is_empty() {
                let index = (seed as usize) % live.len();
                let e = live.swap_remove(index);
                let removed = set.remove(e);
                assert!(removed.is_some());
                assert!(!set.contains(e));
            } else {
                let e = entity(next);
                next += 1;
                set.insert(e, u64::from(e.index()));
                live.push(e);
            }
            crate::invariants::check_sparse_set(&set);
        }

        set.clear();
        crate::invariants::check_sparse_set(&set);
        assert!(set.is_empty());
    }

    #[test]
    fn removal_does_not_truncate_sparse() {
        let mut set: SparseSet<u8> = SparseSet::new();
        let e = entity(1000);
        set.insert(e, 7);
        assert!(set.remove(e).is_some());
        assert!(set.sparse.len() > 1000);
        assert_eq!(set.sparse[1000], SENTINEL);
    }

    #[test]
    fn zst_entries_are_tracked() {
        #[derive(Debug, PartialEq)]
        struct Marker;
        let mut set: SparseSet<Marker> = SparseSet::new();
        let e = entity(3);
        assert!(set.insert(e, Marker).is_none());
        assert!(set.contains(e));
        assert_eq!(set.get(e), Some(&Marker));
        assert_eq!(set.remove(e), Some(Marker));
        assert!(!set.contains(e));
    }
}
