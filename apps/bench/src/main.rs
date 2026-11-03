//! Runs the software benchmark lenses and writes a report.
//!
//! The commands live in [`voxel_bench::cli`]; this binary installs the counting
//! allocator the memory lens needs and hands over.
//!
//! ```
//! cargo run --release -p voxel-bench -- all
//! cargo run --release -p voxel-bench -- memory --out target/bench
//! cargo run --release -p voxel-bench -- cpu --print
//! cargo run --release -p voxel-bench -- frame --input target/frames.csv
//! ```

use voxel_bench::CountingAllocator;

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn main() {
    voxel_bench::cli::main();
}
