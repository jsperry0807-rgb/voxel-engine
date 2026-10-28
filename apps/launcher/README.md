# Voxel Launcher

One window for the things this repository is normally driven by on the command
line: the viewer, the texture editor and its pipeline, the checks, the
benchmarks, and the Linux and Windows builds.

```sh
cargo launcher                       # alias defined in .cargo/config.toml
cargo run -p voxel-launcher          # same thing, spelled out
```

The launcher finds the workspace root by walking up from the working directory
(or from its own manifest), so it works from any subdirectory. Pass
`--root PATH` or set `VOXEL_ROOT` to override that.

## What the window does

| Area | Actions |
| --- | --- |
| Run | Viewer on Linux (`cargo run -p voxel-viewer`), viewer from the Windows exe, the texture editor |
| Textures | `check`, `generate`, `bake-atlas`, `update-all` from the texgen pipeline |
| Verify | fmt, clippy, workspace tests, texture manifest test, shader tests, chunk constants |
| Bench | `gate report`, the full `scripts/bench.sh` suite |
| Build | the Linux workspace build and the Windows cross-build |

Every action is a background job. Its command, combined stdout/stderr, exit
status and elapsed time stay in the output panel; Stop kills the whole process
group, so it also closes a viewer that `cargo run` already handed off to.

The viewer options in the Run section set the same environment variables the
viewer reads: `VOXEL_VIEWER_ATLAS`, `RUST_LOG` and `VOXEL_VIEWER_WAYLAND`. The
committed atlas is used when the atlas field is empty; X11/XWayland stays the
default pointer backend.

## Linux is the default, Windows is kept warm

```sh
cargo build          # Linux, exactly as before
cargo test           # Linux, exactly as before
cargo build-win      # Windows cross-build, same as the Build button
```

Cargo refuses to let a config alias shadow `build`, so the launcher owns the
background Windows build instead of hooking Cargo:

- On launch it runs `cargo build --workspace --target x86_64-pc-windows-gnu` in
  the background. The status chip in the header shows building/ready/failed.
- The exe lands in `target/x86_64-pc-windows-gnu/debug/`. `Open viewer ·
  Windows` runs it directly (through WSL interop when the launcher is on Linux),
  and `Copy exe path` gives a path to paste into a Windows terminal.
- Atlas paths are translated with `wslpath` for that launch, because a Linux
  path means nothing to the Windows process.
- Set `VOXEL_LAUNCHER_NO_AUTOBUILD=1` (or untick *Build Windows on launch*) to
  skip the automatic build.

The launcher takes X11 through XWayland, the same rule the viewer follows,
because the Wayland/EGL path can fail to create a usable surface on WSLg. Set
`VOXEL_VIEWER_WAYLAND=1` to use the native backend.

The `windows-gnu` target needs `mingw-w64` (`x86_64-w64-mingw32-gcc`) and the
Rust target:

```sh
sudo apt install mingw-w64
rustup target add x86_64-pc-windows-gnu
```

Without the linker the launcher reports it once and leaves the Windows buttons
manual. A cross-build only proves that the code compiles and links for Windows;
windowing, pointer capture and GPU behaviour still have to be tried on the
machine itself.
