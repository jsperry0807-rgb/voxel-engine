//! The `texgen` application: GUI by default, CLI for CI.
//!
//! ```text
//! cargo run -p voxel-texgen                 # open the editor
//! cargo run -p voxel-texgen -- check        # verify committed bytes
//! cargo run -p voxel-texgen -- generate     # write textures, manifest, materials
//! cargo run -p voxel-texgen -- generate --key stone   # one face only
//! cargo run -p voxel-texgen -- update-all   # generate + bake the atlas
//! cargo run -p voxel-texgen -- bake-atlas [--check]
//! cargo run -p voxel-texgen -- bake-set resourcepack [--check]
//! ```

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use voxel_texgen::Workspace;
use voxel_texgen::bake;
use voxel_texgen::local_set;
use voxel_texgen::pipeline;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut root: Option<PathBuf> = None;
    if let Some(index) = args.iter().position(|arg| arg == "--root") {
        if index + 1 >= args.len() {
            eprintln!("texgen: --root needs a path");
            return ExitCode::FAILURE;
        }
        root = Some(PathBuf::from(args.remove(index + 1)));
        args.remove(index);
    }

    // `--key` is repeatable and only meaningful for the generate commands.
    let mut keys: Vec<String> = Vec::new();
    while let Some(index) = args.iter().position(|arg| arg == "--key") {
        if index + 1 >= args.len() || args[index + 1].starts_with('-') {
            eprintln!("texgen: --key needs a face name");
            return ExitCode::FAILURE;
        }
        keys.push(args.remove(index + 1));
        args.remove(index);
    }

    let workspace = match root {
        Some(root) => Workspace::at(root),
        None => Workspace::locate(),
    };
    let workspace = match workspace {
        Ok(workspace) => workspace,
        Err(error) => {
            report(&error);
            return ExitCode::FAILURE;
        }
    };

    let command = args.first().map(String::as_str).unwrap_or("gui");
    // `bake-set` takes exactly one positional argument; every other command
    // takes flags only.
    if command != "bake-set" {
        if let Some(unknown) = args
            .iter()
            .skip(1)
            .find(|arg| arg.starts_with('-') && arg.as_str() != "--check")
        {
            eprintln!("texgen: unknown flag `{unknown}`");
            print_help();
            return ExitCode::FAILURE;
        }
        if let Some(extra) = args.iter().skip(1).find(|arg| !arg.starts_with('-')) {
            eprintln!("texgen: unexpected argument `{extra}`");
            return ExitCode::FAILURE;
        }
    } else if let Some(extra) = args.iter().skip(2).find(|arg| !arg.starts_with('-')) {
        eprintln!("texgen: unexpected argument `{extra}`");
        return ExitCode::FAILURE;
    }
    // `--check` verifies instead of writing for every command that can write.
    let check = args.iter().skip(1).any(|arg| arg == "--check");
    if !keys.is_empty()
        && !matches!(
            command,
            "generate" | "update-manifest" | "update-all" | "all"
        )
    {
        eprintln!("texgen: --key only applies to generate and update-all");
        return ExitCode::FAILURE;
    }
    let result = match command {
        "gui" | "edit" => {
            if check {
                eprintln!("texgen: --check does not apply to the editor");
                return ExitCode::FAILURE;
            }
            return run_gui(workspace);
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(String::new())
        }
        "list" => {
            for (key, slot) in workspace.faces().iter() {
                println!("{slot:3}  {key}");
            }
            Ok(String::new())
        }
        "generate" | "update-manifest" => match (check, keys.is_empty()) {
            (true, true) => pipeline::check_all(&workspace),
            (true, false) => pipeline::check_keys(&workspace, &keys),
            (false, true) => pipeline::generate_all(&workspace),
            (false, false) => pipeline::generate_keys(&workspace, &keys),
        },
        "materials" => {
            if check {
                pipeline::check_all(&workspace)
            } else {
                pipeline::write_materials(&workspace)
            }
        }
        "update-all" | "all" => match (check, keys.is_empty()) {
            (true, true) => pipeline::check_all(&workspace),
            (true, false) => pipeline::check_keys(&workspace, &keys),
            (false, true) => pipeline::update_all(&workspace),
            (false, false) => pipeline::generate_keys(&workspace, &keys),
        },
        "check" => pipeline::check_all(&workspace),
        "bake-atlas" => bake::bake_atlas(&workspace, check).map_err(pipeline::PipelineError::Bake),
        "bake-set" => {
            let Some(set) = args.get(1) else {
                eprintln!("texgen: bake-set needs a set name, for example `bake-set resourcepack`");
                return ExitCode::FAILURE;
            };
            local_set::bake_set(&workspace, set, check).map_err(pipeline::PipelineError::Bake)
        }
        other => {
            eprintln!("texgen: unknown command `{other}`");
            print_help();
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(message) => {
            if !message.is_empty() {
                println!("{message}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            report(&error);
            ExitCode::FAILURE
        }
    }
}

fn run_gui(workspace: Workspace) -> ExitCode {
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Voxel Texgen")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1080.0, 720.0]),
        ..Default::default()
    };
    voxel_app_support::prefer_x11_event_loop(&mut options);
    let result = eframe::run_native(
        "Voxel Texgen",
        options,
        Box::new(move |cc| Ok(Box::new(voxel_texgen::app::TexgenApp::new(cc, workspace)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("texgen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "voxel-texgen: node-graph texture and material generator\n\
         \n\
         usage: texgen [--root PATH] [command]\n\
         \n\
         commands:\n\
           gui               open the editor (default)\n\
           list              list the committed face keys and slots\n\
           generate          render graphs to PNGs, then write textures.ron and materials.gen.ron\n\
           update-manifest   alias for generate\n\
           materials         write materials.gen.ron only\n\
           update-all        generate + bake-atlas\n\
           bake-atlas        write the 256x256 atlas and atlas.hash.ron\n\
           bake-set NAME     bake assets/textures/NAME into assets/atlas/NAME\n\
           check             fail if any committed byte differs\n\
         \n\
         options:\n\
           --root PATH       repository root (default: search upward)\n\
           --key FACE        generate/update-all only this face (repeatable)\n\
           --check           verify instead of writing (bake-atlas, bake-set)"
    );
}

fn report(error: &dyn Error) {
    eprintln!("texgen: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}
