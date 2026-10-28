//! `bake-atlas`: validate the texture manifest and write the material atlas.
//!
//! The implementation lives in `voxel-texgraph` so the generator app and this
//! command share one encoder, one downsample filter and one hashing rule.

use std::error::Error;
use std::path::PathBuf;

use voxel_texgraph::Workspace;
use voxel_texgraph::bake as pipeline_bake;

/// Parses `bake-atlas` arguments and runs the bake.
pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let mut check = false;
    let mut root: Option<PathBuf> = None;
    let mut args = args.iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--check" => check = true,
            "--root" => {
                root = Some(PathBuf::from(args.next().ok_or("--root needs a path")?));
            }
            "-h" | "--help" => {
                println!(
                    "usage: voxel-viewer bake-atlas [options]\n\
                     \n\
                     --check       compare against the committed atlas instead of writing\n\
                     --root PATH   repository root (default: search upward)"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`").into()),
        }
    }
    let workspace = match root {
        Some(root) => Workspace::at(root)?,
        None => Workspace::locate()?,
    };
    let message = pipeline_bake::bake_atlas(&workspace, check)?;
    println!("{message}");
    Ok(())
}
