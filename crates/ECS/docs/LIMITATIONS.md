# Limitations — my_ecs 1.0.0 contract

These are the known, accepted limitations of the frozen specification
([`SPEC.md`](SPEC.md) §27). They are not bugs; each has a documented reason and
a possible future direction.

1. **A dropped un-flushed `CommandBuffer` leaks reserved slots.**
   The D1 buffer design intentionally holds no `World` reference, so `Drop`
   cannot release slots. The leak is bounded by the allocator's index space and
   is a resource-exhaustion bug, never UB. Mitigations: `#[must_use]`, a
   debug-only `Drop` assertion (`"ecs: command buffer leak:"`), and a
   debug-only reservation-count assertion in `spawn`. Release builds fail
   silently. Callers must `world.flush(&mut cb)` or `world.cancel(&mut cb)`.
   A future `World::reap_orphaned_reservations` (D2) or a hard cap can address
   this without re-opening the eager-spawn contract.

2. **No `PendingEntity`.** `CommandBuffer::spawn` is eager: it reserves a real
   slot and returns a real `Entity` immediately. Deferred spawning while a
   query guard is alive is not supported (the reservation needs `&mut World`).
   Deferred `insert`/`remove`/`despawn` *are* supported during iteration
   because they do not borrow the world.

3. **The `AllLiveEntities` anchor is O(alive).** Queries with no table term and
   no required sparse term — `(Entity)`, `(Entity, Option<&Health>)`,
   `(Entity, Without<Health>)` — scan every allocator slot. This is correct and
   predictable, but slower than an archetype walk.

4. **Iteration order is unspecified.** Do not rely on order across two query
   calls, even with the same plan. Table swap-remove, sparse-set swap-remove,
   and migration all perturb order without invalidating plans.

5. **No archetype/table GC.** Empty archetypes and their tables persist until
   the world is dropped. `shrink_archetypes()` is deferred; IDs are never
   reclaimed.

6. **No `no_std`.** The crate is structured for it — all collections go through
   `src/prelude.rs` and `std::collections` is banned from `src/` — but the swap
   to `alloc` is a post-1.0 change. `std::thread::panicking()` and
   `std::sync::atomic` are the two explicit `std` dependencies.

7. **No automatic sparse↔table migration.** Storage strategy is chosen once,
   at registration, and never changes.

8. **No batch operations.** `spawn_batch`, `insert_batch`, and friends are out
   of scope for 1.0. Spawning in a loop works but pays per-entity overhead.

9. **No parallel queries.** `World: !Sync` and all guards are single-threaded.
   Rayon splitting and a parallel scheduler are deferred.

10. **Registered components must be `Send`.** `register::<T>` requires
    `T: Send + 'static`. Non-`Send` component types are rejected at compile
    time. This is what makes `World: Send` sound without locks.

## Additional implementation notes

* The plan cache grows without bound (one entry per query type). This is
  documented in SPEC §3.13 and is bounded by the number of distinct query
  types used by the program.
* `ComponentId` uses `0` for the synthetic `Entity` term; real component ids
  start at `1`.
* Panic prefixes are frozen; tests assert prefixes only, never full messages.
* Destructor panics during migration/despawn are collected in
  `FlushOutcome::drop_panics` on the flush path and re-raised on immediate
  `World::insert`/`World::remove` paths where a value is returned (in practice
  the immediate paths do not drop the removed value). `World::clear` catches
  and discards destructor panics to keep the world structurally valid.
