# Benchmarks

The benchmark suite mirrors spec §22. It is **informational and non-gating**.

```sh
cargo bench
```

Criterion writes HTML reports to `target/criterion/`.

## Workloads

| Group | Benchmark | What it measures |
| --- | --- | --- |
| lifecycle | `spawn` | one bundle spawn in a fresh world |
| lifecycle | `spawn_batch` | 1 000 spawns |
| lifecycle | `despawn` | one despawn |
| structural | `insert_existing_table` | replace a table component in place |
| structural | `insert_new_table` | migrate to a new archetype |
| structural | `insert_sparse` | in-place sparse insert |
| structural | `remove_table` | migrate out of an archetype |
| structural | `remove_sparse` | in-place sparse remove |
| query | `table_position` | one table component |
| query | `table_position_velocity` | two table components |
| query | `table_position_velocity_mass` | three table components |
| query | `hybrid_position_health` | table + sparse (table anchor) |
| query | `sparse_health` | sparse-only, smallest-set anchor |
| query | `sparse_health_stamina` | two sparse components |
| query | `all_live_without_health` | `AllLiveEntities` anchor |
| query | `fragment_iter` | manual `.iter()` loop |
| command_buffer | `flush_inserts` | reserve + insert + flush |
| views | `entity_ref` | read a whole entity |
| views | `entity_mut` | mutate a whole entity |

## Comparing against other ECS crates

The spec lists `bevy_ecs` and `hecs` as manual, non-CI baselines. They are
intentionally **not** dev-dependencies in this implementation: pulling them in
would make every test build heavy and couple the crate to their release
cadence. To compare locally, add them to a scratch benchmark crate and run the
same workloads:

```toml
[dev-dependencies]
bevy_ecs = "*"
hecs = "*"
```

Report the machine, toolchain, and world size alongside results; ECS
throughput is dominated by archetype count and component sizes.
