//! `voxel-worldgen` command line.
//!
//! ```text
//! voxel-worldgen macro [--seed N] [--limit N] [--out DIR]
//! ```
//!
//! `macro` generates the 4,096 tiles of the 1,024 km world at 128 m, prints a
//! summary and the aggregated content hash, and optionally writes each tile to
//! `<DIR>/<x>_<z>.vxt`. The hash is the FNV-1a 64 fold of every tile's content
//! hash: two runs with the same seed print the same value.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use voxel_worldgen::generator::{MacroGenerator, SEA_LEVEL_UNITS};
use voxel_worldgen::hydro::Hydrology;
use voxel_worldgen::macro_grid::{
    CELL_SIZE_M, TILE_COUNT, TILES_PER_AXIS, TilePos, WORLD_CELLS, height_from_q16,
};
use voxel_worldgen::tile::{TILE_BYTES, fnv_start, fnv_u64};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("voxel-worldgen: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("macro") => macro_command(&args[1..]),
        Some("hydro") => hydro_command(&args[1..]),
        Some("--help" | "-h") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?}; try --help")),
    }
}

fn print_usage() {
    println!(
        "voxel-worldgen — deterministic world generation\n\
         \n\
         Usage:\n\
         \x20 voxel-worldgen macro [--seed N] [--limit N] [--out DIR]\n\
         \x20 voxel-worldgen hydro [--seed N] [--size N]\n\
         \n\
         Options:\n\
         \x20 --seed N    world seed, decimal or 0x hex (default 1)\n\
         \x20 --limit N   macro: generate only the first N tiles, row-major\n\
         \x20 --out DIR   macro: write each tile to DIR/<x>_<z>.vxt\n\
         \x20 --size N    hydro: run on the first N × N cells (default 8000)"
    );
}

fn macro_command(args: &[String]) -> Result<(), String> {
    let mut seed = 1u64;
    let mut limit: Option<u32> = None;
    let mut out: Option<PathBuf> = None;

    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let mut value = |name: &str| -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--seed" => seed = parse_u64(&value("--seed")?)?,
            "--limit" => limit = Some(parse_u32(&value("--limit")?)?),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            other => return Err(format!("unknown argument {other:?}; try --help")),
        }
        index += 1;
    }

    let generator = MacroGenerator::new(seed);
    let tile_count = limit.unwrap_or(TILE_COUNT).min(TILE_COUNT);
    if let Some(directory) = &out {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("create {}: {error}", directory.display()))?;
    }

    let started = Instant::now();
    let mut world_hash = fnv_start();
    for tile_index in 0..tile_count {
        let pos = TilePos::new(
            (tile_index % TILES_PER_AXIS) as i32,
            (tile_index / TILES_PER_AXIS) as i32,
        );
        let tile = generator
            .generate_tile(pos)
            .map_err(|error| error.to_string())?;
        world_hash = fnv_u64(world_hash, tile.content_hash());
        if let Some(directory) = &out {
            let path = directory.join(format!("{}_{}.vxt", pos.x(), pos.z()));
            std::fs::write(&path, tile.to_bytes())
                .map_err(|error| format!("write {}: {error}", path.display()))?;
        }
    }
    let elapsed = started.elapsed();
    let seconds = elapsed.as_secs_f64();

    println!(
        "macro seed={seed} tiles={tile_count} bytes={} ms={} tiles_per_s={:.1}",
        tile_count as u64 * TILE_BYTES as u64,
        elapsed.as_millis(),
        tile_count as f64 / seconds.max(f64::MIN_POSITIVE),
    );
    println!("world_hash={world_hash:016x}");
    Ok(())
}

fn hydro_command(args: &[String]) -> Result<(), String> {
    let mut seed = 1u64;
    let mut size = WORLD_CELLS as usize;

    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let mut value = |name: &str| -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--seed" => seed = parse_u64(&value("--seed")?)?,
            "--size" => {
                let requested = parse_u32(&value("--size")?)?;
                if requested == 0 || requested > WORLD_CELLS {
                    return Err(format!("--size must be 1..={WORLD_CELLS}, got {requested}"));
                }
                size = requested as usize;
            }
            other => return Err(format!("unknown argument {other:?}; try --help")),
        }
        index += 1;
    }

    let generator = MacroGenerator::new(seed);
    let half_cell = CELL_SIZE_M as i64 / 2;
    let started = Instant::now();
    let mut heights = Vec::with_capacity(size * size);
    for z in 0..size {
        for x in 0..size {
            let world_x = (x as i64 * CELL_SIZE_M as i64 + half_cell) << 16;
            let world_z = (z as i64 * CELL_SIZE_M as i64 + half_cell) << 16;
            heights.push(height_from_q16(generator.surface(world_x, world_z)));
        }
    }
    let build_time = started.elapsed();

    let started = Instant::now();
    let hydro = Hydrology::compute(heights, size, size, SEA_LEVEL_UNITS);
    let hydro_time = started.elapsed();

    let mut hash = fnv_start();
    for &value in hydro.filled() {
        hash = fnv_u64(hash, value as u64);
    }
    for &value in hydro.accumulation() {
        hash = fnv_u64(hash, value as u64);
    }

    let cells = (size * size) as f64;
    println!(
        "hydro seed={seed} cells={} sea_level_units={SEA_LEVEL_UNITS} build_ms={} hydro_ms={} outlets={} lake_cells={} lake_fraction={:.4} max_fill={} max_accumulation={}",
        size * size,
        build_time.as_millis(),
        hydro_time.as_millis(),
        hydro.outlets(),
        hydro.lake_cells(),
        hydro.lake_cells() as f64 / cells,
        hydro.max_fill_depth(),
        hydro.max_accumulation(),
    );
    println!("hydro_hash={hash:016x}");
    Ok(())
}

fn parse_u64(text: &str) -> Result<u64, String> {
    let parsed = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        text.parse::<u64>()
    };
    parsed.map_err(|error| format!("bad number {text:?}: {error}"))
}

fn parse_u32(text: &str) -> Result<u32, String> {
    text.parse::<u32>()
        .map_err(|error| format!("bad count {text:?}: {error}"))
}
