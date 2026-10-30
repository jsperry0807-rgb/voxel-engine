//! `my_ecs` — a compact entity component system with table and sparse-set storage.
//!
//! This crate is the reference implementation of the frozen specification in
//! [`docs/SPEC.md`](../docs/SPEC.md) (also shipped as `docs/ECS_SPEC.md`).
//! The public surface is deliberately small:
//!
//! * Register component types explicitly with [`World::register`].
//! * Spawn bundles (tuples) with [`World::spawn`].
//! * Query with [`World::query`] and [`World::query_mut`].
//! * Stage structural changes from inside a query with [`CommandBuffer`], and
//!   apply them with [`World::flush`].
//!
//! # Example
//!
//! ```
//! use my_ecs::{Storage, World};
//!
//! struct Position(f32);
//! struct Velocity(f32);
//!
//! let mut world = World::new();
//! world.register::<Position>(Storage::Table);
//! world.register::<Velocity>(Storage::Table);
//!
//! let e = world.spawn((Position(0.0), Velocity(1.0)));
//! for (pos, vel) in world.query::<(&Position, &Velocity)>().iter() {
//!     println!("{} {}", pos.0, vel.0);
//! }
//! assert!(world.despawn(e));
//! ```
//!
//! # Threading
//!
//! [`World`] is `Send` but not `Sync`. A `World` can be moved to another
//! thread, but component data cannot be accessed from two threads at once.
//! Query guards (`Query`, `QueryMut`) and entity views are deliberately `!Send`.
//!
//! # Panics
//!
//! Runtime misuse is reported with a stable prefix (`"ecs: …"`). Prefixes are
//! frozen for the `0.x` series; see `docs/SPEC.md` §19.2.
//!
//! # Unsafe
//!
//! The crate contains `unsafe` code for columnar storage and the query
//! iterators. Every block is justified with a `// SAFETY:` comment and the
//! crate is validated with Miri in CI. Safe APIs only expose the contracts
//! described in the specification.

#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// The query plumbing intentionally uses a handful of internal types in the
// signatures of hidden (sealed) trait methods. Users cannot implement these
// traits, and the types are not reachable through the public API.
#![allow(private_interfaces, private_bounds)]

mod archetype;
mod borrow;
mod cmd;
mod component;
mod entity;
mod invariants;
mod location;
#[doc(hidden)]
pub mod query;
mod sparse;
mod table;
mod thread_send;
mod views;
mod world;

pub(crate) mod prelude;

pub use cmd::{CommandBuffer, FlushOutcome};
pub use component::{Bundle, ComponentId, Storage};
pub use entity::Entity;
pub use query::{
    Query, QueryIter, QueryIterMut, QueryMut, QuerySpec, ReadOnlyQuery, With, Without,
};
pub use views::{EntityMut, EntityRef};
pub use world::World;
