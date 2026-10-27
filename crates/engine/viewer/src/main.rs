//! The voxel viewer binary.
//!
//! The viewer opens a window, initializes wgpu for it, keeps the surface
//! configuration in step with the window, and runs a frame loop that clears
//! and presents each frame. Later tasks add the camera and the chunk renderer
//! on top of this shell.
//!
//! ```text
//! cargo run -p voxel-viewer
//! ```
//!
//! `RUST_LOG` follows `env_logger` syntax. The default shows warnings from
//! dependencies and informational messages from the viewer; for a quiet wgpu
//! run use `RUST_LOG=wgpu_core=warn`.

mod error;
mod gpu;

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use wgpu::Color;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::error::ViewerError;
use crate::gpu::Gpu;

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);
/// Background colour before anything is drawn.
const CLEAR_COLOR: Color = Color {
    r: 0.02,
    g: 0.025,
    b: 0.035,
    a: 1.0,
};
/// How often the averaged frame time is logged.
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> ExitCode {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,voxel_viewer=info"),
    )
    .init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("voxel-viewer: {error}");
            let mut source = error.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
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

/// The application state: the window, the graphics handles and frame timing.
#[derive(Default)]
struct Viewer {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    timing: FrameTiming,
    /// A failure raised inside an event-loop callback.
    ///
    /// Callbacks cannot return an error, so the first one is kept here, the
    /// loop is stopped, and [`run`] reports it after the loop unwinds.
    error: Option<ViewerError>,
}

impl Viewer {
    /// Draws and presents one frame, or stops the loop if that fails.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let result = match &self.gpu {
            Some(gpu) => gpu.render(CLEAR_COLOR),
            None => return,
        };
        match result {
            Ok(()) => self.timing.tick(),
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
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
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.error = Some(ViewerError::from(error));
                event_loop.exit();
                return;
            }
        };
        match Gpu::new(window.clone(), event_loop.owned_display_handle()) {
            Ok(gpu) => {
                self.window = Some(window);
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
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
            {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
            }
            // A scale-factor change can change the drawable size without a
            // separate `Resized` event on some platforms.
            WindowEvent::ScaleFactorChanged { .. } => {
                if let (Some(window), Some(gpu)) = (&self.window, &mut self.gpu) {
                    gpu.resize(window.inner_size());
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
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

/// Averages frame intervals and logs one line per [`FRAME_LOG_INTERVAL`].
#[derive(Default)]
struct FrameTiming {
    last: Option<Instant>,
    frames: u32,
    elapsed: Duration,
}

impl FrameTiming {
    fn tick(&mut self) {
        let now = Instant::now();
        let Some(last) = self.last else {
            self.last = Some(now);
            return;
        };
        self.last = Some(now);
        self.elapsed += now.duration_since(last);
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
    }
}
