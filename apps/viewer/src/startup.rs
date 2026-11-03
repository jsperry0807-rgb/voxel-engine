//! Startup: event loop, atlas loading, input bindings and the composition root.
//!
//! Everything here reads the environment once, before the first frame. The
//! running application is [`crate::app::Viewer`].

use std::path::{Path, PathBuf};

use voxel_profiling::HITCH_NS;
use winit::event_loop::EventLoop;

use crate::app::{FrameTiming, Viewer};
use crate::atlas::{Atlas, AtlasLayout, MipLevel, TILE_SIZE, TILE_SIZES};
use crate::error::ViewerError;
use crate::frame_profile::FrameProfiler;
use crate::input::{Action, Bindings, Input, LookOptions};
use crate::look::load_mouse_mode;
use crate::tracker::Tracker;

/// Builds the event loop.
///
/// Wayland pointer capture is unreliable across compositors (the lock can
/// report success before it is active, and relative motion then stops at the
/// screen edge). Prefer X11 through XWayland when it is available; set
/// `VOXEL_VIEWER_WAYLAND=1` to force the native backend.
fn build_event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    let mut builder = EventLoop::builder();
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;

        if voxel_app_support::prefer_x11() {
            builder.with_x11();
            log::info!("using X11/XWayland (set VOXEL_VIEWER_WAYLAND=1 for Wayland)");
        }
    }
    builder.build()
}

/// The runtime tile density named by `VOXEL_VIEWER_TILE`.
///
/// Accepted values are 16, 32, 64, 128 and 256 texels per tile. The value is a
/// tile edge, not an atlas edge: 256 means 256² texels per 1 m face, built from
/// the committed 256² sources without any source loss. Unset or invalid means
/// the committed 16-texel atlas, with a warning for invalid input.
pub(crate) fn tile_layout_from_env() -> AtlasLayout {
    let Some(value) = std::env::var_os("VOXEL_VIEWER_TILE") else {
        return AtlasLayout::DEFAULT;
    };
    let value = value.to_string_lossy();
    match AtlasLayout::parse(&value) {
        Some(layout) => layout,
        None => {
            log::warn!(
                "VOXEL_VIEWER_TILE={value:?} is not one of {TILE_SIZES:?}; using {}",
                AtlasLayout::DEFAULT.tile_size()
            );
            AtlasLayout::DEFAULT
        }
    }
}

/// Builds a denser atlas from the committed 256² sources.
///
/// The sources and the manifest live in the repository, so this needs to run
/// from a checkout (the same assumption the default relative atlas path
/// already makes). Every source is hash-verified against `textures.ron` by the
/// shared texgraph loader.
fn build_dense_atlas(layout: AtlasLayout) -> Result<Atlas, ViewerError> {
    let workspace = voxel_texgraph::pipeline::Workspace::locate().map_err(|error| {
        ViewerError::Atlas(format!(
            "VOXEL_VIEWER_TILE={} needs the committed texture sources: {error}; \
             run the viewer from the repository, unset VOXEL_VIEWER_TILE, \
             or pass 16 to use the committed atlas",
            layout.tile_size()
        ))
    })?;
    let levels = voxel_texgraph::bake::build_levels_at(&workspace, layout)
        .map_err(|error| ViewerError::Atlas(format!("dense atlas bake failed: {error}")))?;
    let levels = levels
        .into_iter()
        .map(|level| {
            MipLevel::new(level.width, level.height, level.pixels)
                .map_err(|error| ViewerError::Atlas(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Atlas::from_levels(layout, levels)
        .map_err(|error| ViewerError::Atlas(format!("dense atlas layout: {error}")))
}

/// The committed atlas path, relative to the repository root.
const COMMITTED_ATLAS: &str = "apps/viewer/assets/atlas/atlas.png";

/// Loads and verifies the committed atlas, or builds the requested density.
///
/// `VOXEL_VIEWER_ATLAS` may name either `atlas.png` or a directory. A
/// directory that holds `atlas-0.png`… is a local bake (for example
/// `assets/atlas/resourcepack`) and is logged as unverified. The committed
/// atlas is hash-verified. An explicit atlas path keeps its own density; the
/// `VOXEL_VIEWER_TILE` override only rebuilds the committed Voxgen atlas.
pub(crate) fn load_startup_atlas(layout: AtlasLayout) -> Result<Atlas, ViewerError> {
    if let Some(value) = std::env::var_os("VOXEL_VIEWER_ATLAS") {
        let candidate = PathBuf::from(value);
        if let Some(atlas) = load_atlas_path(&candidate, layout)? {
            return Ok(atlas);
        }
        log::warn!(
            "VOXEL_VIEWER_ATLAS={} has no atlas; falling back",
            candidate.display()
        );
    }
    if layout.tile_size() != TILE_SIZE {
        log::info!(
            "building {}-texel tiles from the committed 256² sources",
            layout.tile_size()
        );
        let atlas = build_dense_atlas(layout)?;
        log::info!(
            "dense atlas: {}² ({:.1} MiB, {} mips)",
            atlas.atlas_size(),
            atlas.byte_len() as f64 / (1024.0 * 1024.0),
            atlas.mip_count()
        );
        return Ok(atlas);
    }
    if let Some(atlas) = load_atlas_path(Path::new(COMMITTED_ATLAS), layout)? {
        return Ok(atlas);
    }
    log::error!("no committed atlas found at {COMMITTED_ATLAS}");
    Err(ViewerError::Atlas(format!("missing {COMMITTED_ATLAS}")))
}

/// Loads one atlas path, or `None` when the path holds no atlas.
///
/// A directory holding `atlas-0.png`… is a local bake; anything else is tried
/// as a single `atlas.png` (or as a directory containing one).
fn load_atlas_path(candidate: &Path, layout: AtlasLayout) -> Result<Option<Atlas>, ViewerError> {
    if candidate.is_dir() && candidate.join("atlas-0.png").is_file() {
        let atlas = Atlas::load_dir(candidate).map_err(|error| {
            log::error!("atlas load failed at {}: {error}", candidate.display());
            ViewerError::Atlas(error.to_string())
        })?;
        log::warn!(
            "using local unverified atlas from {} ({} mips, {}-texel tiles)",
            candidate.display(),
            atlas.mip_count(),
            atlas.tile_size()
        );
        warn_density_ignored(layout, atlas.tile_size());
        return Ok(Some(atlas));
    }
    let path = if candidate.is_dir() {
        candidate.join("atlas.png")
    } else {
        candidate.to_owned()
    };
    if !path.is_file() {
        return Ok(None);
    }
    // A custom standalone PNG or directory has no committed hash file; load it
    // as a local, unverified atlas instead of failing startup.
    let atlas = if path.with_file_name("atlas.hash.ron").is_file() {
        let atlas = Atlas::load_verified(&path).map_err(|error| {
            log::error!("atlas verification failed at {}: {error}", path.display());
            ViewerError::Atlas(error.to_string())
        })?;
        log::info!(
            "loaded verified atlas from {} ({} mips, {}-texel tiles)",
            path.display(),
            atlas.mip_count(),
            atlas.tile_size()
        );
        atlas
    } else {
        let atlas = Atlas::load(&path).map_err(|error| {
            log::error!("atlas load failed at {}: {error}", path.display());
            ViewerError::Atlas(error.to_string())
        })?;
        log::warn!(
            "using local unverified atlas from {} ({} mips, {}-texel tiles)",
            path.display(),
            atlas.mip_count(),
            atlas.tile_size()
        );
        warn_density_ignored(layout, atlas.tile_size());
        atlas
    };
    Ok(Some(atlas))
}

/// Logs when an explicit atlas keeps a density other than the requested one.
fn warn_density_ignored(requested: AtlasLayout, actual: u32) {
    if requested.tile_size() != actual {
        log::warn!(
            "VOXEL_VIEWER_TILE={} is ignored: the selected atlas has {actual}-texel tiles",
            requested.tile_size()
        );
    }
}

/// Creates the event loop and runs the viewer until the window closes.
pub fn run() -> Result<(), ViewerError> {
    let event_loop = build_event_loop()?;
    let (input, look_options, sensitivity_explicit) = load_input();
    let mut viewer = Viewer {
        input,
        look_options,
        sensitivity_explicit,
        mouse_mode: load_mouse_mode(),
        tracker: Tracker::from_env(),
        timing: FrameTiming::new(FrameProfiler::from_env()),
        ..Viewer::default()
    };
    event_loop.run_app(&mut viewer)?;
    if let Some(stats) = viewer.timing.finish() {
        log::info!(
            "frame profile: {} frames | {:.1} fps avg | {:.1} fps 1% low | \
             {:.2} ms max | {} hitches > {:.1} ms",
            stats.frames,
            stats.avg_fps,
            stats.one_percent_low_fps,
            stats.max_ns as f64 / 1e6,
            stats.hitches,
            HITCH_NS as f64 / 1e6,
        );
    }
    match viewer.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The bindings named by `VOXEL_BINDINGS`, or the defaults, plus the look
/// options.
///
/// `VOXEL_SENSITIVITY` (0.0 to 1.0), `VOXEL_SMOOTH_CAMERA`, `VOXEL_INVERT_X`
/// and `VOXEL_INVERT_Y` configure mouse look; `VOXEL_TOGGLE_SPRINT=1` makes
/// sprint latch instead of hold. There is no rebinding screen yet, so the
/// bindings file is written by hand; a missing or partly invalid file falls
/// back to the defaults with a log line, never a failure.
fn load_input() -> (Input, LookOptions, bool) {
    let (look_options, sensitivity_explicit) = load_look_options();
    let mut input = Input::with_look(load_bindings(), look_options);
    if env_flag("VOXEL_TOGGLE_SPRINT") {
        input.set_toggleable(Action::Sprint);
    }
    (input, look_options, sensitivity_explicit)
}

/// The bindings in the file named by `VOXEL_BINDINGS`, or the defaults.
fn load_bindings() -> Bindings {
    let Some(path) = std::env::var_os("VOXEL_BINDINGS") else {
        return Bindings::default();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            log::warn!(
                "cannot read bindings {}: {error}; using defaults",
                Path::new(&path).display()
            );
            return Bindings::default();
        }
    };
    let (bindings, warnings) = Bindings::from_text(&text);
    for warning in warnings {
        log::warn!("bindings {}: {warning}", Path::new(&path).display());
    }
    log::debug!("bindings:\n{}", bindings.to_text());
    bindings
}

/// Mouse-look options from the environment, falling back to the defaults.
///
/// Also reports whether `VOXEL_SENSITIVITY` was given, so the confined-pointer
/// cap can leave an explicit value alone.
fn load_look_options() -> (LookOptions, bool) {
    let mut options = LookOptions::default();
    let explicit = env_f64("VOXEL_SENSITIVITY");
    if let Some(sensitivity) = explicit {
        options.sensitivity = sensitivity;
    }
    options.smooth = env_flag("VOXEL_SMOOTH_CAMERA");
    options.invert_x = env_flag("VOXEL_INVERT_X");
    options.invert_y = env_flag("VOXEL_INVERT_Y");
    (options, explicit.is_some())
}

/// Reads `name` as a number, logging and ignoring an unparsable value.
fn env_f64(name: &str) -> Option<f64> {
    let value = std::env::var(name).ok()?;
    match value.parse() {
        Ok(number) => Some(number),
        Err(_) => {
            log::warn!("{name}={value:?} is not a number; ignoring");
            None
        }
    }
}

/// Whether `name` is set to `1`, `true` or `yes`.
fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "yes" | "on")
    )
}
