//! The voxel viewer binary.
//!
//! The viewer opens a window, initializes wgpu for it, keeps the surface
//! configuration in step with the window, and runs a frame loop with a depth
//! buffer. A fly camera responds to WASD and mouse motion. Two overlapping
//! debug quads make the depth test visible until the chunk renderer replaces
//! them.
//!
//! ```text
//! cargo run -p voxel-viewer
//! cargo run -p voxel-viewer -- bake-atlas --manifest <path> --out <dir>
//! ```
//!
//! `RUST_LOG` follows `env_logger` syntax. The default shows warnings from
//! dependencies and informational messages from the viewer; for a quiet wgpu
//! run use `RUST_LOG=wgpu_core=warn`.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};
use voxel_viewer::atlas::Atlas;
use voxel_viewer::bake;
use voxel_viewer::camera::Camera;
use voxel_viewer::error::ViewerError;
use voxel_viewer::gpu::Gpu;
use wgpu::Color;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1920.0, 1080.0);
/// Background colour before anything is drawn.
const CLEAR_COLOR: Color = Color {
    r: 0.02,
    g: 0.025,
    b: 0.035,
    a: 1.0,
};
/// How often the averaged frame time is logged.
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
/// Fly speed in world units per second. The M1.5 slice is a 4 m chunk, so
/// this is deliberately slower than a full-world fly speed.
const MOVE_SPEED: f32 = 3.0;
/// Radians of rotation per mouse pixel.
const MOUSE_SENSITIVITY: f32 = 0.0012;
/// Reject implausible single mouse deltas, such as X11 pointer-warp bursts
/// generated while grabbing the cursor.
const MAX_MOUSE_EVENT: f32 = 500.0;
/// A long stall must not teleport the camera across the world.
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
/// Pointer grabs can emit a synthetic motion burst; ignore it so the camera
/// starts on the test block instead of snapping away.
const GRAB_SETTLE: Duration = Duration::from_millis(100);

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
    let event_loop = EventLoop::new()?;
    let mut viewer = Viewer::default();
    event_loop.run_app(&mut viewer)?;
    match viewer.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// The application state: window, graphics handles, camera and input.
#[derive(Default)]
struct Viewer {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    camera: Camera,
    controls: Controls,
    timing: FrameTiming,
    cursor_mode: Option<CursorGrabMode>,
    /// A failure raised inside an event-loop callback.
    ///
    /// Callbacks cannot return an error, so the first one is kept here, the
    /// loop is stopped, and [`run`] reports it after the loop unwinds.
    error: Option<ViewerError>,
}

impl Viewer {
    /// Applies the input gathered since the previous frame.
    fn apply_input(&mut self, delta: Duration) {
        let mouse = std::mem::take(&mut self.controls.mouse_delta);
        if mouse != Vec2::ZERO {
            self.camera
                .rotate(mouse.x * MOUSE_SENSITIVITY, -mouse.y * MOUSE_SENSITIVITY);
        }

        let mut direction = Vec3::ZERO;
        if self.controls.forward {
            direction += self.camera.forward();
        }
        if self.controls.back {
            direction -= self.camera.forward();
        }
        if self.controls.right {
            direction += self.camera.right();
        }
        if self.controls.left {
            direction -= self.camera.right();
        }
        if direction.length_squared() > 0.0 {
            let seconds = delta.min(MAX_FRAME_DELTA).as_secs_f32();
            self.camera
                .translate(direction.normalize() * MOVE_SPEED * seconds);
        }
    }

    /// Draws and presents one frame, or stops the loop if that fails.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // Some compositors only apply the hide on the first pointer enter, or
        // restore the window cursor after a grab. Reassert it while grabbed.
        if self.cursor_mode.is_some()
            && let Some(window) = &self.window
        {
            window.set_cursor_visible(false);
        }
        let delta = self.timing.tick();
        self.apply_input(delta);
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

    /// Captures and hides the pointer, resetting the pending look delta.
    fn grab(&mut self, window: &Window) {
        self.cursor_mode = grab_cursor(window);
        self.controls.mouse_delta = Vec2::ZERO;
        self.controls.last_cursor = None;
        self.controls.raw_motion = false;
        self.controls.ignore_motion_until = Some(Instant::now() + GRAB_SETTLE);
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
    }
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` may fire again on platforms that recreate the surface.
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("voxel-viewer")
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
                self.grab(&window);
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
                let pressed = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) if pressed => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::KeyW) => self.controls.forward = pressed,
                    PhysicalKey::Code(KeyCode::KeyS) => self.controls.back = pressed,
                    PhysicalKey::Code(KeyCode::KeyA) => self.controls.left = pressed,
                    PhysicalKey::Code(KeyCode::KeyD) => self.controls.right = pressed,
                    PhysicalKey::Code(KeyCode::KeyR) if pressed => {
                        self.camera = Camera::default();
                        if let Some(window) = &self.window {
                            let size = window.inner_size();
                            if size.height > 0 {
                                self.camera
                                    .set_aspect(size.width as f32 / size.height as f32);
                            }
                        }
                        self.controls.mouse_delta = Vec2::ZERO;
                        self.controls.last_cursor = None;
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => self.resized(size),
            // A scale-factor change can change the drawable size without a
            // separate `Resized` event on some platforms.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(size) = self.window.as_ref().map(|window| window.inner_size()) {
                    self.resized(size);
                }
            }
            WindowEvent::Focused(true) => {
                if let Some(window) = self.window.clone() {
                    self.grab(&window);
                }
            }
            WindowEvent::CursorEntered { .. }
            | WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                if let Some(window) = self.window.clone() {
                    self.grab(&window);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.controls.raw_motion {
                    if let Some(last) = self.controls.last_cursor {
                        let dx = position.x - last.x;
                        let dy = position.y - last.y;
                        if dx.abs() <= f64::from(MAX_MOUSE_EVENT)
                            && dy.abs() <= f64::from(MAX_MOUSE_EVENT)
                        {
                            self.controls.mouse_delta += Vec2::new(dx as f32, dy as f32);
                        }
                    }
                    self.controls.last_cursor = Some(position);
                }
            }
            WindowEvent::Focused(false) => {
                if let Some(window) = &self.window {
                    let _ = window.set_cursor_grab(CursorGrabMode::None);
                    window.set_cursor_visible(true);
                }
                self.cursor_mode = None;
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
            && self.window.is_some()
        {
            self.controls.raw_motion = true;
            self.controls.last_cursor = None;
            if self
                .controls
                .ignore_motion_until
                .is_some_and(|until| Instant::now() < until)
            {
                return;
            }
            self.controls.ignore_motion_until = None;
            let dx = delta.0 as f32;
            let dy = delta.1 as f32;
            if dx.abs() <= MAX_MOUSE_EVENT && dy.abs() <= MAX_MOUSE_EVENT {
                self.controls.mouse_delta += Vec2::new(dx, dy);
            }
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

/// Keys the camera reads, plus pointer motion accumulated between frames.
#[derive(Default)]
struct Controls {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    mouse_delta: Vec2,
    last_cursor: Option<PhysicalPosition<f64>>,
    raw_motion: bool,
    ignore_motion_until: Option<Instant>,
}

/// Captures the pointer for mouse look, falling back to confinement.
fn grab_cursor(window: &Window) -> Option<CursorGrabMode> {
    let mut last_error = None;
    for mode in [CursorGrabMode::Locked, CursorGrabMode::Confined] {
        match window.set_cursor_grab(mode) {
            Ok(()) => {
                window.set_cursor_visible(false);
                log::info!("cursor grab: {mode:?}");
                return Some(mode);
            }
            Err(error) => {
                log::debug!("cursor grab {mode:?} unavailable: {error}");
                last_error = Some(error);
            }
        }
    }
    log::warn!(
        "cursor grab unavailable; mouse look stops at the window edge: {}",
        last_error.map_or_else(|| "no supported mode".to_owned(), |error| error.to_string())
    );
    None
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
            log::info!(
                "frame timing: {:.1} fps ({millis:.2} ms/frame)",
                f64::from(self.frames) / seconds
            );
            self.frames = 0;
            self.elapsed = Duration::ZERO;
        }
        delta
    }
}
