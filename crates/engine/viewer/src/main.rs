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
//! run use `RUST_LOG=wgpu_core=warn`.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use voxel_viewer::atlas::Atlas;
use voxel_viewer::bake;
use voxel_viewer::camera::Camera;
use voxel_viewer::error::ViewerError;
use voxel_viewer::gpu::Gpu;
use voxel_viewer::input::{
    Action, Bindings, CONFINED_SWEEP_DEGREES, Input, KeyAction, LookOptions, sensitivity_for_sweep,
};
use voxel_viewer::tracker::{MotionSource, Tracker};
use wgpu::Color;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1920.0, 1080.0);
/// Window title.
const TITLE: &str = "voxel-viewer";
/// Background colour before anything is drawn.
const CLEAR_COLOR: Color = Color {
    r: 0.02,
    g: 0.025,
    b: 0.035,
    a: 1.0,
};
/// How often the averaged frame time is logged.
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// Fly speed in world units per second, before the wheel's speed steps.
const MOVE_SPEED: f32 = 8.0;
/// How much faster a sprint is.
const SPRINT_MULTIPLIER: f32 = 4.0;
/// Radians per second the arrow keys turn the view (about 86°/s and 57°/s).
const KEY_LOOK_YAW: f32 = 1.5;
const KEY_LOOK_PITCH: f32 = 1.0;
/// Fraction of the half-window that is normal 1:1 look. Beyond it the cursor
/// pushes the view around, so a confined pointer can still turn a full circle.
const EDGE_ZONE: f32 = 0.85;
/// Radians per second at the window edge (about 172°/s and 103°/s).
const EDGE_YAW_RATE: f32 = 3.0;
const EDGE_PITCH_RATE: f32 = 1.8;
/// Each wheel step multiplies the fly speed by this.
const SPEED_STEP: f32 = 1.2;
/// Bounds on the wheel's speed steps: about 0.23x to 8.9x of [`MOVE_SPEED`].
const MIN_SPEED_STEPS: i32 = -8;
const MAX_SPEED_STEPS: i32 = 12;
/// A long stall must not teleport the camera across the world.
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
/// How long after the first motion the negotiation waits for the second source
/// before choosing the one it has seen.
const SOURCE_GRACE: Duration = Duration::from_millis(50);
/// Raw deltas may be this many times larger than the cursor deltas for the
/// same movement before they are treated as device units instead of pixels
/// (Xwayland reports them that way).
const RAW_TO_CURSOR_MAX: f64 = 4.0;

/// How mouse motion is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseMode {
    /// Raw motion when it is in pixels, cursor deltas otherwise.
    Auto,
    /// Horizon's model: `DeviceEvent::MouseMotion` only.
    Raw,
    /// `CursorMoved` deltas only.
    Cursor,
}

/// Reads `VOXEL_MOUSE`: `auto` (default), `raw` or `cursor`.
fn load_mouse_mode() -> MouseMode {
    let Ok(value) = std::env::var("VOXEL_MOUSE") else {
        return MouseMode::Auto;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => MouseMode::Auto,
        "raw" => MouseMode::Raw,
        "cursor" | "position" => MouseMode::Cursor,
        other => {
            log::warn!("VOXEL_MOUSE={other:?} is not auto, raw or cursor; using auto");
            MouseMode::Auto
        }
    }
}

/// Picks the source from one frame of both: raw wins when it agrees with the
/// cursor's pixels, and loses when it is in device units.
fn choose_source(raw: (f64, f64), cursor: (f64, f64)) -> MotionSource {
    let raw_len = raw.0.abs() + raw.1.abs();
    let cursor_len = cursor.0.abs() + cursor.1.abs();
    if raw_len <= cursor_len * RAW_TO_CURSOR_MAX + 8.0 {
        MotionSource::Raw
    } else {
        MotionSource::Cursor
    }
}

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

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report(&error);
            ExitCode::FAILURE
        }
    }
}

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

        if std::env::var_os("VOXEL_VIEWER_WAYLAND").is_none()
            && std::env::var_os("DISPLAY").is_some()
        {
            builder.with_x11();
            log::info!("using X11/XWayland (set VOXEL_VIEWER_WAYLAND=1 for Wayland)");
        }
    }
    builder.build()
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

/// Loads and verifies the committed atlas.
///
/// `VOXEL_VIEWER_ATLAS` may name either `atlas.png` or a directory. The
/// committed atlas is hash-verified; a temporary `.temp/atlas` directory with
/// `atlas-0.png`…`atlas-4.png` is accepted as an explicit local-development
/// fallback and is logged as unverified.
fn load_startup_atlas() -> Result<Atlas, ViewerError> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("VOXEL_VIEWER_ATLAS") {
        candidates.push(PathBuf::from(value));
    }
    candidates.push(PathBuf::from("crates/engine/viewer/assets/atlas/atlas.png"));
    candidates.push(PathBuf::from(".temp/atlas"));
    for candidate in candidates {
        if candidate.is_dir() && candidate.join("atlas-0.png").is_file() {
            return match Atlas::load_dir(&candidate) {
                Ok(atlas) => {
                    log::warn!(
                        "using temporary unverified atlas from {} ({} mips)",
                        candidate.display(),
                        atlas.mip_count()
                    );
                    Ok(atlas)
                }
                Err(error) => {
                    log::error!(
                        "temporary atlas load failed at {}: {error}",
                        candidate.display()
                    );
                    Err(ViewerError::Atlas(error.to_string()))
                }
            };
        }
        let path = if candidate.is_dir() {
            candidate.join("atlas.png")
        } else {
            candidate
        };
        if !path.is_file() {
            continue;
        }
        return match Atlas::load_verified(&path) {
            Ok(atlas) => {
                log::info!(
                    "loaded atlas from {} ({} mips)",
                    path.display(),
                    atlas.mip_count()
                );
                Ok(atlas)
            }
            Err(error) => {
                log::error!("atlas verification failed at {}: {error}", path.display());
                Err(ViewerError::Atlas(error.to_string()))
            }
        };
    }
    log::error!("no committed atlas found at crates/engine/viewer/assets/atlas/atlas.png");
    Err(ViewerError::Atlas(
        "missing crates/engine/viewer/assets/atlas/atlas.png".to_owned(),
    ))
}

/// Creates the event loop and runs the viewer until the window closes.
fn run() -> Result<(), ViewerError> {
    let event_loop = build_event_loop()?;
    let (input, look_options, sensitivity_explicit) = load_input();
    let mut viewer = Viewer {
        input,
        look_options,
        sensitivity_explicit,
        mouse_mode: load_mouse_mode(),
        tracker: Tracker::from_env(),
        ..Viewer::default()
    };
    event_loop.run_app(&mut viewer)?;
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

/// The application state: window, graphics handles, camera and input.
struct Viewer {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    camera: Camera,
    input: Input,
    /// Wheel steps applied to the fly speed; see [`Viewer::fly_speed`].
    speed_steps: i32,
    timing: FrameTiming,
    /// A failure raised inside an event-loop callback.
    ///
    /// Callbacks cannot return an error, so the first one is kept here, the
    /// loop is stopped, and [`run`] reports it after the loop unwinds.
    error: Option<ViewerError>,
    /// Whether mouse look is switched on (the `ToggleCapture` action).
    look_enabled: bool,
    /// Whether the window is borderless fullscreen.
    fullscreen: bool,
    /// Whether the window has focus. Assumed on until the platform says
    /// otherwise, so the first frames work on platforms that send no
    /// `Focused` event at startup.
    focused: bool,
    /// How the look source is chosen.
    mouse_mode: MouseMode,
    /// The source chosen for this capture; `None` while negotiating.
    source: MotionSource,
    /// Raw deltas since the last frame.
    pending_raw: (f64, f64),
    /// Cursor deltas since the last frame.
    pending_cursor: (f64, f64),
    /// Whether each source has moved at all in this capture.
    seen_raw: bool,
    seen_cursor: bool,
    /// When the first motion of this capture arrived, for [`SOURCE_GRACE`].
    motion_started: Option<Instant>,
    /// The grab mode that took, or `None` when no grab was accepted.
    grab_mode: Option<CursorGrabMode>,
    /// The previous `CursorMoved` position.
    last_cursor: Option<PhysicalPosition<f64>>,
    /// Which source fed the current frame, for the tracker.
    last_source: MotionSource,
    /// Mouse-look settings as configured, before the confined-pointer cap.
    look_options: LookOptions,
    /// Whether `VOXEL_SENSITIVITY` set the sensitivity, in which case the
    /// confined-pointer cap leaves it alone.
    sensitivity_explicit: bool,
    /// Yaw/pitch tracker; off unless `VOXEL_TRACK` is set.
    tracker: Tracker,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            window: None,
            gpu: None,
            camera: Camera::default(),
            input: Input::default(),
            speed_steps: 0,
            timing: FrameTiming::default(),
            error: None,
            look_enabled: true,
            fullscreen: false,
            focused: true,
            mouse_mode: MouseMode::Auto,
            source: MotionSource::None,
            pending_raw: (0.0, 0.0),
            pending_cursor: (0.0, 0.0),
            seen_raw: false,
            seen_cursor: false,
            motion_started: None,
            grab_mode: None,
            last_cursor: None,
            last_source: MotionSource::None,
            look_options: LookOptions::default(),
            sensitivity_explicit: false,
            tracker: Tracker::disabled(),
        }
    }
}

impl Viewer {
    /// Applies the input gathered since the previous frame.
    fn apply_input(&mut self, delta: Duration) {
        self.tracker.frame();
        self.flush_motion();
        // Always called, even when the pointer is free: it consumes the
        // motion and advances the smoothed turn.
        if let Some(look) = self.input.look(delta) {
            log::trace!("look: yaw {:.4} pitch {:.4}", look.yaw, look.pitch);
            self.camera.rotate(look.yaw, look.pitch);
            self.tracker.look(self.last_source, look, &self.camera);
        }
        self.last_source = MotionSource::None;

        // Arrow keys look at a fixed rate, so the view can be turned without
        // the pointer: a confined pointer runs out of room at the window edge.
        let mut yaw = 0.0;
        let mut pitch = 0.0;
        if self.input.is_down(Action::LookRight) {
            yaw += 1.0;
        }
        if self.input.is_down(Action::LookLeft) {
            yaw -= 1.0;
        }
        if self.input.is_down(Action::LookUp) {
            pitch += 1.0;
        }
        if self.input.is_down(Action::LookDown) {
            pitch -= 1.0;
        }
        if yaw != 0.0 || pitch != 0.0 {
            let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f32();
            self.camera.rotate(
                yaw * KEY_LOOK_YAW * seconds,
                pitch * KEY_LOOK_PITCH * seconds,
            );
        }
        self.edge_turn(delta);

        let mut direction = Vec3::ZERO;
        if self.input.is_down(Action::Forward) {
            direction += self.camera.forward();
        }
        if self.input.is_down(Action::Back) {
            direction -= self.camera.forward();
        }
        if self.input.is_down(Action::Right) {
            direction += self.camera.right();
        }
        if self.input.is_down(Action::Left) {
            direction -= self.camera.right();
        }
        if self.input.is_down(Action::Up) {
            direction += Vec3::Y;
        }
        if self.input.is_down(Action::Down) {
            direction -= Vec3::Y;
        }
        if direction.length_squared() > 0.0 {
            let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f32();
            let sprint = if self.input.is_down(Action::Sprint) {
                SPRINT_MULTIPLIER
            } else {
                1.0
            };
            self.camera
                .translate(direction.normalize() * self.fly_speed() * sprint * seconds);
        }
    }

    /// Fly speed in world units per second, after the wheel's speed steps.
    fn fly_speed(&self) -> f32 {
        MOVE_SPEED * SPEED_STEP.powi(self.speed_steps)
    }

    /// Handles the actions that fire once per press rather than while held.
    fn handle_edges(&mut self, event_loop: &ActiveEventLoop) {
        if self.input.clicked(Action::Quit) {
            event_loop.exit();
            return;
        }
        while self.input.clicked(Action::ToggleCapture) {
            self.look_enabled = !self.look_enabled;
            log::info!(
                "mouse look {}",
                if self.look_enabled { "on" } else { "off" }
            );
            let window = self.window.clone();
            if let Some(window) = window {
                self.sync_capture(&window);
            }
        }
        if self.input.clicked(Action::ToggleFullscreen) {
            let window = self.window.clone();
            if let Some(window) = window {
                self.fullscreen = !self.fullscreen;
                window.set_fullscreen(self.fullscreen.then_some(Fullscreen::Borderless(None)));
                log::info!("fullscreen {}", if self.fullscreen { "on" } else { "off" });
            }
        }
    }

    /// Applies the pointer state the look switch and focus imply.
    ///
    /// Ported from Horizon's `Game::capture`: grab `Locked` and fall back to
    /// `Confined`; the pointer warps to the window centre so a confined cursor
    /// has room to move in every direction, and leaving look mode warps it
    /// back so it does not reappear at the edge.
    fn sync_capture(&mut self, window: &Window) {
        let want = self.look_enabled && self.focused;
        if want == self.input.captured() {
            return;
        }
        self.input.set_captured(want);
        self.reset_motion_source();
        if want {
            let mut grabbed = None;
            for mode in [CursorGrabMode::Locked, CursorGrabMode::Confined] {
                match window.set_cursor_grab(mode) {
                    Ok(()) => {
                        grabbed = Some(mode);
                        break;
                    }
                    Err(error) => log::debug!("cursor grab {mode:?} unavailable: {error}"),
                }
            }
            window.set_cursor_visible(grabbed.is_none());
            let locked = grabbed == Some(CursorGrabMode::Locked);
            // A warp is only meaningful with a real lock; an absolute-pointer
            // compositor (WSLg) undoes it, which manufactures a jump.
            if locked {
                warp_to_centre(window);
            }
            self.grab_mode = grabbed;
            match (self.mouse_mode, locked) {
                (MouseMode::Raw, _) => self.use_source(MotionSource::Raw),
                (MouseMode::Cursor, _) => self.use_source(MotionSource::Cursor),
                // A real lock hides the pointer, so only raw motion arrives.
                (MouseMode::Auto, true) => self.use_source(MotionSource::Raw),
                (MouseMode::Auto, false) => {}
            }
            match grabbed {
                Some(mode) => log::info!("pointer captured ({mode:?})"),
                // Look still works through `CursorMoved`; the cursor stays
                // visible because the compositor refused to hide it.
                None => log::warn!("pointer grab refused; the cursor stays visible"),
            }
        } else {
            let was_locked = self.grab_mode == Some(CursorGrabMode::Locked);
            self.grab_mode = None;
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            if self.focused && was_locked {
                warp_to_centre(window);
            }
            log::info!("pointer released");
        }
    }

    /// Forgets which motion source was in use, so the first event after a
    /// capture change cannot be measured against a stale position.
    fn reset_motion_source(&mut self) {
        self.source = MotionSource::None;
        self.pending_raw = (0.0, 0.0);
        self.pending_cursor = (0.0, 0.0);
        self.seen_raw = false;
        self.seen_cursor = false;
        self.motion_started = None;
        self.last_cursor = None;
    }

    /// Fixes the motion source for this capture and applies the sensitivity it
    /// implies.
    ///
    /// Horizon's model is raw motion, so that is what `Raw` and a real `Locked`
    /// grab use. The cursor path is a fallback for platforms whose raw deltas
    /// are not pixels; it gets the sweep cap because it is edge-limited.
    fn use_source(&mut self, source: MotionSource) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.apply_source_options();
        log::info!("mouse look source: {}", source.name());
    }

    /// Applies the sensitivity the current source implies.
    ///
    /// The cursor path is edge-limited, so it gets the sweep cap for the
    /// window's current width; resizing (including fullscreen) re-applies it.
    fn apply_source_options(&mut self) {
        let mut options = self.look_options;
        if self.source == MotionSource::Cursor && !self.sensitivity_explicit {
            let width = self
                .window
                .as_ref()
                .map_or(1280, |window| window.inner_size().width);
            options.sensitivity = options
                .sensitivity
                .min(sensitivity_for_sweep(width, CONFINED_SWEEP_DEGREES));
        }
        self.input.set_look_options(options);
    }

    /// Turns the view while the cursor is held in the outer edge zone.
    ///
    /// Only the cursor source needs this: a raw/locked pointer never runs out
    /// of room. It makes a full turn possible on a confined pointer, which
    /// otherwise stops at the window edge (WSLg has no relative motion, so
    /// nothing reports movement past the edge).
    fn edge_turn(&mut self, delta: Duration) {
        if self.source != MotionSource::Cursor || !self.input.captured() {
            return;
        }
        let (Some(window), Some(position)) = (self.window.clone(), self.last_cursor) else {
            return;
        };
        let size = window.inner_size();
        let half_width = f64::from(size.width) / 2.0;
        let half_height = f64::from(size.height) / 2.0;
        if half_width <= 0.0 || half_height <= 0.0 {
            return;
        }
        let push = |offset: f64, half: f64| {
            let offset = (offset / half).clamp(-1.0, 1.0);
            let magnitude = offset.abs();
            if magnitude <= f64::from(EDGE_ZONE) {
                0.0
            } else {
                let beyond = (magnitude - f64::from(EDGE_ZONE)) / (1.0 - f64::from(EDGE_ZONE));
                (offset.signum() * beyond) as f32
            }
        };
        let yaw = push(position.x - half_width, half_width);
        let pitch = push(position.y - half_height, half_height);
        if yaw == 0.0 && pitch == 0.0 {
            return;
        }
        let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f32();
        // Positive screen y is down, so the pitch is negated.
        self.camera.rotate(
            yaw * EDGE_YAW_RATE * seconds,
            -pitch * EDGE_PITCH_RATE * seconds,
        );
    }

    /// Decides the source if it is still open, then feeds this frame's motion.
    ///
    /// Raw deltas are in pixels on a normal X server and with a real lock, so
    /// they are preferred: that is Horizon's path and it never runs out of
    /// room. Where raw is in device units (Xwayland) or absent, the cursor's
    /// pixel deltas are used instead.
    fn flush_motion(&mut self) {
        if !self.input.captured() {
            return;
        }
        if self.source == MotionSource::None {
            // Wait until both sources have reported so raw can be compared
            // with the cursor's pixels. If only one ever arrives, take it once
            // the grace window after the first motion has passed.
            let decided = if self.seen_raw && self.seen_cursor {
                Some(choose_source(self.pending_raw, self.pending_cursor))
            } else if self
                .motion_started
                .is_some_and(|started| started.elapsed() >= SOURCE_GRACE)
            {
                match (self.seen_raw, self.seen_cursor) {
                    (true, false) => Some(MotionSource::Raw),
                    (false, true) => Some(MotionSource::Cursor),
                    _ => None,
                }
            } else {
                None
            };
            let Some(source) = decided else {
                // Keep accumulating until the source is known, so no motion is
                // lost to the negotiation.
                return;
            };
            self.use_source(source);
        }
        let (dx, dy) = match self.source {
            MotionSource::Raw => self.pending_raw,
            MotionSource::Cursor => self.pending_cursor,
            MotionSource::None => (0.0, 0.0),
        };
        if dx != 0.0 || dy != 0.0 {
            self.last_source = self.source;
            self.input.motion(dx, dy);
        }
        self.pending_raw = (0.0, 0.0);
        self.pending_cursor = (0.0, 0.0);
    }

    /// Feeds a pointer position into the look when cursor positions are the
    /// motion source.
    ///
    /// WSLg's Xwayland reports garbage positions before the grab settles
    /// (`-31079,-32230` was observed) and undoes `set_cursor_position` warps,
    /// so both are rejected here: a position outside the window resets the
    /// baseline, and a jump wider than half the window is treated as a
    /// re-association, not a turn. Re-centring is deliberately not attempted.
    fn cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let inside = position.x >= 0.0
            && position.y >= 0.0
            && position.x <= f64::from(size.width)
            && position.y <= f64::from(size.height);
        if !inside {
            log::trace!("cursor position outside the window: {position:?}");
            self.last_cursor = None;
            return;
        }
        let previous = self.last_cursor.replace(position);
        let Some(previous) = previous else {
            return;
        };
        if !self.input.captured() {
            return;
        }
        let (dx, dy) = (position.x - previous.x, position.y - previous.y);
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let limit = f64::from(size.width.min(size.height)) * 0.5;
        if dx.abs() > limit || dy.abs() > limit {
            log::trace!("ignoring cursor jump {dx:.0},{dy:.0}");
            return;
        }
        log::trace!("cursor delta: {dx:.1},{dy:.1}");
        self.pending_cursor.0 += dx;
        self.pending_cursor.1 += dy;
        self.seen_cursor = true;
        self.motion_started.get_or_insert_with(Instant::now);
    }

    /// Draws and presents one frame, or stops the loop if that fails.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let delta = self.timing.tick();
        self.apply_input(delta);
        self.tracker
            .report(&self.camera, self.input.captured(), self.grab_mode);
        let result = match &self.gpu {
            Some(gpu) => gpu.render(CLEAR_COLOR, &self.camera),
            None => return,
        };
        match result {
            Ok(()) => {}
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    /// Handles a new drawable size for the window and camera.
    fn resized(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(size);
        }
        self.camera
            .set_aspect(size.width as f32 / size.height as f32);
        // A fullscreen or resized window changes the cursor path's sweep.
        self.apply_source_options();
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` may fire again on platforms that recreate the surface.
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(WINDOW_SIZE);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => {
                window.focus_window();
                Arc::new(window)
            }
            Err(error) => {
                self.error = Some(ViewerError::from(error));
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        log::info!(
            "window \"{TITLE}\": {}x{} at scale {}",
            size.width,
            size.height,
            window.scale_factor()
        );
        if size.height > 0 {
            self.camera
                .set_aspect(size.width as f32 / size.height as f32);
        }
        let atlas = match load_startup_atlas() {
            Ok(atlas) => atlas,
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
                return;
            }
        };
        match Gpu::new(window.clone(), event_loop.owned_display_handle(), atlas) {
            Ok(gpu) => {
                self.window = Some(window.clone());
                self.sync_capture(&window);
                self.gpu = Some(gpu);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                let action = match event.state {
                    ElementState::Pressed if event.repeat => KeyAction::Repeat,
                    ElementState::Pressed => KeyAction::Press,
                    ElementState::Released => KeyAction::Release,
                };
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.input.key(code, action);
                }
                self.handle_edges(event_loop);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let action = match state {
                    ElementState::Pressed => KeyAction::Press,
                    ElementState::Released => KeyAction::Release,
                };
                self.input.mouse_button(button, action);
                self.handle_edges(event_loop);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Winit reports line steps on some platforms and pixels on
                // others; Horizon divides the pixel form by the same constant.
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    MouseScrollDelta::PixelDelta(position) => (
                        position.x / voxel_viewer::input::PIXELS_PER_STEP,
                        position.y / voxel_viewer::input::PIXELS_PER_STEP,
                    ),
                };
                let steps = self.input.scroll(x, y);
                if steps != 0 {
                    self.speed_steps =
                        (self.speed_steps + steps).clamp(MIN_SPEED_STEPS, MAX_SPEED_STEPS);
                    log::info!("fly speed: {:.1} units/s", self.fly_speed());
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor_moved(position),
            WindowEvent::Resized(size) => self.resized(size),
            // A scale-factor change can change the drawable size without a
            // separate `Resized` event on some platforms.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
                    self.resized(size);
                }
            }
            WindowEvent::Focused(true) => {
                log::info!("window focused");
                self.focused = true;
                let window = self.window.clone();
                if let Some(window) = window {
                    self.sync_capture(&window);
                }
            }
            WindowEvent::Focused(false) => {
                log::info!("window unfocused");
                self.focused = false;
                let window = self.window.clone();
                if let Some(window) = window {
                    self.sync_capture(&window);
                }
                // A key released while another window had focus never reaches
                // us; do not leave it held or its click pending.
                self.input.release_all();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.input.captured()
        {
            log::trace!("raw motion: {:.1},{:.1}", delta.0, delta.1);
            self.pending_raw.0 += delta.0;
            self.pending_raw.1 += delta.1;
            self.seen_raw = true;
            self.motion_started.get_or_insert_with(Instant::now);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Keep one frame in flight. With `Fifo` presentation this is paced by
        // the display, so it does not spin.
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Moves the pointer to the middle of the window.
///
/// A `Locked` pointer ignores this; a `Confined` one starts with room to move
/// in every direction instead of in whichever direction it was last pushed.
fn warp_to_centre(window: &Window) {
    let size = window.inner_size();
    let centre = PhysicalPosition::new(f64::from(size.width) / 2.0, f64::from(size.height) / 2.0);
    // Wayland only honours this for a locked pointer; the error is expected.
    let _ = window.set_cursor_position(centre);
}

/// Averages frame intervals and logs one line per [`FRAME_LOG_INTERVAL`].
#[derive(Default)]
struct FrameTiming {
    last: Option<Instant>,
    frames: u32,
    elapsed: Duration,
}

impl FrameTiming {
    /// Returns the time since the previous frame; zero on the first frame.
    fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let Some(last) = self.last.replace(now) else {
            return Duration::ZERO;
        };
        let delta = now.duration_since(last);
        self.elapsed += delta;
        self.frames += 1;
        if self.elapsed >= FRAME_LOG_INTERVAL {
            let seconds = self.elapsed.as_secs_f64();
            let millis = seconds * 1_000.0 / f64::from(self.frames);
            // `debug`: once a second is too much for a default run, and on a
            // display that does not pace presents (WSLg) it is pure noise.
            log::debug!(
                "frame timing: {:.1} fps ({millis:.2} ms/frame)",
                f64::from(self.frames) / seconds
            );
            self.frames = 0;
            self.elapsed = Duration::ZERO;
        }
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_wins_when_it_matches_the_cursor() {
        assert_eq!(choose_source((10.0, 0.0), (12.0, 2.0)), MotionSource::Raw);
        assert_eq!(choose_source((0.0, -7.0), (0.0, -9.0)), MotionSource::Raw);
    }

    #[test]
    fn cursor_wins_when_raw_is_in_device_units() {
        // Values measured from Xwayland: a 10 px move reports 888.
        assert_eq!(
            choose_source((888.0, 342.0), (10.0, 4.0)),
            MotionSource::Cursor
        );
    }

    #[test]
    fn small_cursor_deltas_do_not_reject_raw() {
        // A still cursor never reaches the comparison (no cursor movement), so
        // this documents the tolerance for jitter when both sources report.
        assert_eq!(choose_source((2.0, 2.0), (1.0, 0.0)), MotionSource::Raw);
    }
}
