# my_ecs

A compact entity component system implementing the frozen
[`docs/SPEC.md`](docs/SPEC.md) (also shipped as [`docs/ECS_SPEC.md`](docs/ECS_SPEC.md)).

* **Table + sparse-set storage.** Table components are columnar and define
  archetypes; sparse components never migrate entities.
* **Generational entities.** 8-byte `Entity { index, generation }`, reuse with
  generation bumps, retirement on overflow.
* **Explicit registration.** `World::register::<T>(Storage)` is the only way to
  make a component usable.
* **Borrow-checked queries.** `World::query::<(&A, &B)>()`,
  `World::query_mut::<(&mut A, Option<&mut B>)>()`, `With`/`Without`,
  `Entity` yields, `Query::get`, `QueryMut::get_mut`.
* **Deferred edits.** `CommandBuffer` with eager reservation, coalescing,
  poisoning, cancellation, and collected destructor panics.
* **Checked unsafe.** `World: Send + !Sync`, Miri-targeted unsafe blocks, an
  internal invariant checker, and a property-test fuzzer.

## Quick start

```rust
use my_ecs::{Storage, World};

struct Position(f32);
struct Velocity(f32);

let mut world = World::new();
world.register::<Position>(Storage::Table);
world.register::<Velocity>(Storage::Table);

let entity = world.spawn((Position(0.0), Velocity(1.0)));

world
    .query_mut::<(&mut Position, &Velocity)>()
    .for_each(|(position, velocity)| position.0 += velocity.0);

assert_eq!(world.get::<Position>(entity).unwrap().0, 1.0);
```

## Building and testing

```sh
cargo test --tests          # unit + integration tests
cargo test --doc            # doctests
cargo test --examples       # examples are wired into the test runner
cargo run --example basic
cargo run --example hybrid
cargo run --example deferred
cargo bench                 # informational; non-gating
```

The crate targets stable Rust (`rust-version = "1.85"`), `std` only. CI runs
Linux and macOS; Windows is best-effort and is the platform used during
development of this implementation.

## Repository layout

```
src/
  entity.rs       generational allocator + SlotState
  component.rs    registry, ComponentInfo, Bundle tuple impls
  archetype.rs    archetype graph and add/remove edges
  table.rs        columnar storage, swap-remove, scratch rows
  sparse.rs       SparseSet<T>, erased sparse sets, S1–S7
  location.rs     EntityLocation
  world.rs        World, spawn/despawn/insert/remove/clear/access
  query/          QuerySpec/Fetch, plans, iterators, guards
  borrow.rs       BorrowMap behind UnsafeCell
  views.rs        EntityRef / EntityMut
  cmd.rs          CommandBuffer, flush/cancel, D1 lifecycle
  thread_send.rs  the only unsafe impl Send blocks
  invariants.rs   debug/test invariant checkers + property test
  prelude.rs      alloc-facing collection aliases
docs/
  SPEC.md         frozen specification (canonical)
  DESIGN.md       implementation design and decisions
  LIMITATIONS.md  the ten documented limitations
  CHANGELOG.md    version history
tests/            spec §24 matrix
benches/          criterion workload table (§22)
examples/         basic, hybrid, deferred (§23)
```

## Semantics worth knowing

* Iteration order is **unspecified**. Table and sparse swap-remove perturb it.
* Components must be `Send + 'static`.
* `World` is `Send` but `!Sync`; query guards and views are `!Send`.
* A `CommandBuffer` has no `World` reference. Dropping it with un-flushed
  reservations leaks entity slots in release builds — always `flush` or
  `cancel`. See [`docs/LIMITATIONS.md`](docs/LIMITATIONS.md).

## License

MIT OR Apache-2.0.
