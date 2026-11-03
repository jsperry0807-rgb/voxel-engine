//! The voxel viewer binary.
//!
//! The viewer opens a window, initializes wgpu for it, keeps the surface
//! configuration in step with the window, and runs a frame loop with a depth
//! buffer. A fly camera responds to the bound movement actions, mouse look and
//! the wheel (which changes the fly speed). Two overlapping debug quads make
//! the depth test visible until the chunk renderer replaces them.
//!
//! ```text
//! cargo run -p voxel-viewer
//! cargo run -p voxel-viewer -- bake-atlas --manifest <path> --out <dir>
//! ```
//!
//! Movement is `W`/`A`/`S`/`D`, `Space`/`Shift` fly vertically and `Ctrl`
//! sprints; the arrow keys look around, `F11` toggles borderless fullscreen,
//! `Esc` frees the pointer (press it again to recapture), and `Q` quits.
//! Bindings can be rebound by pointing `VOXEL_BINDINGS` at a file; see
//! [`voxel_viewer::input`]. `VOXEL_TOGGLE_SPRINT=1` makes sprint latch instead
//! of hold. `VOXEL_TRACK=1` logs yaw/pitch once per second, and
//! `VOXEL_TRACK=look.csv` also writes the per-frame data; see
//! [`voxel_viewer::tracker`].
//! A locked pointer (real desktop) gives Horizon's model: raw motion at the
//! configured sensitivity. Where raw deltas are device units or absent (WSLg's
//! Xwayland) the look follows cursor positions and the sensitivity is capped so
//! a full sweep across the window turns 120°. `VOXEL_MOUSE=raw|cursor` forces
//! one source; the default, `auto`, negotiates.
//!
//! `RUST_LOG` follows `env_logger` syntax. The default shows warnings from
//! dependencies and informational messages from the viewer; for a quiet wgpu
//! run use `RUST_LOG=wgpu_core=warn`. `VOXEL_VIEWER_TILE=16|32|64|128|256`
//! picks the texels per 1 m face: 16 is the committed atlas, and larger values
//! rebuild the same 16×16-slot atlas from the committed 256² sources.
//!
//! The viewer logs an average / 1%-low / max-hitch summary at `info` when it
//! exits. Set `VOXEL_FRAME_PROFILE=1` to also log the per-second FPS line at
//! `info`, or `VOXEL_FRAME_PROFILE=<path>` to write a frame-trace CSV.
//! `cargo run -p voxel-bench -- frame --input <path>` turns that trace into a
//! report against the 60 Hz target.

use std::error::Error;
use std::process::ExitCode;

use voxel_viewer::bake;
use voxel_viewer::startup;

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,voxel_viewer=info"),
    )
    .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(command) = args.first() {
        if command == "bake-atlas" {
            return match bake::run(&args[1..]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    report(error.as_ref());
                    ExitCode::FAILURE
                }
            };
        }
        eprintln!("voxel-viewer: unknown command `{command}`");
        eprintln!("usage: voxel-viewer [bake-atlas [options]]");
        return ExitCode::FAILURE;
    }

    match startup::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error);
            ExitCode::FAILURE
        }
    }
}

/// Prints an error and its source chain.
fn report(error: &dyn Error) {
    eprintln!("voxel-viewer: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}
