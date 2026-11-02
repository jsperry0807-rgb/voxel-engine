# Implementation design — my_ecs 1.0.0

This document describes how [`SPEC.md`](SPEC.md) is realized. It is the
engineering companion to the frozen contract: the spec says *what*, this says
*how* and *why*. Where the spec leaves a choice open, the chosen option and its
rationale are recorded here.

## 1. Shape of the system

```
                         ┌──────────────────────────── World ───────────────────────────┐
                         │ id: u64, clear_epoch: u64                                    │
  Entity { index,gen } ──┤ allocator: EntityAllocator                                    │
                         │ locations: Vec<EntityLocation>                               │
                         │ registry: Registry { ComponentInfo, TypeId → ComponentId }   │
                         │ archetypes: Vec<Archetype>       (archetype id == table id)  │
                         │ tables: Vec<Table>               (columns + entity_rows)     │
                         │ sparse_sets: Vec<Option<SparseSetErased>>                    │
                         │ archetype_index: HashMap<sorted ids, ArchetypeId>            │
                         │ borrows: UnsafeCell<BorrowMap>                               │
                         │ plans: UnsafeCell<PlanCache>                                 │
                         └──────────────────────────────────────────────────────────────┘
```

Two independent storage families:

* **Table** components define an archetype. An entity's table membership is
  the sorted set of its table components. Adding/removing one migrates the
  entity between tables. Tables are columnar: every column is a raw allocation
  of `T` and all columns are length-locked.
* **SparseSet** components never affect archetype identity. Each registered
  sparse component owns a `SparseSet<T>`: a `sparse: Vec<u32>` indexed by
  entity index (with `u32::MAX` as sentinel), a dense value array, and a
  parallel dense `Vec<Entity>`. Adding/removing is in-place.

An entity therefore has exactly one table row plus an arbitrary sparse
membership derived solely from the sparse arrays (spec §3.3).

## 2. Entity allocation

`EntityAllocator` keeps three parallel `Vec`s indexed by entity index:

* `generations: Vec<u32>` — current generation, starts at `1`;
* `states: Vec<SlotState>` — `Free | Reserved | Alive | Retired`;
* `free_list: Vec<u32>` — LIFO reuse.

`allocate(state)` either pops the free list (generation unchanged, because
`free_and_bump` already bumped it) or appends a new slot with generation `1`.
`free_and_bump` is the only path into `Free`: it increments the generation or,
at `u32::MAX`, marks the slot `Retired` and never reuses it. `Reserved` slots
are dead to every access API.

The allocator caps the index space at `u32::MAX` slots, with valid indices
`0 ..= u32::MAX - 1`, leaving `u32::MAX` exclusively for the sparse sentinel.

## 3. Component registry and bundles

`Registry` stores `ComponentInfo` per `ComponentId` plus a
`HashMap<TypeId, ComponentId>`. Registration is append-only and idempotent for
the same storage choice; a different choice panics with
`"ecs: storage mismatch:"`.

`ComponentInfo` records layout, a type-erased `drop_fn` (only when
`needs_drop::<T>()`), the storage strategy, and whether the type is a ZST.
Table ZSTs use a provenance-less aligned dangling pointer and presence is
carried entirely by archetype membership; sparse ZSTs still maintain sparse
presence and run `drop_fn` once per dense entry.

### Bundles

`Bundle` is a sealed public trait; the actual machinery is the hidden
`BundleImpl` supertrait:

```rust
fn component_ids(world: &World) -> SmallVec<[ComponentId; 16]>;
fn put(self, world: &mut World, entity: Entity);
```

`()` and tuples of arity 1–16 are implemented by a macro. `component_ids`
resolves every element through the registry (so an unregistered type panics at
the bundle boundary) and `put` writes each value into the freshly reserved row
or sparse set. Duplicate detection is done in `World::spawn` /
`CommandBuffer::spawn` by sorting the collected ids and checking adjacent
equality — O(n log n) with `n ≤ 16`.

`put` is only ever called on a row whose table components are all
uninitialized: spawn creates a new row, and migration moves existing components
separately. This is the key invariant that makes `ptr::write` sound without a
"is this slot initialized" flag.

## 4. Tables and the row protocol

`Table` owns `entity_rows: Vec<Entity>`, a sorted
`SmallVec<[(ComponentId, Column); 8]>`, and a capacity. Every `Column` is a raw
`NonNull<u8>` allocation with an element `Layout`, capacity, length, and
optional drop glue.

Rows are managed with three primitives:

| Primitive | Effect |
| --- | --- |
| `begin_row() -> u32` | ensures capacity (1.5× growth, min 4) and returns the future row index **without committing it** |
| `commit_row(entity, row)` | pushes the entity into `entity_rows`, sets every column length |
| `swap_remove_nodrop(row)` | moves the last row into `row`, updates `entity_rows`, decreases all lengths, **drops nothing** |

`commit_row` is what makes partial construction invisible: a query can never
see an uncommitted row, because `table.len()` is `entity_rows.len()`.

For despawn, `take_row` copies every droppable value into a separately
allocated, correctly aligned `Scratch` buffer. The row is then logically moved
out, so `swap_remove_nodrop` can overwrite its bytes without dropping anything.
`Scratch::drop_items` either propagates the first panic (immediate despawn) or
isolates each destructor with `catch_unwind` and collects payloads (flush).
The scratch buffer is always allocated (even when empty) so its `Drop` is
uniform.

## 5. Migration (spec §9.2)

`migrate_add<T>` and `migrate_remove<T>` implement Phases 0–7 for the single
component operations exposed by 1.0. The bundle machinery is used only for
spawn, never for component removal, so the generalized Phase 3/7 scratch path
is exercised by despawn and by deferred removal.

Add (entity in table `S`, component `X` new):

1. Validate (done at the public method boundary).
2. `target = archetype_add(S, X)` (creating the table if necessary) and
   `target_row = target.begin_row()`.
3. Copy every shared column from `S[row]` to `T[target_row]` with
   `ptr::copy_nonoverlapping` (bitwise moves; ownership transfers).
4. `ptr::write` the new `X` into the target row.
5. Commit the location and the target row.
6. `S.swap_remove_nodrop(row)`, updating the moved entity's location.
7. No destructors run: no value was removed.

Remove mirrors this, skipping the removed column during the copy and
`ptr::read`-ing the removed value out before the source row is overwritten.
The removed value is returned to the caller; on the flush path the caller
drops it inside `catch_unwind` and collects a panic payload.

The two tables are borrowed simultaneously through
`two_tables_mut`, which uses raw pointers after asserting distinct indices.

Failure tiers follow the spec exactly: validation and allocation leave the
source intact; destructor panics leave the world structurally valid and only
lose the value.

## 6. Query engine

### 6.1 Type-level surface

```rust
pub trait QuerySpec: sealed::Sealed + 'static {
    type Item<'q> where Self: 'q;
    #[doc(hidden)] type State: Copy + 'static;
    #[doc(hidden)] fn state(world: &World) -> Self::State;
    #[doc(hidden)] fn describe(world: &World) -> QueryDesc;
    #[doc(hidden)] unsafe fn fetch<'q>(...) -> Option<Self::Item<'q>>;
    #[doc(hidden)] fn matches(...) -> bool;
}
```

Each supported term implements the hidden `Fetch` trait:
`&'static T`, `&'static mut T`, `Entity`, `Option<&'static T>`,
`Option<&'static mut T>`, `With<T>`, `Without<T>`. Tuple `QuerySpec` impls
combine fetch terms and their `State` tuples. `ReadOnlyQuery` is implemented
for exactly those tuples whose terms implement `ReadOnlyFetch`, which is what
makes `World::query(&self)` sound without runtime write checks.

`QuerySpec: 'static` is deliberate: it lets the plan cache key on
`TypeId::of::<Q>()` (spec §3.13) and lets elided `&T` in a turbofish resolve to
`&'static T` while item lifetimes remain `'q` (GAT indirection).

### 6.2 Description → plan

At construction:

1. `Q::state` resolves component ids (panics `"ecs: component not
   registered:"`).
2. `Q::describe` produces a `QueryDesc`: read/write ids, fetch terms for
   duplicate detection, required/excluded table and sparse ids.
3. `validate_desc` sorts the fetch terms and panics
   `"ecs: duplicate component in query:"` on adjacent duplicates; it also
   rejects a component that appears in both read and write.
4. `World::plan_for::<Q>` returns the cached plan or builds one and stores it
   in a `Box` (stable address).
5. Borrows are acquired atomically from the plan's `AccessSet`.

A plan stores an `AnchorMode`, per-archetype matches with precomputed table
column indices, sparse filter lists, and the access set. Plan state is
`Unbuilt → Ready`, with `Ready → Stale` on a new archetype, a new registration,
or `World::clear()`.

### 6.3 Anchors and iteration

| Anchor | When | Loop |
| --- | --- | --- |
| `TableArchetypes` | any required table component | archetypes → rows |
| `SmallestSparseSet` | no table term, at least one required sparse term | dense entities of the smallest required set |
| `AllLiveEntities` | otherwise | allocator slots |

Each candidate calls `Q::fetch`, which re-checks sparse presence per row and
applies `With`/`Without` per row. Plan-level archetype skipping is therefore a
pure optimization; correctness does not depend on it.

`QueryIter` and `QueryIterMut` share `next_item`. The iterator holds
`&'w World`, `&'w QueryPlan`, `Q::State`, and a small anchor cursor. Mutable
items are produced from raw column/dense pointers, which is sound because the
guard acquired the plan's write borrows and `iter_mut` exclusively borrows the
guard for `'q`.

### 6.4 Random access and access sets

`Query::get`/`QueryMut::get`/`get_mut` check registration first, then the
plan's access set (`"ecs: query access violation:"`), then liveness and
`Q::matches`, and finally read through raw pointers without acquiring another
borrow (the guard already holds the plan's borrows). Sparse-only filters are
included in the access set because per-row presence checks read the sparse
array; table filters are not, because they are resolved by archetype skipping
and acquire no borrow (matching §14.1).

## 7. Borrow tracking

`BorrowMap` is a `HashMap<ComponentId, {readers: u32, writer: bool}>` inside an
`UnsafeCell<BorrowMap>` on the world. `World: !Sync` makes interior mutability
sound: only one thread can ever hold `&World`, and all `&mut World` APIs are
exclusive.

`acquire_many` validates the entire read/write set before mutating anything, so
a conflict panic can never leak a partially acquired set. Conflicts and reader
overflow are always-on (`"ecs: borrow conflict:"`, `"ecs: reader overflow:"`),
not debug-only.

`EntityRef`/`EntityMut` collect the entity's component ids first (table columns
in id order, then sparse ids in slot order), acquire the whole set atomically,
and release it in reverse order on drop. This ordering is deterministic and
panic-safe.

## 8. Command buffer

### Commands

```rust
Spawn   { entity, ids, apply: FnOnce(&mut World, Entity) }
Insert  { entity, key: TypeId, apply: FnOnce(&mut World, &mut DropPanics) }
Remove  { entity, key: TypeId, apply: FnOnce(&mut World, &mut DropPanics) }
Despawn { entity }
```

`spawn` eagerly reserves a slot and returns a real entity. `insert`/`remove`
store a type-erased thunk because only `T` can downcast the value; registration
is deliberately deferred to flush. `despawn` records the entity and a
`despawned` set used for record-time ordering checks.

### Coalescing

1. Find spawn entities that are also despawned; drop both commands and remove
   the reservation.
2. Drop `Insert`/`Remove` commands targeting any despawned entity.
3. Walk the remaining commands backwards, keeping the last
   `(entity, TypeId)` insert/remove, then reverse. Spawn/Despawn keep their
   relative order, so pass 3 of the spec ("spawns never reordered") is
   automatic.

### Flush

1. Poison check.
2. Bind: first interaction fixes `world_id`/`clear_epoch`; a different world
   panics `"ecs: cross-world buffer:"`, a post-clear epoch panics
   `"ecs: stale buffer:"`.
3. Coalesce.
4. Free reservations without an active spawn (coalesced-out pairs).
5. Apply commands inside `catch_unwind`. A spawn materializes its slot only
   after the bundle is written and the row is committed. Destructor panics from
   replaced/removed values and from despawn are caught per value and pushed to
   `FlushOutcome::drop_panics`.
6. On success clear reservations. On a T0/T1 panic mark the buffer poisoned,
   reclaim any still-`Reserved` slots (with generation bump, sparse purge, and
   location invalidation), then resume unwinding.

`cancel` frees still-reserved slots when the binding matches, clears the
poison, and resets the buffer. It is idempotent. When the epoch differs (the
world was cleared) the slots were already reclaimed by `clear`, so cancel only
clears bookkeeping.

`Drop` intentionally cannot free anything (D1). In debug builds it asserts on
un-flushed reservations and on a sane reservation count in `spawn`.

## 9. `World::clear`

1. `allocator.clear()` frees every `Alive`/`Reserved` slot via
   `free_and_bump`; `Retired` stays retired.
2. Every location becomes `INVALID`.
3. Every table drops its values with per-value `catch_unwind` and keeps its
   capacity.
4. Every sparse set drops its dense values and truncates its sparse array to
   length 0 (the only operation allowed to truncate).
5. Every cached plan is marked stale.
6. `clear_epoch` increments, invalidating previously bound buffers.

Registrations, archetypes, tables, and capacities are retained.

## 10. Unsafe inventory

All `unsafe` is concentrated in a small number of audited places:

| Location | Why it is sound |
| --- | --- |
| `table.rs` column alloc/grow/ptr | allocations use the registered `Layout`; `ptr_at` is called only for committed or reserved rows; ZST pointers are aligned dangling pointers |
| `table.rs` `take_row`/`swap_remove_nodrop` | the row is logically moved out before being overwritten; scratch copies are correctly aligned |
| `sparse.rs` dense storage | manual allocation with `Layout::array::<T>`; `get_ptr` requires world borrow discipline; swap-remove moves values bitwise and decrements `len` |
| `world.rs` migration | two distinct tables, shared columns copied bitwise, values moved exactly once |
| `query/fetch.rs` | yields references from raw storage after the plan's borrow set was acquired; callers pass live entities and correct archetype/row |
| `query/plan.rs` | `UnsafeCell<PlanCache>`; plans live in stable `Box`es, and invalidation happens only under `&mut World` |
| `borrow.rs` | `UnsafeCell<BorrowMap>`; `World: !Sync` |
| `thread_send.rs` | `Send` for `Column` and `SparseSetErased`, anchored on `register<T: Send>` |

Every block carries a `// SAFETY:` justification and the crate denies
`unsafe_op_in_unsafe_fn`.

## 11. Panic and failure policy

* Panic prefixes are frozen and are the only stable part of panic messages;
  tests assert prefixes.
* Destruction paths never double-drop: the scratch design moves values out
  first, and structural commit (length changes, locations, generation bumps)
  happens before user destructors run.
* Flush catches destructor panics only; command-application panics (validation
  and allocation) poison the buffer.
* Double panic (a destructor panic while already unwinding) aborts, per Tier 3.
* `World::clear` catches destructor panics and continues; the world is already
  structurally empty.

## 12. Complexity

| Operation | Cost |
| --- | --- |
| spawn | O(b log b) for bundle duplicate check + amortized O(b) writes |
| despawn | O(#sparse components) + O(table columns) |
| insert/remove table | O(columns) copy + archetype lookup |
| insert/remove sparse | amortized O(1) |
| table query | O(matching rows × terms) |
| sparse query | O(size of smallest set × terms) |
| all-live query | O(allocator slots × terms) |
| flush | O(commands + coalescing) |

## 13. Testing and CI

* Unit tests cover the allocator, borrow map, sparse S1–S7, and a 800-step
  internal property fuzzer with the full invariant checker after every step.
* Integration tests follow the §24 matrix: allocator, registry, bundle,
  archetype/migration, table/alignment/ZST, sparse, locations, query forms,
  borrow rules, views, command buffer, clear, duplicates, cross-world/poison,
  panic prefixes, numeric limits, and no-std import hygiene.
* `tests/invariants_property.rs` fuzzes the public API against a model.
* The panic-prefix test also scans `src/` to ensure every frozen prefix is
  still produced.
* CI: fmt, clippy (`-D warnings`), stable/beta/nightly tests, doctests,
  examples, MSRV check, Miri on the unsafe-heavy modules, panic-prefix tests,
  std-isolation test, and a non-gating benchmark job.

## 14. Decisions where the spec leaves room

| Topic | Decision | Rationale |
| --- | --- | --- |
| `Entity` term in the access set | reserved `ComponentId(0)` used only for duplicate detection | keeps the fetch set homogeneous |
| `With`/`Without` as fetch terms | not included in duplicate detection; allowed alongside `&T` | filters are constraints, not values; `(&A, With<A>)` is meaningful and common |
| Plan cache storage | `HashMap<TypeId, Option<Box<QueryPlan>>>` | stable plan addresses; `None` encodes `Stale` |
| Table column order | sorted by `ComponentId` | deterministic archetype identity and migration |
| Sparse components on despawn | iterate the registered sparse sets | no per-entity sparse bit, per spec §3.3 |
| Migration scratch | one aligned allocation per despawn | simple, panic-safe, no per-type codegen |
| `std` exceptions | `std::thread::panicking()` + `std::sync::atomic` | required by the D1 `Drop` contract and world ids; everything else is routed through `prelude.rs` |
| Benchmark baselines | criterion only | `bevy_ecs`/`hecs` are optional external comparisons (see `benches/README.md`); keeping them out of dev-deps keeps CI light |
| Query type bounds | `QuerySpec: 'static` | enables `TypeId` plan keys and `'static` elided term lifetimes while items stay `'q` |

## 15. Build metadata

* `Cargo.toml` sets `rust-version = "1.85"` (conservative current-stable minus
  two-releases policy) and denies `unsafe_op_in_unsafe_fn`, `missing_docs`,
  and the panics/safety doc lints.
* `clippy.toml` disallows `std::collections` and lock primitives in `src/`.
* `tests/no_std_imports.rs` enforces the same rule at test time.

## 16. Validation status of this implementation

The following were run on the development machine (Windows, Rust 1.98 stable):

* `cargo fmt --all --check`
* `cargo clippy --all-targets -- -D warnings`
* `cargo test --all-targets` in both debug and release profiles
* `cargo test --doc` (including the `!Sync`/`!Send` compile-fail assertions)
* `cargo +1.85.0 check --all-targets` and `cargo +1.85.0 test --lib` (MSRV)
* `cargo +nightly miri test --lib`, plus the query, views, command-buffer,
  table, clear, borrow, and archetype integration suites under
  `-Zmiri-strict-provenance -Zmiri-disable-isolation`
* `cargo run --example basic|hybrid|deferred`

Heavy property fuzzers are `#[cfg_attr(miri, ignore)]` because they are far too
slow under Miri; they run in the normal test matrix. The `no_std_imports` test
is filesystem-only and is excluded from Miri runs (it is not part of the Miri
CI job).
