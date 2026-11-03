# Changelog

All notable changes to `voxel-ecs`. The frozen contract lives in
[`SPEC.md`](SPEC.md).

## 1.0.0 — 2026-10-27 (frozen)

This release implements the frozen specification. Final-round additions:

* `std::thread::panicking()` guard in `CommandBuffer::drop`, so a buffer that
  is dropped during unwinding does not mask the original panic with a debug
  assertion.
* `World::cancel` is idempotent and resets the poisoned flag.
* Documented `#[must_use]` caveat on `CommandBuffer`.
* Debug-only reservation-count assertion in `CommandBuffer::spawn`.
* New frozen prefix `"ecs: command buffer leak:"`.

### Breaking changes since the 1.0 draft (pre-release; no compatibility owed)

1. `register<T>` requires `T: Send`.
2. `"ecs: unregistered component:"` → `"ecs: component not registered:"`.
3. `get_unchecked` on an unregistered `T` panics instead of being UB.
4. Iteration order is unspecified.
5. `CommandBuffer` poisons itself after a T0/T1 flush panic.
6. `CommandBuffer` gains cross-world binding and stale-epoch checks.
7. `World::clear()` bumps entity generations.
8. `Deref<Target = impl Iterator>` removed; use `.iter()`.
9. `get`/`get_mut` are restricted to the query's `AccessSet`.
10. Uniform `'q` item lifetimes for read-only and mutable iteration.
11. `Bundle: Send`.
12. `QueryMut` and `EntityMut` are explicitly `!Send`.

## 1.2 — rounds 7+

* D1 command-buffer lifecycle: eager reservation, no world reference in the
  buffer.
* `FlushOutcome { drop_panics }` collects destructor panics during flush.
* `'q` lifetime discipline for query items.
* Access-set restriction for random access.
* `!Send` markers for mutable guards.
* First-flush binding for insert-only buffers.
* Migration Phase 7 catch-and-expose.

## 1.1 — round 6

Twelve critical issues fixed:

* Generation bump on every transition into `Free`.
* `Send` guarantees anchored on `register<T: Send>`.
* Duplicate-component detection at query and bundle boundaries.
* Flush panic semantics and poisoning.
* Migration tiers T0–T3.
* Anchor modes and plan selection.
* API completion (`clear`, `cancel`, `entity_ref`/`entity_mut`).
* Unregistered-component matrix.
* Cross-world buffer binding.
* Iteration order documented as unspecified.
* Concrete storage representations for tables and sparse sets.

## 1.0 (draft) — round 5

Premature freeze; superseded by 1.1/1.2.

## 0.5 — round 4

* Option C command buffer, migration algorithm, `EntityRef` protocol,
  `World::clear()`, invariant harness.

## 0.4 — round 3

* Flush ordering, sparse invariants S1–S7, query inner loops, CI matrix.

## 0.3 — round 2

* Query surface, plan cache, borrow rules, panic prefixes.

## 0.2 — round 1

* Scope, entity model, registry, bundles, dead-entity policy.

## 0.1

* Initial blueprint.
