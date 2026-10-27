//! The voxel viewer binary.
//!
//! The viewer opens a window, initializes wgpu for it and keeps the surface
//! configuration in step with the window. Later tasks add the camera and the
//! chunk renderer on top of this shell.
//!
//! ```text
//! cargo run -p voxel-viewer
//! ```

mod error;
mod gpu;

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::error::ViewerError;
use crate::gpu::Gpu;

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

fn main() -> ExitCode {
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

/// The application state: the window and the graphics handles for it.
#[derive(Default)]
struct Viewer {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// A failure raised inside an event-loop callback.
    ///
    /// Callbacks cannot return an error, so the first one is kept here, the
    /// loop is stopped, and [`run`] reports it after the loop unwinds.
    error: Option<ViewerError>,
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
            _ => {}
        }
    }
}
