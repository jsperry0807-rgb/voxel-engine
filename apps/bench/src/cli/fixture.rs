//! Frozen-fixture commands (`fixture build` / `fixture verify`).

use super::*;

pub(crate) fn fixture_mode(args: &[String]) {
    let Some(command) = args.first() else {
        usage("fixture needs `build` or `verify`");
    };
    match command.as_str() {
        "build" => fixture_build(&args[1..]),
        "verify" => fixture_verify(&args[1..]),
        other => usage(&format!("unknown fixture command `{other}`")),
    }
}

pub(crate) fn fixture_build(args: &[String]) {
    let mut out = PathBuf::from(crate::fixture::DEFAULT_DIR);
    let mut recipe = crate::fixture::Recipe::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                index += 1;
                out = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--out needs a directory")),
                );
            }
            "--chunk-min" => {
                index += 1;
                recipe.chunk_min = parse_point(
                    "--chunk-min",
                    args.get(index)
                        .unwrap_or_else(|| usage("--chunk-min needs x,y,z")),
                );
            }
            "--chunk-max" => {
                index += 1;
                recipe.chunk_max = parse_point(
                    "--chunk-max",
                    args.get(index)
                        .unwrap_or_else(|| usage("--chunk-max needs x,y,z")),
                );
            }
            "--seed" => {
                index += 1;
                recipe.seed = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| usage("--seed needs a u64"));
            }
            other => usage(&format!("unknown fixture build argument `{other}`")),
        }
        index += 1;
    }

    match crate::fixture::build_recipe(&out, &recipe) {
        Ok(built) => {
            let manifest = &built.manifest;
            println!(
                "froze {} at {} ({} chunks, {} bytes, {} files)",
                manifest.id,
                built.save_dir.display(),
                manifest.observed.live_chunks,
                manifest.integrity.total_bytes,
                manifest.integrity.files.len(),
            );
            println!("manifest  {}", built.manifest_path.display());
            println!("blake3    {}", manifest.integrity.world_hash);
            println!(
                "evidence  cave air {}, overhang roofs {}, structures {}, trees {}",
                manifest.checks.cave_air_voxels,
                manifest.checks.overhang_roof_blocks,
                manifest.checks.structure_blocks,
                manifest.checks.tree_blocks,
            );
            println!(
                "status    provisional until the first real save; do not quote its sizes as gate results"
            );
        }
        Err(error) => {
            eprintln!("failed to build fixture: {error}");
            std::process::exit(1);
        }
    }
}

pub(crate) fn fixture_verify(args: &[String]) {
    let mut manifest = crate::fixture::default_manifest();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest" => {
                index += 1;
                manifest = PathBuf::from(
                    args.get(index)
                        .unwrap_or_else(|| usage("--manifest needs a file")),
                );
            }
            other => usage(&format!("unknown fixture verify argument `{other}`")),
        }
        index += 1;
    }

    match crate::fixture::verify(&manifest) {
        Ok(verified) => {
            let manifest = &verified.manifest;
            println!(
                "verified {} at {} ({} chunks, {} bytes, {} files)",
                manifest.id,
                verified.save_dir.display(),
                manifest.observed.live_chunks,
                manifest.integrity.total_bytes,
                manifest.integrity.files.len(),
            );
            println!("manifest  {}", verified.manifest_path.display());
            println!("blake3    {}", manifest.integrity.world_hash);
            println!(
                "format    {} v{}, {}³ chunks, {}",
                manifest.format.chunk_format,
                manifest.format.chunk_version,
                manifest.format.chunk_dim,
                manifest.format.region_format,
            );
            println!(
                "materials {} distinct; cave air {}, overhang roofs {}, structures {}, trees {}",
                manifest.observed.materials.len(),
                manifest.checks.cave_air_voxels,
                manifest.checks.overhang_roof_blocks,
                manifest.checks.structure_blocks,
                manifest.checks.tree_blocks,
            );
            if verified.rebuilt {
                println!("rebuild   byte-identical from the manifest recipe");
            }
            println!("status    provisional until the first real save");
        }
        Err(error) => {
            eprintln!("fixture verify failed: {error}");
            std::process::exit(1);
        }
    }
}
