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
| Run | Viewer on Linux (`cargo run -p voxel-viewer`), viewer from the Windows exe, the texture editor, the Voxgen/resource-pack texture-source toggle, and the 16²/32²/64²/128²/256² texture density |
| Textures | `check`, `generate`, `bake-atlas`, `update-all` from the texgen pipeline, plus `bake-set` for local resource packs |
| Verify | fmt, clippy, workspace tests, texture manifest test, shader tests, chunk constants |
| Bench | `gate report`, the full `scripts/bench.sh` suite |
| Build | the Linux workspace build, the Windows cross-build, or both at once |

Every action is a background job. Its command, combined stdout/stderr, exit
status and elapsed time stay in the output panel; Stop kills the whole process
group, so it also closes a viewer that `cargo run` already handed off to. The
output text scales with the window (15–20 pt) so a maximised launcher stays
readable without crushing long lines.

The viewer options in the Run section set the same environment variables the
viewer reads: `VOXEL_VIEWER_ATLAS`, `RUST_LOG` and `VOXEL_VIEWER_WAYLAND`.
Texture source picks the atlas: *Voxgen* leaves the override empty so the
committed procedural atlas is used, *Resource pack* points at
`assets/atlas/resourcepack`, and *Custom path* uses the field below. X11/XWayland
stays the default pointer backend.

*Texture density* sets `VOXEL_VIEWER_TILE` to 16, 32, 64, 128 or 256 texels per
1 m face. It is enabled for *Voxgen*: 16 uses the committed atlas, and larger
values rebuild the same 16×16-slot atlas from the committed 256² sources at
startup (256 keeps every source pixel, at 4096² / ~85 MiB of VRAM). A resource
pack or custom atlas keeps the density that was baked into its files, and the
viewer logs that the override was ignored.

## Voxgen vs resource-pack textures

Both sides are 256² face sources that the same box filter bakes into the
viewer's atlas tiles, so switching sources compares like with like. The
committed bake stays at 16² tiles; the launcher's *Texture density* only
changes the view-time rebuild of the Voxgen set.

```text
assets/textures/voxgen/        generated faces (copy of the committed set)
assets/textures/resourcepack/  resource-pack faces; local only, never committed
assets/atlas/voxgen/           standby bake of the generated set
assets/atlas/resourcepack/     atlas-0.png … atlas-4.png written by bake-set
```

Both sets bake through the same command, so either can be on standby:

```sh
cargo run -p voxel-texgen -- bake-set voxgen
cargo run -p voxel-texgen -- bake-set resourcepack
```

The Voxgen base level is byte-identical to the committed `atlas.png`; the
launcher still uses the committed, hash-verified atlas for *Voxgen*.

`assets/` is gitignored on purpose: resource packs are almost always
all-rights-reserved, so they cannot ship with the repository. Create the set by
copying the pack's `assets/minecraft/textures/block/*.png` into
`assets/textures/resourcepack/` (flat, one PNG per face). Faces the pack does
not include have to be supplied from another source; `texgen bake-set` names
every missing file instead of baking a partial atlas.

Then either press *Bake resource pack atlas* in the launcher or run:

```sh
cargo run -p voxel-texgen -- bake-set resourcepack
cargo run -p voxel-texgen -- bake-set resourcepack --check
```

Finally set *Texture source* to *Resource pack* and open a viewer. The viewer
logs the local atlas as unverified because, unlike the committed atlas, it has
no manifest hash.

## Linux and Windows stay warm

```sh
cargo build          # Linux, exactly as before
cargo test           # Linux, exactly as before
cargo build-win      # Windows cross-build, same as the Build button
```

Both targets share one Cargo build-directory lock. The Windows job starts after
the Linux job finishes (or after it yields the lock), so treat *Build both* as
"queue both", not "run both in parallel" — the second job waits with
`Blocking waiting for file lock on build directory` until the first is done.
The launcher still starts them as two independent jobs, so either can be
stopped without cancelling the other.

Cargo refuses to let a config alias shadow `build`, so the launcher owns the
background builds instead of hooking Cargo:

- On launch it starts `cargo build --workspace`, and the Windows job only when
  the cross toolchain is already present (a missing toolchain never triggers a
  package install at startup). The status chip in the header shows the Windows
  job; the status bar shows Linux.
- The Windows cross-build needs mingw-w64 and the `x86_64-pc-windows-gnu` Rust
  target. `cargo build-win` checks both and fails with the `cargo setup-win`
  instruction; `cargo setup-win` installs. The launcher does the same from
  *Build Windows* or *Install Windows toolchain*. On a native Windows host the
  *Windows build* is simply `cargo build --workspace`.
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
cargo build-win      # preflight and cross-build; never installs by itself
cargo xtask doctor   # report only; exits 1 if something is missing
```

`build-win` checks the prerequisites and fails with the `cargo setup-win`
instruction when something is missing; it only installs when you pass
`--install`. The launcher's *Build Windows* button runs `setup-win` first and
continues with the build once the toolchain is present, but the launcher's
autostart never installs packages. CI installs the same packages and runs
`cargo build-win`, so a broken cross-build cannot merge unnoticed.

If the installer cannot run (for example sudo needs a terminal the launcher
does not have), the launcher reports it and the Windows buttons are enabled
again as soon as the toolchain is present. A cross-build only proves that the
code compiles and links for Windows; windowing, pointer capture and GPU
behaviour still have to be tried on the machine itself.
