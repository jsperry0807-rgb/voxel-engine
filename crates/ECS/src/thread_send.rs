//! Targeted `Send` implementations for type-erased storage.
//!
//! These are the crate's only `unsafe impl Send` blocks. The thread-safety
//! story is anchored on `World::register<T: Send>`: a storage cell can only
//! ever contain values of a type that was proven `Send`, and each cell is
//! exclusively owned by its `World`. Moving the world to another thread moves
//! the values with it.

use crate::sparse::SparseSetErased;
use crate::table::Column;

// SAFETY: a column's pointed-to data is exclusively owned by its `World` and
// was created through `register<T: Send>`. `drop_fn` is a plain function
// pointer with no captured state.
unsafe impl Send for Column {}

// SAFETY: the erased sparse set boxes a `SparseSet<T>` created through
// `register<T: Send>`, and is exclusively owned by its `World`.
unsafe impl Send for SparseSetErased {}
