//! The viewer application: window, graphics handles, camera and input.
//!
//! [`Viewer`] is the winit application handler. It owns the window and GPU
//! handles, applies input to the camera, and turns events into frames. Startup
//! and environment parsing live in [`crate::startup`]; this module is only the
//! running application.

use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use wgpu::Color;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use crate::camera::Camera;
use crate::error::ViewerError;
use crate::frame_profile::FrameProfiler;
use crate::gpu::Gpu;
use crate::input::{
    Action, CONFINED_SWEEP_DEGREES, Input, KeyAction, LookOptions, sensitivity_for_sweep,
};
use crate::look::{MouseMode, SOURCE_GRACE, choose_source};
use crate::startup::{load_startup_atlas, tile_layout_from_env};
use crate::tracker::{MotionSource, Tracker};

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1920.0, 1080.0);
/// Window title.
const TITLE: &str = "voxel-viewer";

/// Delay before retrying a frame the surface could not present.
///
/// Retrying immediately would spin whenever no presentation paces the loop
/// (occluded window, swap-chain timeout, WSLg without pacing).
const RETRY_INTERVAL: Duration = Duration::from_millis(16);
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

/// The application state: window, graphics handles, camera and input.
pub(crate) struct Viewer {
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) gpu: Option<Gpu>,
    pub(crate) camera: Camera,
    pub(crate) input: Input,
    /// Wheel steps applied to the fly speed; see [`Viewer::fly_speed`].
    pub(crate) speed_steps: i32,
    pub(crate) timing: FrameTiming,
    /// A failure raised inside an event-loop callback.
    ///
    /// Callbacks cannot return an error, so the first one is kept here, the
    /// loop is stopped, and [`run`] reports it after the loop unwinds.
    pub(crate) error: Option<ViewerError>,
    /// Whether mouse look is switched on (the `ToggleCapture` action).
    pub(crate) look_enabled: bool,
    /// Whether the window is borderless fullscreen.
    pub(crate) fullscreen: bool,
    /// Whether the window has focus. Assumed on until the platform says
    /// otherwise, so the first frames work on platforms that send no
    /// `Focused` event at startup.
    pub(crate) focused: bool,
    /// How the look source is chosen.
    pub(crate) mouse_mode: MouseMode,
    /// The source chosen for this capture; `None` while negotiating.
    pub(crate) source: MotionSource,
    /// Raw deltas since the last frame.
    pub(crate) pending_raw: (f64, f64),
    /// Cursor deltas since the last frame.
    pub(crate) pending_cursor: (f64, f64),
    /// Whether each source has moved at all in this capture.
    pub(crate) seen_raw: bool,
    pub(crate) seen_cursor: bool,
    /// When the first motion of this capture arrived, for [`SOURCE_GRACE`].
    pub(crate) motion_started: Option<Instant>,
    /// The grab mode that took, or `None` when no grab was accepted.
    pub(crate) grab_mode: Option<CursorGrabMode>,
    /// The previous `CursorMoved` position.
    pub(crate) last_cursor: Option<PhysicalPosition<f64>>,
    /// Which source fed the current frame, for the tracker.
    pub(crate) last_source: MotionSource,
    /// Mouse-look settings as configured, before the confined-pointer cap.
    pub(crate) look_options: LookOptions,
    /// Whether `VOXEL_SENSITIVITY` set the sensitivity, in which case the
    /// confined-pointer cap leaves it alone.
    pub(crate) sensitivity_explicit: bool,
    /// Yaw/pitch tracker; off unless `VOXEL_TRACK` is set.
    pub(crate) tracker: Tracker,
    /// Whether the last frame reached the swap chain; see `about_to_wait`.
    pub(crate) last_frame_presented: bool,
    /// Whether the platform says the window is occluded (minimized/covered).
    pub(crate) occluded: bool,
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
            last_frame_presented: true,
            occluded: false,
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
        let result = match &mut self.gpu {
            Some(gpu) => gpu.render(CLEAR_COLOR, &self.camera),
            None => return,
        };
        match result {
            Ok(presented) => self.last_frame_presented = presented,
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
            // The old surface is no longer valid after suspend/resume or a
            // display-server restart.
            if let Some(gpu) = &mut self.gpu
                && let Err(error) = gpu.recreate_surface()
            {
                self.error = Some(error);
                event_loop.exit();
                return;
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
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
        let atlas = match load_startup_atlas(tile_layout_from_env()) {
            Ok(atlas) => atlas,
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
                return;
            }
        };
        // Put the density in the title so a launch can be checked at a glance,
        // without reading the log: the value is the atlas that actually loaded,
        // not the one that was requested.
        window.set_title(&format!("{TITLE} · {}-texel tiles", atlas.tile_size()));
        log::info!("texture density: {}-texel tiles", atlas.tile_size());
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
                        position.x / crate::input::PIXELS_PER_STEP,
                        position.y / crate::input::PIXELS_PER_STEP,
                    ),
                };
                let steps = self.input.scroll(x, y);
                if steps != 0 {
                    self.speed_steps = self
                        .speed_steps
                        .saturating_add(steps)
                        .clamp(MIN_SPEED_STEPS, MAX_SPEED_STEPS);
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
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                // While occluded no frame is presented, so `about_to_wait`
                // stops retrying; restart when the window is visible again.
                if !occluded && let Some(window) = &self.window {
                    window.request_redraw();
                }
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // A DPI change can report a stale inner size from inside the event, so
        // resync against the window here; this also catches a missed `Resized`.
        if let (Some(window), Some(gpu)) = (&self.window, &mut self.gpu) {
            gpu.resize(window.inner_size());
        }
        // Nothing can be drawn while the platform says the window is hidden;
        // `Occluded(false)` asks for a frame again.
        if self.occluded {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        // With `Fifo` presentation a presented frame is paced by the display,
        // so keep one frame in flight.
        if self.last_frame_presented {
            event_loop.set_control_flow(ControlFlow::Wait);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        // A skipped frame (occluded, timed out, reconfigured) means nothing is
        // pacing us. Retry at display cadence instead of spinning at 100% CPU.
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + RETRY_INTERVAL));
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // Wake a skipped frame: `about_to_wait` armed a `WaitUntil` timer
        // instead of requesting a redraw.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && let Some(window) = &self.window
        {
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
pub(crate) struct FrameTiming {
    last: Option<Instant>,
    frames: u32,
    elapsed: Duration,
    profile: FrameProfiler,
}

impl Default for FrameTiming {
    fn default() -> Self {
        Self::new(FrameProfiler::disabled())
    }
}

impl FrameTiming {
    pub(crate) fn new(profile: FrameProfiler) -> Self {
        Self {
            last: None,
            frames: 0,
            elapsed: Duration::ZERO,
            profile,
        }
    }

    /// Returns the time since the previous frame; zero on the first frame.
    fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let Some(last) = self.last.replace(now) else {
            return Duration::ZERO;
        };
        let delta = now.duration_since(last);
        self.profile.record(delta);
        self.elapsed += delta;
        self.frames += 1;
        if self.elapsed >= FRAME_LOG_INTERVAL {
            let seconds = self.elapsed.as_secs_f64();
            let millis = seconds * 1_000.0 / f64::from(self.frames);
            let fps = f64::from(self.frames) / seconds;
            // The default run keeps the one-second line at `debug`; the exit
            // summary is the measurement the 60 Hz target is checked with.
            if self.profile.logs_interval() {
                log::info!("frame timing: {fps:.1} fps ({millis:.2} ms/frame)");
                self.profile.flush();
            } else {
                // Once a second is too much for a default run, and on a
                // display that does not pace presents (WSLg) it is pure noise.
                log::debug!("frame timing: {fps:.1} fps ({millis:.2} ms/frame)");
            }
            self.frames = 0;
            self.elapsed = Duration::ZERO;
        }
        delta
    }

    /// Flushes the trace and returns the final summary, if profiling was on.
    pub(crate) fn finish(&mut self) -> Option<voxel_profiling::FrameStats> {
        self.profile.finish()
    }
}
