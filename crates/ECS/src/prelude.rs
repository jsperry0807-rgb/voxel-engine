//! Alloc-facing collection aliases.
//!
//! Every collection used inside the crate is routed through this module. To
//! port the crate to `alloc`-only builds, this file is the only place that has
//! to change. `std::collections` is intentionally not used directly anywhere
//! in `src/`; see `tests/no_std_imports.rs`.

pub(crate) use std::boxed::Box;
pub(crate) use std::vec::Vec;

#[allow(unused_imports)]
pub(crate) use bitvec::prelude::BitVec;
pub(crate) use hashbrown::HashMap;
#[allow(unused_imports)]
pub(crate) use rustc_hash::FxHashMap;
#[allow(unused_imports)]
pub(crate) use smallvec::SmallVec;
