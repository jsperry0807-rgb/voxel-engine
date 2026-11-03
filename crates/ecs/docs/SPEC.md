# ECS Specification — v1.0.0 (FROZEN)

**Canonical location:** `docs/SPEC.md`. Versioned with the crate. Rustdoc links here for deep semantics.
**Status:** Frozen for `1.0.0`. Changes to frozen contracts require a new round.
**Companion files:** `docs/CHANGELOG.md`, `docs/LIMITATIONS.md`.

---

## §0 — Document control

| Field               | Value                                                         |
| ------------------- | ------------------------------------------------------------- |
| Spec version        | 1.0.0 (frozen)                                                |
| Crate version       | `0.x` unstable pre-release; `1.0.0` at freeze landing         |
| Threading           | `World: Send`, `World: !Sync`. Single-threaded API. No locks. |
| MSRV                | current stable − 2 releases                                   |
| Target              | stable `std` Rust; CI Linux + macOS; Windows best-effort      |
| Canonical spec      | `docs/SPEC.md` (this document)                                |
| Panic-prefix policy | Frozen for all of `0.x`; add-only until `1.0.0`               |

---

## §1 — Scope

### §1.1 — In scope (v1.0)

Multi-threaded-`World` (`Send`, `!Sync`), explicit `register::<T>(Storage)`, Table + SparseSet storage, Miri-clean query layer, `CommandBuffer`, tuple `Bundle` impls, `EntityRef` / `EntityMut`, per-`World` query-plan cache, `World::clear()`, stable panic prefixes, property-tested invariants, Miri in CI, benchmark harness, shipped examples.

### §1.2 — Out of scope (deferred; no API surface in v1.0)

`Changed<T>` / `Added<T>`, Rayon query splitting, parallel system scheduler, automatic sparse↔table migration, serialization, derive macros, per-archetype sparse presence, `no_std` (structured for it, not required), archetype/table GC (`shrink_archetypes`), batch ops (`spawn_batch`, `insert_batch`), `Box<dyn Bundle>`, `PendingEntity` (deferred to v1.2), `World::reap_orphaned_reservations` (see §27), Loom.

### §1.3 — Dependencies

`hashbrown` (`default-features = false, features = ["alloc"]`), `rustc-hash`, `smallvec`, `bitvec`. Dev-only: `bevy_ecs`, `hecs` (benchmark baselines, non-CI), `criterion` (benches).

### §1.4 — Target

`std` only. CI on Linux + macOS required; Windows best-effort.

### §1.5 — Known limitations

See §26. Summary: no `PendingEntity`, `AllLiveEntities` anchor is O(alive), iteration order unspecified, no archetype GC, no `no_std`, no auto migration, no batch ops, no parallel queries, `Send` components only, dropped un-flushed `CommandBuffer` leaks reservations.

---

## §2 — Terminology

| Term            | Definition                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------- |
| **Entity**      | Generational ID `{ index: u32, generation: u32 }`, `#[repr(C)]`, opaque, 8 bytes.           |
| **Component**   | `T: Send + 'static`, registered with a `ComponentId`.                                       |
| **Storage**     | `{ Table, SparseSet }`.                                                                     |
| **Archetype**   | In v1, identical to a `TableId` (set of table-stored component IDs).                        |
| **Table**       | Columnar storage for one archetype's table components.                                      |
| **Sparse set**  | `sparse: Vec<u32>` + dense components + parallel dense entity list.                         |
| **World**       | Owns allocator, registry, archetypes, tables, sparse sets, plan cache.                      |
| **Query plan**  | Cached per-`World`, per-`TypeId<Q>`, list of `ArchetypeMatch` + `AnchorMode` + `AccessSet`. |
| **SlotState**   | `{ Free, Reserved, Alive, Retired }`.                                                       |
| **WorldId**     | `u64`, monotonic per process, assigned at `World::new`.                                     |
| **clear_epoch** | `u64`, incremented on every `World::clear()`.                                               |

---

## §3 — Core invariants

1. **Entity uniqueness.** A live entity appears exactly once in the allocator and exactly once in its table row.
2. **Archetype ≡ TableId.** Archetype identity = `TableId` = set of table-stored component IDs. Sparse components **never** affect archetype identity.
3. **Sparse membership.** Derived solely from each sparse set's own sparse array. `EntityLocation` carries no sparse bit.
4. **Migration triggers.** Table-stored add/remove → archetype migration. Sparse add/remove → in-place mutation.
5. **Generation start.** `generation = 1` at first allocation. Despawn (and every Free-transition) increments. Overflow → `Retired`.
6. **Reserved transience.** `SlotState::Reserved` is set by `cb.spawn`, becomes `Alive` at flush (materialization), or `Free` if the spawn was coalesced-out. Never observable in a query.
7. **Borrow exclusivity.** No aliasing `&mut` to the same component column simultaneously.
8. **Borrow conflict panics.** In **both** debug and release.
9. **Flush ordering.** Record-ordered after coalescing.
10. **Buffer capacity.** `CommandBuffer` retains capacity across flushes.
11. **Flush is explicit.** Only `world.flush(&mut cb)` applies deferred changes. No API auto-flushes.
12. **Mid-iteration changes.** Only via command buffer; immediate changes require `&mut World` and type-exclude active query guards.
13. **Plan cache scope.** Per-`World`, keyed by `TypeId::of::<Q>()`, invalidated on new archetype or new registration, marked `Stale` on `World::clear()`. Unbounded growth documented.
14. **Send/Sync.** `World: Send` iff its storages are `Send` (guaranteed by `register<T: Send>`); `World: !Sync`. `Query`, `EntityRef`: `!Send` (hold `&World`). `QueryMut`, `EntityMut`: `!Send` (explicit `PhantomData<*const ()>`). `CommandBuffer: Send`.
15. **Iteration order.** **Unspecified** across any two query calls. Swap-remove and migration perturb order without plan invalidation.
16. **Table lockstep.** `entity_rows.len() == every column's len == table.len()` at all times.
17. **Reserved → Free timing.** At flush end (or `cancel`, or panic-cleanup in flush), single free-list push, **generation bumped**.
18. **Generation-bump-on-free.** Every transition into `Free` from `Alive`, `Reserved`, or through `clear()` increments the slot's generation, or retires the slot if generation is `u32::MAX`. No slot is ever `Free` with a generation that was handed out to user code. **`free_and_bump` is the only path into `Free`.**
19. **`BorrowMap` interior mutability.** Internal `UnsafeCell<BorrowMap>` sound under `World: !Sync`. Public API exposes no interior mutability.
20. **Anchor mode.** Each `QueryPlan` carries an `AnchorMode` (§13.4). No plan iterates without a well-defined anchor.
21. **Non-`PendingEntity` v1.0.** Only real `Entity` values exist. `cb.spawn` materializes a real reserved slot at record time. No placeholders, no post-flush resolution.
22. **Flush per-command atomicity.** Every applied command either fully applies or panics. `apply` never partially mutates and returns.
23. **Duplicate components.** Detected at construction (queries) or record time (bundles) — see §13.9.
24. **Cap meaning.** In this document, "cap" means **maximum count**, never maximum index. Index domain for a capped value is `[0, cap)`.
25. **`u32::MAX` never a valid entity index.** Reserved as sparse `SENTINEL`. All sparse arrays use it exclusively as a sentinel.

---

## §4 — Public API (complete surface)

```rust
// ────────── Traits ──────────
pub trait Bundle: Send { /* sealed */ }
pub trait QuerySpec {
    type Item<'q> where Self: 'q;
    /* private plan-building methods */
}
pub trait ReadOnlyQuery: QuerySpec {}

// ────────── Types ──────────
pub struct Entity { /* opaque, #[repr(C)], 8 bytes */ }
pub struct ComponentId(/* private */);
pub enum Storage { Table, SparseSet }
pub struct With<T>(PhantomData<T>);
pub struct Without<T>(PhantomData<T>);

pub struct World;
pub struct Query<'w, Q: QuerySpec>;
pub struct QueryIter<'w, 'q, Q: QuerySpec> where 'w: 'q;
pub struct QueryMut<'w, Q: QuerySpec> { _not_send: PhantomData<*const ()>, /* ... */ }
pub struct QueryIterMut<'w, 'q, Q: QuerySpec> where 'w: 'q;
pub struct EntityRef<'w>;
pub struct EntityMut<'w> { _not_send: PhantomData<*const ()>, /* ... */ }

#[must_use]
pub struct CommandBuffer;

pub struct FlushOutcome {
    /// Panic payloads from `Drop` impls that panicked during migration
    /// Phase 7, in attempt order. The world remains structurally valid.
    pub drop_panics: Vec<Box<dyn std::any::Any + Send>>,
}

// ────────── World ──────────
impl World {
    pub fn new() -> Self;

    // Registration
    pub fn register<T: Send + 'static>(&mut self, s: Storage) -> ComponentId;

    // Lifecycle
    pub fn spawn(&mut self, b: impl Bundle) -> Entity;
    pub fn despawn(&mut self, e: Entity) -> bool;
    pub fn insert<T: Send + 'static>(&mut self, e: Entity, v: T) -> Option<T>;
    pub fn remove<T: Send + 'static>(&mut self, e: Entity) -> Option<T>;

    // Command buffer
    pub fn flush(&mut self, cb: &mut CommandBuffer) -> FlushOutcome;
    pub fn cancel(&mut self, cb: &mut CommandBuffer);

    // Reset
    pub fn clear(&mut self);

    // Safe access
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T>;
    pub fn get_mut<T: Send + 'static>(&mut self, e: Entity) -> Option<&mut T>;

    // Unsafe access
    /// # Panics
    /// `"ecs: component not registered:"` if `T` was never registered.
    /// This check occurs before the caller's safety contract applies.
    ///
    /// # Safety
    /// Caller guarantees: `e` is `Alive`; `T` is present on `e`;
    /// no `&mut T` to the same component on `e` exists simultaneously.
    /// Violation is UB.
    pub unsafe fn get_unchecked<T: Send + 'static>(&self, e: Entity) -> &T;

    /// # Safety
    /// Same as `get_unchecked`, plus exclusive access to `T` on `e`.
    pub unsafe fn get_unchecked_mut<T: Send + 'static>(&mut self, e: Entity) -> &mut T;

    // Entity views
    pub fn entity_ref(&self, e: Entity) -> EntityRef<'_>;
    pub fn entity_mut(&mut self, e: Entity) -> EntityMut<'_>;

    // Queries
    pub fn query<Q: ReadOnlyQuery>(&self) -> Query<'_, Q>;
    pub fn query_mut<Q: QuerySpec>(&mut self) -> QueryMut<'_, Q>;
}

// ────────── Query (read-only) ──────────
impl<'w, Q: QuerySpec> Query<'w, Q> {
    pub fn iter<'q>(&'q self) -> QueryIter<'w, 'q, Q> where 'w: 'q;
    pub fn for_each<'q>(&'q self, f: impl FnMut(Q::Item<'q>)) where 'w: 'q;
    pub fn try_for_each<'q, R>(&'q self, f: impl FnMut(Q::Item<'q>) -> ControlFlow<R>) -> ControlFlow<R>
        where 'w: 'q;

    /// Panics `"ecs: query access violation:"` if `T ∉ Q::ReadSet`.
    /// Returns `None` for dead entities, filtered-out entities, or entities
    /// lacking `T`. Panics `"ecs: component not registered:"` if `T`
    /// was never registered.
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T>;

    pub fn contains(&self, e: Entity) -> bool;
}

// ────────── QueryMut ──────────
impl<'w, Q: QuerySpec> QueryMut<'w, Q> {
    pub fn iter_mut<'q>(&'q mut self) -> QueryIterMut<'w, 'q, Q> where 'w: 'q;
    pub fn for_each<'q>(&'q mut self, f: impl FnMut(Q::Item<'q>)) where 'w: 'q;
    pub fn try_for_each<'q, R>(&'q mut self, f: impl FnMut(Q::Item<'q>) -> ControlFlow<R>) -> ControlFlow<R>
        where 'w: 'q;

    /// Panics `"ecs: query access violation:"` if `T ∉ Q::ReadSet ∪ Q::WriteSet`.
    pub fn get<T: Send + 'static>(&self, e: Entity) -> Option<&T>;

    /// Panics `"ecs: query access violation:"` if `T ∉ Q::WriteSet`.
    pub fn get_mut<T: Send + 'static>(&mut self, e: Entity) -> Option<&mut T>;

    pub fn contains(&self, e: Entity) -> bool;
}

// ────────── CommandBuffer ──────────
impl CommandBuffer {
    pub fn new() -> Self;

    /// Eagerly reserves a slot in `world`; returns a real `Entity`.
    /// Borrows `&mut World` only for the duration of this call.
    /// Binds the buffer to `world` if not already bound.
    pub fn spawn(&mut self, world: &mut World, b: impl Bundle) -> Entity;

    pub fn insert<T: Send + 'static>(&mut self, e: Entity, v: T);
    pub fn remove<T: Send + 'static>(&mut self, e: Entity);
    pub fn despawn(&mut self, e: Entity);
}
```

**`#[must_use]`:** `Query`, `QueryMut`, `EntityRef`, `EntityMut`, `CommandBuffer`.

**Sealed:** `Bundle`, `QuerySpec`, `ReadOnlyQuery`.

**Internal only (not exposed):** `ArchetypeId`, `TableId`, `SlotState`, `AnchorMode`, `AccessSet`, `QueryPlan`, `BorrowMap`.

---

## §5 — Module layout

```
src/
  lib.rs              // re-exports; #![deny(missing_docs, unsafe_op_in_unsafe_fn)]
  prelude.rs          // alloc-facing aliases: Box, Vec, HashMap, SmallVec, BitVec
  thread_send.rs      // unsafe impl Send for Column, SparseSetErased
  entity.rs           // Entity, SlotState, EntityAllocator
  component.rs        // ComponentInfo, registry, Storage
  archetype.rs        // archetype graph, edges, two-phase migration
  table.rs            // columnar storage, swap-remove, growth
  sparse.rs           // SparseSet<T>, invariants S1–S7
  location.rs         // EntityLocation, locations Vec
  world.rs            // World, WorldId, clear_epoch, spawn/despawn/insert/remove/flush/cancel/clear
  query/
    mod.rs            // Query, QueryMut, guards
    plan.rs           // QueryPlan, ArchetypeMatch, AnchorMode, AccessSet, cache
    iter.rs           // QueryIter, QueryIterMut; anchor-specific loops
    fetch.rs          // &T, &mut T, Entity, Option<&…>, With, Without
  borrow.rs           // BorrowMap (UnsafeCell), acquire/release, reader overflow
  views.rs            // EntityRef, EntityMut
  cmd.rs              // CommandBuffer, coalescing, flush, cancel, drop
  invariants.rs       // #[cfg(any(test, debug_assertions))] check_* functions
```

**Forbidden in `src/`:** `std::collections::*`, `std::thread::*`, `std::sync::*` (except `AtomicUsize` behind `#[cfg(feature = "std")]`). Enforced by `clippy::disallowed_types` + `tests/no_std_imports.rs`.

**`alloc`-readiness:** all collections routed through `crate::prelude`.

---

## §6 — Entity allocator

```rust
pub struct Entity { index: u32, generation: u32 }   // #[repr(C)], opaque

pub(crate) enum SlotState { Free, Reserved, Alive, Retired }

pub(crate) struct EntityAllocator {
    generations: Vec<u32>,        // 1-based
    states: Vec<SlotState>,
    free_list: Vec<u32>,          // LIFO
    #[cfg(debug_assertions)]
    last_handed_out: Vec<u32>,    // for invariant checks (§19)
}
```

**Operations:**

- `spawn()` — pop from free list if non-empty (`Free` → `Alive`, generation unchanged); else push a new index (`generation = 1`). Panics `"ecs: entity index exhausted:"` if `u32::MAX` entities exist.
- `reserve()` — same, but `Free` → `Reserved`.
- `materialize(e)` — `Reserved` → `Alive`.
- `free_and_bump(index)` — the **only** path into `Free`. Increments generation; if generation was `u32::MAX`, transitions to `Retired` instead. Push index to `free_list` if `Free`.
- `is_alive(e)` — `index < generations.len() ∧ states[index] == Alive ∧ generations[index] == e.generation`.

**`Reserved` treatment:** `is_alive(reserved) == false`. All access APIs treat `Reserved` as dead.

---

## §7 — Component registry

```rust
pub(crate) struct ComponentInfo {
    id: ComponentId,
    type_id: TypeId,
    name: &'static str,
    layout: Layout,
    drop_fn: Option<unsafe fn(*mut u8)>,
    storage: Storage,
    is_zst: bool,
}
```

- `register::<T: Send + 'static>(s)`:
  - Unregistered → assigns next sequential `ComponentId`, records metadata, returns it.
  - Registered, same `s` → idempotent, returns existing.
  - Registered, different `s` → panic `"ecs: storage mismatch:"`.
- `TypeId → ComponentId` map: `HashMap<TypeId, ComponentId>`.
- **Table ZSTs:** zero-byte column, pointer via `NonNull::<T>::dangling().cast::<u8>()` (correctly aligned for any over-aligned ZST). Presence = archetype membership. `drop_fn` called once per row on removal.
- **Sparse ZSTs:** sparse array maintained; dense is zero-sized; `drop_fn` called once per dense entry.
- **Drop order:** unspecified, documented. `Drop` impls must not panic (or if they do, see §9.2 Tier 2).

---

## §8 — Bundles

- Tuple impls only: `()`, `(A,)`, `(A, B)`, …, up to 16-tuple arity.
- `Bundle: Send` supertrait.
- Duplicate components → runtime panic (§13.9).
- Unregistered component → panic `"ecs: component not registered:"` (at `spawn`/`insert`/`cb.spawn`, where world access is available).
- Future `Box<dyn Bundle>` must detect duplicates; not in v1.

---

## §9 — Archetype graph and migration

### §9.1 — Archetype

```rust
pub(crate) struct Archetype {
    table_components: SmallVec<[ComponentId; 8]>,   // sorted
    table_id: TableId,
    add_edges:    HashMap<ComponentId, ArchetypeId>,
    remove_edges: HashMap<ComponentId, ArchetypeId>,
}
```

- Archetype ID ≡ Table ID (1:1) in v1.
- **Sparse components never affect archetype identity.**
- Archetypes store only table component sets; **no sparse presence.**
- Empty archetypes persist; no ID reclamation.

### §9.2 — Migration: two-phase commit

**Phase order:**

```
PHASE 0: Validate          — bundle dup, unregistered, e alive
PHASE 1: Pre-commit prep   — target_table.reserve(1); scratch buffer alloc
PHASE 2: Move shared comps — source_row → target_row (memcpy)
PHASE 3: Extract removed   — source_row → scratch (move out, no drop yet)
PHASE 4: Add new comps     — from bundle into target
PHASE 5: Commit location   — locations[e] = (target_arch, target_row); target.len += 1
PHASE 6: Source swap-remove — moved last row into source_row; update moved e.loc; source.len -= 1
PHASE 7: Drop scratch      — per-item catch_unwind; payloads → FlushOutcome.drop_panics
```

### §9.3 — Failure tiers

| Tier   | Trigger                    | World state                                    | Entity location | Removed component                                                                               |
| ------ | -------------------------- | ---------------------------------------------- | --------------- | ----------------------------------------------------------------------------------------------- |
| **T0** | Validation (Phase 0)       | Unchanged                                      | Unchanged       | Unchanged                                                                                       |
| **T1** | Allocation (Phase 1)       | Source intact; target uninitialized row leaked | Unchanged       | Unchanged                                                                                       |
| **T2** | Destructor panic (Phase 7) | Structurally valid                             | Target          | Memory leaked; payload in `FlushOutcome.drop_panics` (flush path) or re-raised (immediate path) |
| **T3** | Panic-during-panic         | Process aborts                                 | n/a             | n/a                                                                                             |

### §9.4 — Contract

> Migration is **structurally atomic** per command: no query observes a half-migrated entity. `Drop` panics during Phase 7 are caught; the world remains valid, the entity is fully in `target_arch`, and the panic payload is **collected** in `FlushOutcome.drop_panics` (deferred path) or **re-raised** (immediate path — `world.insert`, `world.remove`). The leaked component's value is lost. `Drop`-that-panics-while-unwinding aborts (Tier 3).

**Path distinction:** `world.insert` / `world.remove` (immediate, `&mut World`) re-raise Phase 7 panics — no `FlushOutcome` exists. `world.flush` collects them.

---

## §10 — Table storage

```rust
pub(crate) struct Table {
    entity_rows: Vec<Entity>,
    columns: SmallVec<[(ComponentId, Column); 8]>,   // sorted by ComponentId
    capacity: usize,                                  // lockstep across all columns
}

pub(crate) struct Column {
    ptr: NonNull<u8>,
    layout: Layout,
    drop_fn: Option<unsafe fn(*mut u8)>,
    len: usize,
}
```

- **Growth:** 1.5× amortized. Initial capacity 0. `new_cap = max(4, cap * 3 / 2)`.
- **Lockstep invariant:** `entity_rows.len() == column.len() == table.len()` at all times.
- **`swap_remove(row)`:**
  1. Return/drop component at `row`.
  2. Move row `len - 1` components into `row`.
  3. `entity_rows[row] = entity_rows[len - 1]`.
  4. `world.locations[moved_entity.index].row = row`.
  5. `len -= 1`.
- **ZST columns:** pointer from `NonNull::<T>::dangling().cast::<u8>()`, `size == 0`. `drop_fn` called per row on removal.
- **Overflow:** panics `"ecs: table row overflow:"` if `len == u32::MAX`; `"ecs: table column overflow:"` if `columns.len() == u16::MAX - 1`.

---

## §10b — `Send` implementation

**Registration bound:** `register<T: Send + 'static>`. This is the crate's **only** thread-safety guarantee mechanism for storage.

**`thread_send.rs`:**

```rust
// SAFETY: Registration enforces T: Send. Column's pointed-to data is exclusively
// owned by World; moving Column to another thread moves the components with it.
// drop_fn is a plain fn pointer with no captured state.
unsafe impl Send for Column {}

// SAFETY: same as Column.
unsafe impl Send for SparseSetErased {}
```

**Auto-trait propagation:**

- `Table`, `SparseSet`, `Archetype`, `World` derive `Send` from `Send` fields (`Vec`, `HashMap`, `SmallVec`, `BitVec`, `UnsafeCell<BorrowMap>` — all `Send` when contents are).
- `World: !Sync` because `UnsafeCell<BorrowMap>` is `!Sync`.
- `Query`, `EntityRef`: `!Send` automatically (hold `&World`; `&World: Send` iff `World: Sync`, false).
- `QueryMut`, `EntityMut`: `!Send` via explicit `PhantomData<*const ()>` (because `&mut World: Send` when `World: Send`).
- `CommandBuffer: Send` — requires `Command::Insert/Remove` store `Box<dyn Any + Send>`; the `+ Send` is zero-cost because `register<T: Send>` already enforces it.

**No blanket `unsafe impl Send for World`.** Auto traits do the work.

---

## §11 — Sparse set

```rust
pub(crate) struct SparseSet<T> {
    sparse: Vec<u32>,        // indexed by Entity.index; SENTINEL or dense index
    dense:  Vec<T>,
    entities: Vec<Entity>,   // parallel to dense
    _cap: usize,
}

const SENTINEL: u32 = u32::MAX;
```

### §11.1 — Invariants

| #      | Invariant                                                                                                                                                              |
| ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **S1** | `dense.len() == entities.len() == len`                                                                                                                                 |
| **S2** | `∀ i < len: sparse[entities[i].index] == i`                                                                                                                            |
| **S3** | `∀ j < sparse.len() with sparse[j] != SENTINEL: sparse[j] < dense.len() ∧ entities[sparse[j]].index == j`                                                              |
| **S4** | All `entities[i].index` are distinct                                                                                                                                   |
| **S5** | `if len > 0: sparse.len() > max(entities[i].index for i < len)`. When `len == 0`: `sparse` is not truncated by ordinary removal; only `clear()` truncates to length 0. |
| **S6** | No valid dense index equals `SENTINEL`                                                                                                                                 |
| **S7** | `capacity` never decreases except in `Drop`                                                                                                                            |

### §11.2 — Operations

- **`insert(e, v)`** — Pre: `e.index < u32::MAX`; `sparse` does not contain `e`. Post: `sparse[e.index] == len_before`; `entities[len_before] == e`; `dense[len_before] == v`; S1–S7 hold.
- **`get(e) -> Option<&T>`** — no mutation; `Some(&dense[sparse[e.index]])` iff in range and not `SENTINEL`; `None` otherwise.
- **`remove(e) -> Option<T>`** — absent → `None`. Present → `swap_remove_at(sparse[e.index])`; `sparse[e.index] = SENTINEL`; return `Some(old)`.
- **`swap_remove_at(i)`** — move last dense element into slot `i`; update `sparse[entities[i].index] = i`; pop last; S1–S7 hold.
- **`clear()`** — `len = 0`; `sparse` and `entities` cleared; capacity retained.

**Overflow:** panics if `dense.len() == u32::MAX` (implied by entity cap; defensive).

---

## §12 — Entity location

```rust
pub(crate) struct EntityLocation {
    generation: u32,
    archetype_id: ArchetypeId,   // ≡ TableId
    row: u32,
}
```

Stored in `locations: Vec<EntityLocation>` indexed by entity index. Updated on spawn, despawn (invalidate), migration, swap-remove. No sparse-presence bit.

---

## §13 — Query planner

### §13.1 — Supported forms

- Yields: `&T`, `&mut T`, `Entity`, `Option<&T>`, `Option<&mut T>`
- Filters (zero-yield): `With<T>`, `Without<T>`
- `Entity` yieldable; appears at most once per query (compile-time bound + runtime duplicate check).

### §13.2 — Construction

- `World::query::<Q>(&self)` — `Q: ReadOnlyQuery` (no `&mut T`).
- `World::query_mut::<Q>(&mut self)` — allows `&mut T`.

Both acquire borrows at construction (§14); panic before iteration on conflict.

### §13.3 — Plan state machine

```
Uninitialized ──(first query)──► Building ──(build complete)──► Ready {plan}
                                                                    │
                                                                    │ new archetype ∨
                                                                    │ new registration ∨
                                                                    │ clear() → Stale
                                                                    ▼
                                                                 Stale ──(next query)──► Building
```

### §13.4 — Plan shape

```rust
pub(crate) struct QueryPlan {
    anchor: AnchorMode,
    matches: SmallVec<ArchetypeMatch>,
    duplicate_check: SmallVec<ComponentId>,   // sorted
    access: AccessSet,
}

pub(crate) enum AnchorMode {
    TableArchetypes,
    SmallestSparseSet { set: ComponentId },
    AllLiveEntities,
}

pub(crate) struct ArchetypeMatch {
    archetype_id: ArchetypeId,
    table_id: TableId,
    table_columns: SmallVec<[u16; 4]>,          // precomputed column indices
    sparse_components: SmallVec<[ComponentId; 4]>,
    with_sparse: SmallVec<[ComponentId; 4]>,
    without_sparse: SmallVec<[ComponentId; 4]>,
    skip: bool,
}

pub(crate) struct AccessSet {
    pub read:  SmallVec<[ComponentId; 4]>,      // includes With/Without sparse-only
    pub write: SmallVec<[ComponentId; 4]>,
}
```

`QueryPlan` holds `ArchetypeId` and `TableId` only, never `&Table`.

### §13.5 — Anchor selection

1. Query has any table component → `TableArchetypes`.
2. Else query has any **required** sparse `&T` / `&mut T` → `SmallestSparseSet { set: smallest }`.
3. Else → `AllLiveEntities`. Covers `(Entity)`, `(Entity, Option<&Health>)`, `(Entity, Without<Health>)`, etc.

### §13.6 — Inner-loop layouts

**Pure-table** (`TableArchetypes`):

```
for m in plan.matches:
    if m.skip: continue
    let table = &world.tables[m.table_id]
    for row in 0..table.len():
        yield row
```

**Smallest-sparse** (`SmallestSparseSet { set }`):

```
for e in world.sparse_set(set).entities_iter():
    let Some(v0) = world.sparse.get::<A>(e) else { continue };   // other required sparse
    let opt = world.sparse.get::<C>(e);
    apply per-row presence filters (With/Without for remaining sparse)
    yield (e, v0, opt)
```

**All-live** (`AllLiveEntities`):

```
for (idx, slot) in world.allocator.alive_iter():
    if state(slot) != Alive: continue
    let e = Entity { index: idx, generation: generations[idx] }
    apply per-row presence filters
    yield e
```

### §13.7 — Item lifetimes (soundness)

```rust
impl<'w, 'q, Q: QuerySpec> Iterator for QueryIterMut<'w, 'q, Q> {
    type Item = Q::Item<'q>;
    fn next(&mut self) -> Option<Q::Item<'q>>;
}
```

`QueryIterMut` is created by `iter_mut(&'q mut self)`. During `'q`, `query` is exclusively borrowed. `Q::Item<'q>` aliases nothing outside the guard's borrow set.

Rejection case (compiler-enforced):

```rust
let mut q = world.query_mut::<(&mut A,)>();
let first = q.iter_mut().next().unwrap();   // Q::Item<'q>
let second = q.get_mut::<A>(e);             // requires &mut q; conflicts with 'q
```

**`Query::iter` uses the same `'q`** for consistency, even though `'w` would be sound for read-only (uniform lifetimes prevent accidental constructor-swap divergence).

### §13.8 — Row-skip vs archetype-skip

| Filter                     | Where filtered             |
| -------------------------- | -------------------------- |
| `With<T>`, `T` table       | plan build (`skip = true`) |
| `Without<T>`, `T` table    | plan build (`skip = true`) |
| `With<T>`, `T` sparse      | per row                    |
| `Without<T>`, `T` sparse   | per row                    |
| `&T` mandatory, `T` sparse | per row (skip if absent)   |
| `Option<&T>`, `T` sparse   | never (yields `None`)      |

### §13.9 — Duplicate-component detection

**Query construction (order):**

1. Collect `ComponentId`s from `Q`'s fetch set. `Entity` treated as a reserved slot `ComponentId::ENTITY`.
2. Sort; adjacent duplicate → panic `"ecs: duplicate component in query:"`.
3. Borrow-conflict check → `"ecs: borrow conflict:"`.
4. `AccessSet` populated.

**Bundle operations (record-time or immediate, world available):**

1. Collect `ComponentId`s from tuple; sort; adjacent duplicate → panic `"ecs: duplicate component in bundle:"`.

**Cost:** O(n log n) for `n ≤ 16`. Negligible.

### §13.10 — Random access (`get`/`get_mut`)

`Query::get::<T>` / `QueryMut::get::<T>` / `QueryMut::get_mut::<T>` restricted to `Q`'s `AccessSet`:

| Method                   | Allowed `T`                    | Panic on violation               |
| ------------------------ | ------------------------------ | -------------------------------- |
| `Query::get::<T>`        | `T ∈ Q::ReadSet`               | `"ecs: query access violation:"` |
| `QueryMut::get::<T>`     | `T ∈ Q::ReadSet ∪ Q::WriteSet` | `"ecs: query access violation:"` |
| `QueryMut::get_mut::<T>` | `T ∈ Q::WriteSet`              | `"ecs: query access violation:"` |

Return `None` for dead entity, filtered-out entity, or entity lacking `T`. Panic `"ecs: component not registered:"` if `T` unregistered (checked first). Does **not** acquire additional borrows (already held by the guard).

---

## §14 — Borrow tracking

```rust
pub(crate) struct BorrowMap {
    per_component: HashMap<ComponentId, ComponentBorrow>,
}
struct ComponentBorrow { readers: u32, writer: bool }
```

- **Always-on in release**, single `unlikely()` branch fast path. Panics in both debug and release. Prefix `"ecs: borrow conflict:"`.
- Reader-count overflow → panic `"ecs: reader overflow:"` (unreachable in practice; documented).

### §14.1 — Rules table

| Operation                             | Acquires                 | Conflict if                   |
| ------------------------------------- | ------------------------ | ----------------------------- |
| `query::<(&A, &B)>()`                 | R(A), R(B)               | W(A) ∨ W(B)                   |
| `query_mut::<(&A, &mut B)>()`         | R(A), W(B)               | W(A) ∨ R(B) ∨ W(B)            |
| `query_mut::<(Entity, &A, &mut B)>()` | R(A), W(B)               | W(A) ∨ R(B) ∨ W(B)            |
| `query::<(&A, Option<&B>)>()`         | R(A), R(B)               | W(A) ∨ W(B)                   |
| `query_mut::<(&A, Option<&mut B>)>()` | R(A), W(B)               | W(A) ∨ R(B) ∨ W(B)            |
| `query::<(&A, With<B>)>()`            | R(A)                     | W(A)                          |
| `query::<(&A, Without<B>)>()`         | R(A)                     | W(A)                          |
| `query::<(Entity, &A)>()`             | R(A)                     | W(A)                          |
| `entity_ref(e)`                       | R(all components on `e`) | any W on `e`'s components     |
| `entity_mut(e)`                       | W(all components on `e`) | any R ∨ W on `e`'s components |
| `get::<A>(e)`                         | R(A), short-lived        | W(A)                          |
| `get_mut::<A>(e)`                     | W(A)                     | R(A) ∨ W(A)                   |
| `insert` / `remove`                   | `&mut World`             | type-excluded                 |
| `flush` / `cancel` / `clear`          | `&mut World`             | type-excluded                 |

**Nested queries** allowed iff no write conflict. Debug `assert!` that releases are balanced.

**`BorrowMap` internal `UnsafeCell`:** sound under `World: !Sync`; documented in §3.19.

---

## §15 — `EntityRef` / `EntityMut`

```rust
pub struct EntityRef<'w> {
    world: &'w World,
    entity: Entity,
    holds: SmallVec<[ComponentId; 8]>,
}
pub struct EntityMut<'w> {
    _not_send: PhantomData<*const ()>,
    world: &'w mut World,
    entity: Entity,
    holds: SmallVec<[ComponentId; 8]>,
}
```

### §15.1 — Acquisition

```
fn entity_ref(&World, e) -> EntityRef:
    assert is_alive(e)                          // panic "ecs: dead entity:"
    loc = world.locations[e.index]
    table = &world.tables[loc.archetype_id]
    let mut holds = SmallVec::new()
    // Deterministic order: table columns in ComponentId order, then sparse in order.
    for c in table.component_ids():             // sorted
        world.borrows.acquire_read(c)           // panic "ecs: borrow conflict:"
        holds.push(c)
    for c in world.sparse_component_ids():      // sorted
        if world.sparse_set(c).contains(e):
            world.borrows.acquire_read(c)
            holds.push(c)
    EntityRef { world, entity: e, holds }
```

**`EntityMut`** acquires writes in the same order.

**Panic safety:** if acquisition panics mid-way, already-acquired borrows are released via `ReleaseOnDrop` guard.

### §15.2 — Access

- `EntityRef::get::<T>() -> Option<&T>` (checks `holds`)
- `EntityRef::entity() -> Entity`
- `EntityMut::get::<T>() -> Option<&T>`
- `EntityMut::get_mut::<T>(&mut self) -> Option<&mut T>` — `&mut self` because a second `get_mut::<T>` would alias

### §15.3 — Release

`Drop`: `for c in holds.iter().rev() { world.borrows.release(c); }`.

### §15.4 — Reserved entities

`entity_ref(reserved)` / `entity_mut(reserved)` panic `"ecs: dead entity:"`.

---

## §16 — Iteration order

**Unspecified.** Users must not rely on ordering across any two query calls, even with the same plan lifetime. Table swap-remove, sparse-set swap-remove, and archetype migration all perturb order without invalidating plans. Tests must not depend on order across structural mutations.

---

## §17 — Command buffer and flush

### §17.1 — Structure (D1)

```rust
#[must_use]
pub(crate) struct CommandBuffer {
    world_id:     Option<WorldId>,
    clear_epoch:  Option<u64>,
    commands:     Vec<Command>,
    reservations: SmallVec<[Entity; 8]>,
    poisoned:     bool,
}

pub(crate) struct World {
    id: WorldId,             // u64, monotonic per process
    clear_epoch: u64,        // incremented on clear()
    // ...
    #[cfg(debug_assertions)]
    _debug_reservations: (),  // see §19
}
```

### §17.2 — Binding

`world_id` and `clear_epoch` bind at the **first interaction** with a `World` — either `cb.spawn(&mut world, ..)` or the first `flush`/`cancel`. Once bound, all subsequent interactions with a different world panic `"ecs: cross-world buffer:"`; interactions after a `clear()` panic `"ecs: stale buffer:"`.

### §17.3 — Reservation model (D1)

`cb.spawn(&mut world, bundle) -> Entity`:

- Borrows `&mut World` only for the call.
- Reserves a slot (`Free` → `Reserved`); pushes to `reservations`.
- Pushes `Spawn { entity, bundle }` to `commands`.
- Returns the real `Entity`.

`cb.insert` / `cb.remove` / `cb.despawn` do **not** borrow `World`. Registration and ordering are checked as follows (see §17.4).

### §17.4 — Record-time validation

| Operation             | Record-time check                                                                                                                                                |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cb.spawn(&mut w, b)` | Bundle dup (`"ecs: duplicate component in bundle:"`); registration (`"ecs: component not registered:"`); buffer binding; debug reservation-count assert (§17.11) |
| `cb.insert(e, v)`     | Ordering: `e` not in buffer's despawned set (`"ecs: buffer ordering:"`); registration **deferred to flush**                                                      |
| `cb.remove::<T>(e)`   | Ordering; registration deferred                                                                                                                                  |
| `cb.despawn(e)`       | Ordering (only fails on dead-and-already-despawned-in-buffer — actually idempotent; see below)                                                                   |

**Dead-entity policy at record time:**

- `cb.despawn(e)` on an entity that was dead **before** the buffer began recording → **no-op** at flush (composable with conditional logic).
- `cb.despawn(e)` then `cb.insert(e, ..)` later in the same buffer → **panic at the insert** `"ecs: buffer ordering:"` (the buffer killed it, then touched it — a bug).
- Duplicate `cb.despawn(e)` → coalesced to one.

### §17.5 — Coalescing (infallible)

**Preconditions (guaranteed by record-time checks):** no ordering violations, no dup spawn.

**Pass 1** — despawn coalescing: drop `Insert`/`Remove` on any `e` that also has a `Despawn`; coalesce duplicate `Despawn`s; if `Spawn { e }` and `Despawn(e)` both present, drop both and remove `e` from `reservations`.

**Pass 2** — per-`(entity, ComponentId)`: last-wins between `Insert` and `Remove`.

**Pass 3** — `Spawn` entries never reordered; `Insert`/`Remove` on their entities appear after the `Spawn`.

Post-condition: `commands` is canonical, minimal, ordered.

### §17.6 — Flush algorithm

```rust
pub fn flush(&mut self, cb: &mut CommandBuffer) -> FlushOutcome {
    if cb.poisoned { panic!("ecs: buffer poisoned:"); }

    // Binding
    match cb.world_id {
        Some(wid) if wid != self.id => panic!("ecs: cross-world buffer:"),
        None => {
            cb.world_id = Some(self.id);
            cb.clear_epoch = Some(self.clear_epoch);
        }
        _ => {}
    }
    if cb.clear_epoch != Some(self.clear_epoch) {
        panic!("ecs: stale buffer:");
    }

    cb.coalesce();                                 // infallible
    let commands = std::mem::take(&mut cb.commands);
    let mut drop_panics = Vec::new();

    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        for cmd in commands {
            apply(self, cmd, &mut drop_panics);
        }
    }));

    match result {
        Ok(()) => {
            cb.reservations.clear();
            FlushOutcome { drop_panics }
        }
        Err(e) => {
            cb.poisoned = true;
            // Clean up slots reserved but whose Spawn didn't apply.
            for entity in cb.reservations.drain(..) {
                if self.allocator.state(entity.index) == SlotState::Reserved {
                    self.allocator.free_and_bump(entity.index);
                }
            }
            std::panic::resume_unwind(e);
        }
    }
}
```

**`apply`:**

- **T0/T1 panic** (validation, allocation) → propagates → `catch_unwind` poisons buffer.
- **T2 panic** (Phase 7 `Drop` panic during migration) → caught **inside** `apply`, payload appended to `drop_panics`, `apply` returns `Ok(())`.
- **T3 panic** (double-panic) → aborts.

### §17.7 — Panic mid-flush guarantees

- **Per-command atomicity** (§3.22).
- **T0/T1 panic:** world consistent after the last successful command. Reserved slots for un-applied `Spawn`s are freed with generation bump (`free_and_bump`). Buffer poisoned. Next `flush` panics `"ecs: buffer poisoned:"`. User must `cancel` or drop+recreate.
- **T2 panic:** collected in `drop_panics`. `apply` returns `Ok`. `flush` proceeds. World valid. See §9.4.
- **Double-panic (T3):** process aborts.

### §17.8 — `cancel`

```rust
pub fn cancel(&mut self, cb: &mut CommandBuffer)
```

- Drains `commands` and `reservations`.
- Frees any slot still `Reserved` via `free_and_bump`.
- If `cb.clear_epoch != Some(self.clear_epoch)` (post-`clear`): reservations already gone; skip freeing, `debug_assert!` consistency, no panic.
- Resets `world_id = None`, `clear_epoch = None`, `poisoned = false`.
- **Idempotent:** calling twice is a no-op.

`cancel` on a different world than `spawn`'s → **unsupported** (documented in `# Panics`; no runtime check because `cb.spawn` bound the world).

### §17.9 — `Drop` (D1)

```rust
impl Drop for CommandBuffer {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            debug_assert!(
                self.reservations.is_empty(),
                "ecs: command buffer leak: dropped with {} un-flushed reservations; \
                 call World::flush or World::cancel before dropping",
                self.reservations.len()
            );
        }
        // Intentionally does NOT release slots — no World reference (D1).
    }
}
```

**Contract:** _"A `CommandBuffer` dropped with un-flushed reservations **leaks** the reserved entity slots. This is a resource-exhaustion bug, not UB. Callers must `world.flush(&mut cb)` or `world.cancel(&mut cb)` before dropping. `#[must_use]` catches `let _ = CommandBuffer::new();` but does **not** catch a buffer created, populated, and dropped via a variable — release builds leak silently. Debug builds assert on the invariant. See `docs/LIMITATIONS.md`."_

### §17.10 — Poison

- After a T0/T1 flush panic, buffer is `poisoned`. `flush` panics `"ecs: buffer poisoned:"`.
- `cancel` clears poison (resets to `new`-equivalent state).
- `Drop` works normally (reservations already freed in flush's panic path).

### §17.11 — Debug reservation-count assert

```rust
pub fn spawn(&mut self, world: &mut World, b: impl Bundle) -> Entity {
    // ... reservation logic ...
    debug_assert!(
        self.reservations.len() < 1_000_000,
        "ecs: command buffer leak: {} reservations; did you forget to flush?",
        self.reservations.len()
    );
    // ...
}
```

Catches the realistic "loop spawn without flush" misuse in development. No hard cap in release.

### §17.12 — `#[must_use]` caveat (documented)

In `CommandBuffer`'s rustdoc:

> **Misuse warning.** `#[must_use]` catches `let _ = CommandBuffer::new();` but **does not** catch a buffer created, populated, and dropped via a variable. In release builds, dropping a buffer with un-flushed reservations leaks those entity slots silently. Always `flush` or `cancel` before drop. See §17.9 and `docs/LIMITATIONS.md`.

### §17.13 — Cross-world binding details

`world_id` binds at first `cb.spawn(&mut world, ..)` or first `flush`/`cancel` into a `World`. A buffer with `Spawn` commands was bound at spawn time; a buffer with only `insert`/`remove`/`despawn` binds at first flush.

Flush into a different world than `spawn`'s → `"ecs: cross-world buffer:"`.

---

## §18 — `World::clear()`

```rust
pub fn clear(&mut self)
```

- All `Alive` and `Reserved` entities → `Free` (via `free_and_bump`, generation bumped); `Retired` stays `Retired`.
- All archetypes/tables/sparse sets emptied (columns dropped via `drop_fn`); capacities retained.
- Archetype graph and Table IDs **retained**.
- Query plan cache **retained**; all plans marked `Stale`.
- Component registry **retained**.
- `clear_epoch += 1`.
- **All `CommandBuffer`s bound before `clear()` become stale** (see §17.13).
- **Drop order of cleared components:** unspecified, same as `World::drop`.
- **Cannot overlap active queries:** type-enforced (`&mut self`).

---

## §19 — Panics, dead-entity policy, unsafe contracts

### §19.1 — Dead-entity / absent-component behavior

| API                                       | Dead entity                 | Component absent | Component unregistered                    |
| ----------------------------------------- | --------------------------- | ---------------- | ----------------------------------------- |
| `despawn`                                 | returns `false`             | —                | —                                         |
| `insert`                                  | panic `"ecs: dead entity:"` | inserts (normal) | panic `"ecs: component not registered:"`  |
| `remove`                                  | panic `"ecs: dead entity:"` | returns `None`   | panic `"ecs: component not registered:"`  |
| `get` / `get_mut`                         | `None`                      | `None`           | panic `"ecs: component not registered:"`  |
| `get_unchecked` / `get_unchecked_mut`     | **UB**                      | **UB**           | panic (safe check before safety contract) |
| `entity_ref` / `entity_mut`               | panic `"ecs: dead entity:"` | n/a              | n/a                                       |
| `spawn` w/ unregistered                   | —                           | —                | panic `"ecs: component not registered:"`  |
| `cb.insert` / `cb.remove` w/ unregistered | —                           | —                | panic **at flush**                        |
| `cb.spawn` w/ unregistered                | —                           | —                | panic **at record time**                  |

`Reserved` entities are treated as **dead** by all access APIs.

### §19.2 — Panic prefix table (frozen for all of `0.x`)

| Prefix                                  | Firing condition                                                            |
| --------------------------------------- | --------------------------------------------------------------------------- |
| `"ecs: borrow conflict:"`               | read/write overlap                                                          |
| `"ecs: dead entity:"`                   | dead, retired, or reserved passed to panicking API                          |
| `"ecs: component not registered:"`      | use before `register`                                                       |
| `"ecs: storage mismatch:"`              | `register` twice with different strategies                                  |
| `"ecs: entity index exhausted:"`        | `spawn` / `cb.spawn` on full index space                                    |
| `"ecs: flush invariant:"`               | flush-time state inconsistency (internal)                                   |
| `"ecs: duplicate component in query:"`  | query repeats a component                                                   |
| `"ecs: duplicate component in bundle:"` | bundle repeats a component                                                  |
| `"ecs: query access violation:"`        | `get`/`get_mut` outside `Q`'s access set                                    |
| `"ecs: cross-world buffer:"`            | buffer flushed into different `World`                                       |
| `"ecs: stale buffer:"`                  | buffer flushed after `World::clear()` since binding                         |
| `"ecs: buffer ordering:"`               | `insert`/`remove` on an entity despawned earlier in the buffer              |
| `"ecs: buffer poisoned:"`               | flush after a prior flush panic (T0/T1)                                     |
| `"ecs: component id exhausted:"`        | `ComponentId` space full                                                    |
| `"ecs: table column overflow:"`         | table reached column cap                                                    |
| `"ecs: table row overflow:"`            | table reached row cap                                                       |
| `"ecs: reader overflow:"`               | `BorrowMap` reader count saturated                                          |
| `"ecs: command buffer leak:"`           | **debug-only** — `Drop` with reservations, or `spawn` count over sane bound |

**Tests assert prefix only**, not full message.

### §19.3 — Unsafe contracts

- `get_unchecked::<T>(e)`: caller guarantees `e` is `Alive` ∧ `T` present on `e` ∧ no `&mut T` to same component on `e`. Violation is **UB**. Unregistered `T` → **safe panic**, checked first.
- `get_unchecked_mut::<T>(e)`: same plus exclusive access.
- Raw column access in iterators: encapsulated; module docs describe invariants; Miri-validated.

### §19.4 — `# Panics` / `# Safety` docs

Enforced by `clippy::missing_panics_doc` and `clippy::missing_safety_doc` (`deny`).

### §19.5 — Numeric limits

| Value               | Max **index**            | Max **count**  | On overflow                      |
| ------------------- | ------------------------ | -------------- | -------------------------------- |
| Entity              | `u32::MAX - 1`           | `u32::MAX`     | `"ecs: entity index exhausted:"` |
| Sparse `SENTINEL`   | `u32::MAX` (never valid) | —              | —                                |
| Entity generation   | `u32::MAX`               | —              | slot → `Retired`                 |
| `ComponentId`       | —                        | `u32::MAX`     | `"ecs: component id exhausted:"` |
| Table row           | `u32::MAX - 1`           | `u32::MAX`     | `"ecs: table row overflow:"`     |
| Table column index  | `u16::MAX - 1`           | `u16::MAX - 1` | `"ecs: table column overflow:"`  |
| Sparse dense length | —                        | `u32::MAX`     | implied by entity cap            |

The word "cap" **means maximum count**, never maximum index. Index domain is `[0, cap)`.

---

## §20 — Invariant checker harness

```rust
// src/invariants.rs, #[cfg(any(test, debug_assertions))]
pub(crate) fn check_world(world: &World);           // O(entities + components)
pub(crate) fn check_sparse_set<T>(set: &SparseSet<T>); // S1–S7
pub(crate) fn check_table(table: &Table);           // lockstep, entity_rows
pub(crate) fn check_locations(world: &World);       // locations point into valid rows
pub(crate) fn check_generation_bumps(world: &World); // debug-only, uses last_handed_out
```

**Called from:** property-test fuzzers after every op; `debug_assert!` at end of `World::flush` and `World::clear`. **Not** called during query iteration.

**Property tests:** hand-rolled LCG (no `proptest`); each sparse-set or table operation followed by the matching `check_*`.

---

## §21 — Cargo, CI, and build

### §21.1 — `Cargo.toml`

```toml
[package]
name = "voxel-ecs"
version = "0.1.0"
edition = "2021"
rust-version = "1.XX"   # current stable − 2, pinned at release

[dependencies]
hashbrown   = { version = "0.15", default-features = false, features = ["alloc"] }
rustc-hash  = "2"
smallvec    = "1"
bitvec      = "1"

[dev-dependencies]
bevy_ecs  = "*"
hecs      = "*"
criterion = { version = "0.5", features = ["html_reports"] }

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"
missing_docs           = "deny"

[lints.clippy]
missing_panics_doc         = "deny"
missing_safety_doc         = "deny"
undocumented_unsafe_blocks = "deny"
disallowed_types           = "deny"   # config: std::collections::* in src/
```

### §21.2 — CI matrix

| Job             | Toolchain             | Flags                                                                                                                    |
| --------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `fmt`           | stable                | `cargo fmt --check`                                                                                                      |
| `clippy`        | stable                | `-D warnings`                                                                                                            |
| `test`          | stable, beta, nightly | `cargo test --all-targets`                                                                                               |
| `doctest`       | stable                | `cargo test --doc`                                                                                                       |
| `examples`      | stable                | `cargo test --examples`                                                                                                  |
| `msrv`          | `1.MINIMUM`           | `cargo check --all-targets` + `cargo test --lib`                                                                         |
| `miri`          | nightly               | `MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-disable-isolation" cargo miri test --lib -- query:: sparse:: entity:: cmd::` |
| `panics`        | stable                | `cargo test --test panic_prefix`                                                                                         |
| `std-isolation` | stable                | `cargo test --test no_std_imports`                                                                                       |
| `bench`         | nightly (non-gating)  | `cargo bench`                                                                                                            |

---

## §22 — Benchmarks

Run against workloads defined in `benches/README.md`. Baselines: `bevy_ecs`, `hecs` (dev-dep, manual, non-CI).

| Benchmark                                  | Category                      |
| ------------------------------------------ | ----------------------------- |
| `spawn`                                    | lifecycle                     |
| `spawn_batch` (loop)                       | lifecycle                     |
| `despawn`                                  | lifecycle                     |
| `insert` (existing, table)                 | structural                    |
| `insert` (new, table)                      | structural                    |
| `insert_sparse`                            | structural                    |
| `remove` (table)                           | structural                    |
| `remove_sparse`                            | structural                    |
| `query &P`                                 | read                          |
| `query &P &V`                              | read                          |
| `query &P &V &M`                           | read                          |
| `query hybrid &P &Health_sparse`           | read                          |
| `query sparse-only &Health`                | read                          |
| `query sparse-only &Health &Stamina`       | read (smallest-set anchor)    |
| `query all-live (Entity, Without<Health>)` | read (AllLiveEntities anchor) |
| `fragment_iter`                            | read                          |
| `command_buffer_flush`                     | structural                    |
| `entity_ref`                               | access                        |
| `entity_mut`                               | access                        |

---

## §23 — Examples

- `examples/basic.rs` — register, spawn, query.
- `examples/hybrid.rs` — table + sparse, hybrid query.
- `examples/deferred.rs` — `CommandBuffer`: `spawn`, `insert`, `flush`, `FlushOutcome::drop_panics` inspection, `cancel` on abort paths, `// WARNING` showing the D1 leak.

Wired into `cargo test --examples` and doctests.

---

## §24 — Test matrix (spec § → test file)

| Spec §                      | Test file                                                                                             |
| --------------------------- | ----------------------------------------------------------------------------------------------------- |
| §6 Entity allocator         | `tests/allocator.rs` (spawn/despawn/recycle, generation bump, retirement)                             |
| §7 Component registry       | `tests/registry.rs` (idempotent, storage mismatch, ZST, drop count)                                   |
| §8 Bundles                  | `tests/bundle.rs` (unregistered panic, arity)                                                         |
| §9 Archetype + migration    | `tests/archetype.rs` (edges, migration tiers T0–T2)                                                   |
| §10 Table                   | `tests/table.rs` (swap-remove, alignment, lockstep, over-aligned ZST)                                 |
| §11 Sparse set              | `tests/sparse.rs` (S1–S7, property tests)                                                             |
| §12 Location                | `tests/locations.rs` (post-swap-remove, post-migration)                                               |
| §13 Query                   | `tests/query.rs` (pure-table, pure-sparse, hybrid, all-live, filters, duplicate, access-set)          |
| §14 Borrow tracking         | `tests/borrow.rs` (conflict matrix)                                                                   |
| §15 `EntityRef`/`EntityMut` | `tests/views.rs` (acquisition order, panic unwind)                                                    |
| §17 Command buffer          | `tests/command_buffer.rs` (D1 leak, `cancel`, `drop_panics`, ordering, poisoning, cross-world, stale) |
| §18 `clear()`               | `tests/clear.rs` (reset, plan invalidation, generation bump)                                          |
| §19 Panics                  | `tests/panic_prefix.rs` (one per prefix; debug-only gated)                                            |
| §19.5 Numeric limits        | `tests/limits.rs`                                                                                     |
| §20 Invariants              | `src/invariants.rs` + `tests/invariants_property.rs`                                                  |
| §19.2 prefix list           | `tests/panic_prefix.rs`                                                                               |
| Miri                        | CI; targets `query`, `sparse`, `entity`, `cmd`                                                        |

---

## §25 — Changelog (0.1 → 1.0.0)

- **0.1:** Initial blueprint.
- **0.2:** Round 1 — scope, entity model, registry, bundles, dead-entity policy.
- **0.3:** Round 2 — query surface, plan cache, borrow rules, panic prefixes.
- **0.4:** Round 3 — flush ordering, sparse invariants, query inner loops, CI.
- **0.5:** Round 4 — Option C command buffer, migration algorithm, `EntityRef` protocol, `World::clear()`, invariant harness.
- **1.0 (draft):** Round 5 — premature freeze.
- **1.1:** Round 6 — 12 critical issues: generation bump on free, `Send` guarantees, duplicate detection, flush panic, migration tiers, anchor modes, API completion, unregistered matrix, cross-world buffer, iteration order, concrete representation.
- **1.2:** Rounds 7+ — D1 buffer lifecycle, `FlushOutcome.drop_panics`, `'q` lifetimes, access-set restriction, `!Send` markers, `Bundle: Send`, first-flush binding, migration Phase 7 catch+expose.
- **1.0.0 (frozen):** Final — `thread::panicking()` `Drop` guard, `cancel` idempotence, `#[must_use]` doc caveat, debug reservation-count assert, `"ecs: command buffer leak:"` prefix.

**Breaking changes since v1.0 draft** (all pre-release, no compatibility owed):

1. `register<T>` requires `T: Send`.
2. `"ecs: unregistered component:"` → `"ecs: component not registered:"`.
3. `get_unchecked` on unregistered `T` panics (was UB).
4. Iteration order unspecified.
5. `CommandBuffer` poisons on T0/T1 flush panic.
6. `CommandBuffer` gains cross-world/stale binding.
7. `World::clear()` bumps generations.
8. `Deref<Target = impl Iterator>` removed; use `.iter()`.
9. `get`/`get_mut` restricted to `AccessSet`.
10. Uniform `'q` item lifetimes.
11. `Bundle: Send`.
12. `QueryMut`/`EntityMut` explicitly `!Send`.

---

## §26 — Implementation tasks

| #   | Task                                                                                                                        | Depends     |
| --- | --------------------------------------------------------------------------------------------------------------------------- | ----------- |
| 1   | `prelude.rs`, `Cargo.toml`, CI skeleton, std-isolation test                                                                 | —           |
| 2   | `entity.rs` — allocator, `SlotState`, `free_and_bump`, `last_handed_out` debug tracker                                      | 1           |
| 3   | `component.rs` — registry, `T: Send + 'static`, ZST, drop_fn                                                                | 2           |
| 4   | `location.rs` — `EntityLocation`, `locations: Vec`                                                                          | 2           |
| 5   | `table.rs` — lockstep growth, swap-remove, ZST `NonNull::<T>::dangling()`, overflow checks                                  | 3, 4        |
| 6   | `sparse.rs` — S1–S7, `SENTINEL`, overflow checks                                                                            | 2           |
| 7   | `archetype.rs` — edges, Phase 0–7 migration, drop-panic capture                                                             | 5, 6        |
| 8   | `borrow.rs` — `UnsafeCell<BorrowMap>`, reader overflow                                                                      | 3           |
| 9   | `thread_send.rs` — targeted `unsafe impl Send`                                                                              | 5, 6        |
| 10  | `views.rs` — `EntityRef`/`EntityMut`, `!Send`, acquisition                                                                  | 5, 6, 8     |
| 11  | `query/plan.rs` — `QueryPlan`, `AnchorMode`, `AccessSet`, dup check                                                         | 7           |
| 12  | `query/fetch.rs` — fetch impls for supported forms                                                                          | 11          |
| 13  | `query/iter.rs` — anchor-specific loops                                                                                     | 12, 10      |
| 14  | `query/mod.rs` — `Query`/`QueryMut` guards, `'q` lifetimes, named iterators, `get`/`get_mut` restriction                    | 13          |
| 15  | `cmd.rs` — `CommandBuffer` (D1), `spawn`, `flush`, `cancel`, `Drop`, poison, `FlushOutcome.drop_panics`, record-time checks | 5, 7, 8     |
| 16  | `world.rs` — `World`, `WorldId`, `clear_epoch`, all public methods                                                          | 2–15        |
| 17  | `invariants.rs` — `check_*` + property fuzz                                                                                 | 5, 6, 7, 16 |
| 18  | `tests/panic_prefix.rs` — one test per prefix                                                                               | 16          |
| 19  | `tests/duplicates.rs` — bundle + query duplicates                                                                           | 16          |
| 20  | `tests/cross_world.rs` — binding, poison, ordering                                                                          | 15, 16      |
| 21  | `tests/command_buffer.rs` — D1 leak, `cancel`, `drop_panics`, ordering, debug asserts                                       | 15, 16      |
| 22  | `benches/` — all §22 benchmarks + `benches/README.md`                                                                       | 16          |
| 23  | `examples/{basic,hybrid,deferred}.rs`                                                                                       | 16          |
| 24  | Docs pass: `# Panics`/`# Safety`, `docs/SPEC.md`, `docs/CHANGELOG.md`, `docs/LIMITATIONS.md`, README                        | 16–23       |
| 25  | Full CI matrix green (Miri, MSRV, std-isolation, bench non-gating)                                                          | 1–24        |

**Critical path:** 1 → 2 → {3, 4} → {5, 6} → 7 → 8 → 10 → 11 → 12 → 13 → 14 → 15 → 16. Everything after 16 parallelizes.

---

## §27 — `docs/LIMITATIONS.md`

1. **Dropped un-flushed `CommandBuffer` leaks reserved slots.** Bounded by allocator index space. Silent in release; `#[must_use]` + debug `Drop` assert + debug `spawn`-count assert are the mitigations. Callers must `flush` or `cancel` before drop. A future `World::reap_orphaned` (D2) or hard cap can address this without re-opening the eager-spawn contract.
2. **No `PendingEntity`.** `cb.spawn` is eager; a real `Entity` is available immediately. Deferred spawn during iteration is not supported.
3. **`AllLiveEntities` anchor is O(alive).** `(Entity)`, `(Entity, Option<&Health>)`, `(Entity, Without<Health>)` scan all live entities.
4. **Iteration order unspecified.**
5. **No archetype/table GC.** `shrink_archetypes()` deferred.
6. **No `no_std`.** Structured for it; the swap is a post-1.0 change.
7. **No automatic sparse↔table migration.**
8. **No batch operations.**
9. **No parallel queries.** `World: !Sync`.
10. **Registered components must be `Send`.**

---

## §28 — Freeze declaration

**This document is the authoritative, frozen specification for `voxel-ecs` v1.0.0.**

Frozen contracts:

- §3 core invariants (25 items).
- §4 public API surface (all signatures).
- §11 sparse invariants S1–S7.
- §9.3 migration tiers T0–T3.
- §19.2 panic prefix list (additions allowed pre-`1.0.0`; removals and renames only at a new round).
- §13.4 `AnchorMode` variants.
- §17.9 D1 buffer lifecycle.

Changes to any frozen contract require a new round. Internal refactors, doc changes, bug fixes, and additive out-parameters do not.

**Build against this document.** If implementation reveals a frozen contract is wrong, stop and re-open the round.
