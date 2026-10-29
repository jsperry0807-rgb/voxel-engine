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
| Build | the Linux workspace build, the Windows cross-build, or both at once |

Every action is a background job. Its command, combined stdout/stderr, exit
status and elapsed time stay in the output panel; Stop kills the whole process
group, so it also closes a viewer that `cargo run` already handed off to. The
output text scales with the window (15–20 pt) so a maximised launcher stays
readable without crushing long lines.

The viewer options in the Run section set the same environment variables the
viewer reads: `VOXEL_VIEWER_ATLAS`, `RUST_LOG` and `VOXEL_VIEWER_WAYLAND`. The
committed atlas is used when the atlas field is empty; X11/XWayland stays the
default pointer backend.

## Linux and Windows stay warm

```sh
cargo build          # Linux, exactly as before
cargo test           # Linux, exactly as before
cargo build-win      # Windows cross-build, same as the Build button
```

Both targets can build at the same time. Cargo keeps separate locks for
`target/debug` and `target/x86_64-pc-windows-gnu/debug`, so the target-specific
work overlaps; only shared host build scripts can briefly wait on Cargo's build
lock. The *Build both* button starts the two jobs together.

Cargo refuses to let a config alias shadow `build`, so the launcher owns the
background builds instead of hooking Cargo:

- On launch it starts `cargo build --workspace` and
  `cargo build --workspace --target x86_64-pc-windows-gnu` as two jobs. The
  status chip in the header shows the Windows build; the status bar shows
  Linux.
- The Windows cross-build needs mingw-w64 and the `x86_64-pc-windows-gnu` Rust
  target. `cargo build-win` checks both and installs whatever is missing before
  it builds; `cargo setup-win` performs only the setup. The launcher does the
  same from *Build Windows* or *Install Windows toolchain*.
- The exe lands in `target/x86_64-pc-windows-gnu/debug/`. `Open viewer ·
  Windows` runs it directly (through WSL interop when the launcher is on Linux),
  and `Copy exe path` gives a path to paste into a Windows terminal.
- Atlas paths are translated with `wslpath` for that launch, because a Linux
  path means nothing to the Windows process.
- Set `VOXEL_LAUNCHER_NO_AUTOBUILD=1` (or untick *Build Linux + Windows on
  launch*) to skip both automatic builds.

The launcher takes X11 through XWayland, the same rule the viewer follows,
because the Wayland/EGL path can fail to create a usable surface on WSLg. Set
`VOXEL_VIEWER_WAYLAND=1` to use the native backend.

The `windows-gnu` target needs `mingw-w64` (`x86_64-w64-mingw32-*`) and the
Rust target. Both are handled by the preflight:

```sh
cargo setup-win      # install what is missing, then stop
cargo build-win      # same preflight, then the cross-build
cargo xtask doctor   # report only; exits 1 if something is missing
```

`build-win` asks for sudo only when it actually has to install something. Set
`VOXEL_WIN_TOOLCHAIN_NO_INSTALL=1` (or pass `--no-install`) to make it fail
instead. CI installs the same packages and runs `cargo build-win`, so a broken
cross-build cannot merge unnoticed.

If the installer cannot run (for example sudo needs a terminal the launcher
does not have), the launcher reports it and the Windows buttons are enabled
again as soon as the toolchain is present. A cross-build only proves that the
code compiles and links for Windows; windowing, pointer capture and GPU
behaviour still have to be tried on the machine itself.
