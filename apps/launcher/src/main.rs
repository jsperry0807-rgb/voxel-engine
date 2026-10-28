//! `voxel-launcher`: one window for running the viewer, the texture tools, the
//! checks and the builds.
//!
//! The launcher exists so a normal session needs no command line. It discovers
//! the workspace root, starts the Windows cross-build in the background, and
//! offers the actions this repository is driven by.
//!
//! ```text
//! cargo launcher                     # alias from .cargo/config.toml
//! cargo run -p voxel-launcher        # same thing, spelled out
//! cargo run -p voxel-launcher -- --root /path/to/voxel-approved
//! ```
//!
//! The Linux build is the default test surface (`cargo build`, `cargo test`).
//! The launcher also keeps a `x86_64-pc-windows-gnu` build warm so the same
//! changes can be tried natively on Windows, where WSL cannot reproduce every
//! windowing and pointer bug.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod app;
mod jobs;

fn main() -> ExitCode {
    let mut root: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => match args.next() {
                Some(path) => root = Some(PathBuf::from(path)),
                None => {
                    eprintln!("launcher: --root needs a path");
                    return ExitCode::FAILURE;
                }
            },
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("launcher: unknown argument `{other}`");
                print_help();
                return ExitCode::FAILURE;
            }
        }
    }

    let root = match root {
        Some(root) => match validate_root(root) {
            Ok(root) => root,
            Err(error) => {
                eprintln!("launcher: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => match find_root() {
            Ok(root) => root,
            Err(error) => {
                eprintln!("launcher: {error}");
                return ExitCode::FAILURE;
            }
        },
    };

    run(root)
}

/// Opens the launcher window and returns when it closes.
fn run(root: PathBuf) -> ExitCode {
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Voxel Launcher")
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    prefer_x11(&mut options);
    let result = eframe::run_native(
        "Voxel Launcher",
        options,
        Box::new(move |cc| Ok(Box::new(app::LauncherApp::new(cc, root)))),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("launcher: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prefers X11 through XWayland, the same rule the viewer follows.
///
/// eframe takes Wayland whenever `WAYLAND_DISPLAY` is set. On WSLg the
/// Wayland/EGL path can fail to create a usable surface, while XWayland works.
/// Set `VOXEL_VIEWER_WAYLAND=1` to keep the native backend.
fn prefer_x11(options: &mut eframe::NativeOptions) {
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;

        if std::env::var_os("VOXEL_VIEWER_WAYLAND").is_none()
            && std::env::var_os("DISPLAY").is_some()
        {
            options.event_loop_builder = Some(Box::new(|builder| {
                builder.with_x11();
            }));
        }
    }
    // The parameter is unused on hosts where the block above is compiled out.
    let _ = options;
}

/// Finds the workspace root from `VOXEL_ROOT`, the working directory, or the
/// launcher's own manifest directory.
fn find_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("VOXEL_ROOT") {
        return validate_root(PathBuf::from(root));
    }
    let mut starts = Vec::new();
    if let Ok(directory) = std::env::current_dir() {
        starts.push(directory);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for start in starts {
        let mut directory = start.as_path();
        loop {
            if is_root(directory) {
                return Ok(directory.to_path_buf());
            }
            match directory.parent() {
                Some(parent) => directory = parent,
                None => break,
            }
        }
    }
    Err("could not find the workspace root; run from the repository or pass --root".to_owned())
}

/// Accepts an explicit root only when it looks like this workspace.
fn validate_root(root: PathBuf) -> Result<PathBuf, String> {
    if is_root(&root) {
        return Ok(root);
    }
    Err(format!(
        "{} is not the Voxel-Stuff workspace root",
        root.display()
    ))
}

/// A workspace root contains the launcher and the viewer crate.
fn is_root(directory: &Path) -> bool {
    directory.join("apps/launcher/Cargo.toml").is_file()
        && directory.join("crates/engine/viewer/Cargo.toml").is_file()
}

fn print_help() {
    println!(
        "voxel-launcher: desktop launcher for Voxel-Stuff\n\
         \n\
         usage: launcher [--root PATH]\n\
         \n\
         options:\n\
           --root PATH   workspace root (default: search upward from the working directory)\n\
           -h, --help    print this help\n\
         \n\
         The window runs the viewer and texture editor, the texture and check\n\
         pipelines, and the Linux and Windows builds. The Windows cross-build\n\
         starts automatically unless VOXEL_LAUNCHER_NO_AUTOBUILD is set.\n\
         The launcher prefers X11/XWayland, like the viewer; set\n\
         VOXEL_VIEWER_WAYLAND=1 to use the native backend."
    );
}
