//! Columnar table storage.
//!
//! A table owns one column per table-stored component plus a row of entity
//! ids. The length of `entity_rows`, the length of every column, and the
//! table's logical length are always identical (spec §3.16).

use core::alloc::Layout;
use core::any::Any;
use core::ptr::{self, NonNull};

use crate::component::{ComponentId, Registry};
use crate::entity::Entity;
use crate::prelude::*;

fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// One stashed droppable value: byte offset into the scratch buffer plus its
/// destructor.
type ScratchItem = (usize, Option<unsafe fn(*mut u8)>);

/// A single typed column, type-erased to a byte pointer.
pub(crate) struct Column {
    ptr: NonNull<u8>,
    layout: Layout,
    drop_fn: Option<unsafe fn(*mut u8)>,
    cap: usize,
    len: usize,
    is_zst: bool,
}

// SAFETY: `register<T: Send>` is the only way to create a column, and a column
// is exclusively owned by its `World`. Moving the column to another thread
// therefore moves the components with it. The targeted `unsafe impl Send` lives
// in `thread_send.rs`.
impl Column {
    fn new(layout: Layout, drop_fn: Option<unsafe fn(*mut u8)>) -> Self {
        let is_zst = layout.size() == 0;
        let ptr = if is_zst {
            // SAFETY: a provenance-less, aligned, non-null address is a valid
            // dangling pointer for a zero-sized type. No memory is accessed.
            unsafe { NonNull::new_unchecked(ptr::without_provenance_mut(layout.align())) }
        } else {
            NonNull::dangling()
        };
        Self {
            ptr,
            layout,
            drop_fn,
            cap: 0,
            len: 0,
            is_zst,
        }
    }

    fn array_layout(&self, cap: usize) -> Layout {
        let size = self
            .layout
            .size()
            .checked_mul(cap)
            .expect("ecs: table row overflow: column byte length overflowed usize");
        Layout::from_size_align(size, self.layout.align())
            .expect("ecs: table row overflow: invalid column layout")
    }

    fn reserve(&mut self, needed: usize) {
        if needed <= self.cap {
            return;
        }
        if self.is_zst {
            self.cap = needed;
            return;
        }
        let new_layout = self.array_layout(needed);
        let ptr = if self.cap == 0 {
            // SAFETY: `new_layout` has a non-zero size because the column is
            // not zero-sized.
            unsafe { std::alloc::alloc(new_layout) }
        } else {
            let old_layout = self.array_layout(self.cap);
            // SAFETY: `self.ptr` was allocated with `old_layout` and has that
            // size; the new layout has the same alignment and a larger size.
            unsafe { std::alloc::realloc(self.ptr.as_ptr(), old_layout, new_layout.size()) }
        };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(new_layout);
        }
        // SAFETY: `alloc`/`realloc` returned a non-null pointer.
        self.ptr = unsafe { NonNull::new_unchecked(ptr) };
        self.cap = needed;
    }

    /// Pointer to the column element at `row`.
    pub(crate) fn ptr_at(&self, row: usize) -> *mut u8 {
        if self.is_zst {
            self.ptr.as_ptr()
        } else {
            let offset = row
                .checked_mul(self.layout.size())
                .expect("ecs: table row overflow: row byte offset overflowed usize");
            // SAFETY: `row < self.cap` is guaranteed by all callers (rows are
            // committed or reserved within the current capacity).
            unsafe { self.ptr.as_ptr().add(offset) }
        }
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    fn dealloc(&mut self) {
        if !self.is_zst && self.cap > 0 {
            let layout = self.array_layout(self.cap);
            // SAFETY: allocated by `reserve` with this layout.
            unsafe { std::alloc::dealloc(self.ptr.as_ptr(), layout) };
            self.cap = 0;
        }
    }
}

/// Columnar storage for one archetype.
pub(crate) struct Table {
    entity_rows: Vec<Entity>,
    columns: SmallVec<[(ComponentId, Column); 8]>,
    cap: usize,
}

impl Table {
    /// Creates an empty table with the given (sorted, duplicate-free) table
    /// component set.
    pub(crate) fn new(ids: &[ComponentId], registry: &Registry) -> Self {
        if ids.len() >= u16::MAX as usize {
            panic!(
                "ecs: table column overflow: archetype has {} table components (max {})",
                ids.len(),
                u16::MAX - 1
            );
        }
        let mut columns: SmallVec<[(ComponentId, Column); 8]> = SmallVec::new();
        for &id in ids {
            let info = registry.info(id);
            columns.push((id, Column::new(info.layout, info.drop_fn)));
        }
        Self {
            entity_rows: Vec::new(),
            columns,
            cap: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entity_rows.len()
    }

    pub(crate) fn entity_rows(&self) -> &[Entity] {
        &self.entity_rows
    }

    pub(crate) fn columns(&self) -> &[(ComponentId, Column)] {
        &self.columns
    }

    pub(crate) fn column_index(&self, id: ComponentId) -> Option<usize> {
        self.columns
            .binary_search_by_key(&id, |(existing, _)| *existing)
            .ok()
    }

    pub(crate) fn column(&self, id: ComponentId) -> Option<&Column> {
        self.column_index(id).map(|index| &self.columns[index].1)
    }

    pub(crate) fn column_mut(&mut self, id: ComponentId) -> Option<&mut Column> {
        match self.column_index(id) {
            Some(index) => Some(&mut self.columns[index].1),
            None => None,
        }
    }

    pub(crate) fn contains(&self, id: ComponentId) -> bool {
        self.column_index(id).is_some()
    }

    /// Reserves capacity for one more row without committing it.
    ///
    /// Returns the row index the new row will occupy. The caller must not
    /// expose the table to queries until [`Self::commit_row`] is called.
    pub(crate) fn begin_row(&mut self) -> u32 {
        let len = self.entity_rows.len();
        if len >= u32::MAX as usize {
            panic!(
                "ecs: table row overflow: table already has {} rows",
                u32::MAX
            );
        }
        if len == self.cap {
            let grown = (self.cap + self.cap / 2).max(4).max(len + 1);
            for (_, column) in &mut self.columns {
                column.reserve(grown);
            }
            self.cap = grown;
        }
        len as u32
    }

    /// Commits a row previously reserved with [`Self::begin_row`].
    pub(crate) fn commit_row(&mut self, entity: Entity, row: u32) {
        debug_assert_eq!(row as usize, self.entity_rows.len());
        self.entity_rows.push(entity);
        let len = self.entity_rows.len();
        for (_, column) in &mut self.columns {
            column.len = len;
        }
    }

    /// Removes `row` by moving the last row into its place.
    ///
    /// The removed row's bytes are **not** dropped: the caller must have moved
    /// every value out first (despawn or migration). Returns the entity that
    /// was moved, if any, so the caller can update its location.
    pub(crate) fn swap_remove_nodrop(&mut self, row: u32) -> Option<Entity> {
        let len = self.entity_rows.len();
        debug_assert!((row as usize) < len);
        let last = len - 1;
        let moved = if row as usize != last {
            for (_, column) in &self.columns {
                let size = column.layout().size();
                if size != 0 {
                    // SAFETY: both rows are within the committed range, and
                    // the column pointer is valid for `len` elements.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            column.ptr_at(last),
                            column.ptr_at(row as usize),
                            size,
                        );
                    }
                }
            }
            let moved = self.entity_rows[last];
            self.entity_rows[row as usize] = moved;
            Some(moved)
        } else {
            None
        };
        self.entity_rows.pop();
        let new_len = self.entity_rows.len();
        for (_, column) in &mut self.columns {
            column.len = new_len;
        }
        moved
    }

    /// Copies every droppable value in `row` into an owned scratch buffer.
    ///
    /// The row is logically moved out: after this call the caller must either
    /// overwrite the row (swap-remove/migration) or treat it as uninitialized.
    pub(crate) fn take_row(&self, row: u32) -> Scratch {
        let mut items: SmallVec<[ScratchItem; 8]> = SmallVec::new();
        let mut offset = 0usize;
        let mut align = 1usize;
        for (_, column) in &self.columns {
            if column.drop_fn.is_none() {
                continue;
            }
            let layout = column.layout();
            offset = align_up(offset, layout.align());
            align = align.max(layout.align());
            let size = layout.size();
            items.push((offset, column.drop_fn));
            // The actual byte copy is performed below, once the buffer layout
            // is known; record the source column index in a parallel pass.
            offset = offset
                .checked_add(size)
                .expect("ecs: table row overflow: scratch size overflowed");
        }

        let buf_layout = Layout::from_size_align(offset.max(1), align)
            .expect("ecs: flush invariant: bad scratch");
        // SAFETY: the layout has a non-zero size.
        let buf = unsafe { std::alloc::alloc(buf_layout) };
        if buf.is_null() {
            std::alloc::handle_alloc_error(buf_layout);
        }
        // SAFETY: the pointer was just allocated with `buf_layout`.
        let buf = unsafe { NonNull::new_unchecked(buf) };

        // Second pass: copy the droppable values.
        let mut item_index = 0usize;
        for (_, column) in &self.columns {
            if column.drop_fn.is_none() {
                continue;
            }
            let size = column.layout().size();
            if size != 0 {
                let offset = items[item_index].0;
                // SAFETY: `buf` is valid for `offset + size` bytes and the
                // source row is initialized. The regions cannot overlap
                // because the scratch buffer is freshly allocated.
                unsafe {
                    ptr::copy_nonoverlapping(
                        column.ptr_at(row as usize),
                        buf.as_ptr().add(offset),
                        size,
                    );
                }
            }
            item_index += 1;
        }

        Scratch {
            buf,
            layout: buf_layout,
            items,
        }
    }

    /// Drops all values and resets the table to length zero, retaining
    /// capacity. Destructor panics are caught and discarded so the world stays
    /// structurally valid (spec §18).
    pub(crate) fn clear(&mut self) {
        let len = self.entity_rows.len();
        self.entity_rows.clear();
        for (_, column) in &mut self.columns {
            column.len = 0;
        }
        for row in 0..len {
            for (_, column) in &self.columns {
                if let Some(drop_fn) = column.drop_fn {
                    let ptr = column.ptr_at(row);
                    // SAFETY: the value was initialized and is dropped exactly
                    // once here. AssertUnwindSafe is sound because a failing
                    // destructor cannot leave the table with dangling rows:
                    // the length is already zero.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                        drop_fn(ptr)
                    }));
                }
            }
        }
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn check(&self) {
        let len = self.entity_rows.len();
        assert!(
            self.cap >= len,
            "ecs: flush invariant: table capacity below length"
        );
        for (id, column) in &self.columns {
            assert_eq!(
                column.len, len,
                "ecs: flush invariant: column {id:?} length differs from table length"
            );
            assert!(
                column.cap >= len,
                "ecs: flush invariant: column {id:?} capacity below length"
            );
        }
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        let len = self.entity_rows.len();
        for (_, column) in &self.columns {
            if let Some(drop_fn) = column.drop_fn {
                for row in 0..len {
                    let ptr = column.ptr_at(row);
                    // SAFETY: every row below `len` is initialized, and this is
                    // the final use of the storage.
                    unsafe { drop_fn(ptr) };
                }
            }
        }
        for (_, column) in &mut self.columns {
            column.dealloc();
        }
    }
}

/// An owned copy of a row's droppable values.
///
/// Used by despawn and migration to move values out before mutating the table,
/// then drop them afterwards (with optional panic collection). The scratch
/// buffer is always allocated, even when empty, so [`Drop`] is uniform.
pub(crate) struct Scratch {
    buf: NonNull<u8>,
    layout: Layout,
    items: SmallVec<[ScratchItem; 8]>,
}

impl Scratch {
    /// Drops every stashed value.
    ///
    /// With `panics == None`, a panicking destructor propagates and the
    /// remaining values are leaked (the world is still structurally valid).
    /// With `panics == Some(sink)`, each destructor is isolated and its panic
    /// payload is collected instead of propagating.
    pub(crate) fn drop_items(&mut self, mut panics: Option<&mut Vec<Box<dyn Any + Send>>>) {
        for (offset, slot) in &mut self.items {
            let Some(drop_fn) = slot.take() else {
                continue;
            };
            // SAFETY: the pointer addresses an initialized value in the
            // scratch buffer.
            let ptr = unsafe { self.buf.as_ptr().add(*offset) };
            match panics.as_deref_mut() {
                Some(sink) => {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        // SAFETY: same as above.
                        unsafe { drop_fn(ptr) }
                    }));
                    if let Err(payload) = result {
                        sink.push(payload);
                    }
                }
                None => {
                    // SAFETY: same as above.
                    unsafe { drop_fn(ptr) }
                }
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.drop_items(None);
        }
        // SAFETY: allocated with `self.layout` in `Table::take_row`.
        unsafe { std::alloc::dealloc(self.buf.as_ptr(), self.layout) };
    }
}
