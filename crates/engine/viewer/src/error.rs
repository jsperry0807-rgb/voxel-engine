//! Errors surfaced by the viewer.
//!
//! The viewer is a binary, so errors only need to reach the terminal with a
//! useful chain. They stay typed until then: platform and graphics failures are
//! kept as the error that produced them rather than flattened into a string.

use std::error::Error;
use std::fmt;

use wgpu::{CreateSurfaceError, RequestAdapterError, RequestDeviceError};
use winit::error::{EventLoopError, OsError};

/// A failure that prevented the viewer from starting or running.
#[derive(Debug)]
pub enum ViewerError {
    /// The event loop could not be created, or it stopped with an error.
    EventLoop(EventLoopError),
    /// The platform could not create the window.
    Window(OsError),
    /// wgpu could not create a surface for the window.
    Surface(CreateSurfaceError),
    /// No adapter can present to the window surface.
    Adapter(RequestAdapterError),
    /// The adapter could not open a logical device.
    Device(RequestDeviceError),
    /// The adapter cannot present to this surface at all.
    UnsupportedSurface,
    /// Acquiring the next frame raised a validation error.
    SurfaceValidation,
}

impl fmt::Display for ViewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoop(error) => write!(f, "event loop failed: {error}"),
            Self::Window(error) => write!(f, "could not create the window: {error}"),
            Self::Surface(error) => write!(f, "could not create the window surface: {error}"),
            Self::Adapter(error) => write!(f, "no suitable graphics adapter: {error}"),
            Self::Device(error) => write!(f, "could not open the graphics device: {error}"),
            Self::UnsupportedSurface => {
                f.write_str("the graphics adapter cannot present to this surface")
            }
            Self::SurfaceValidation => f.write_str("surface acquisition failed validation"),
        }
    }
}

impl Error for ViewerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EventLoop(error) => Some(error),
            Self::Window(error) => Some(error),
            Self::Surface(error) => Some(error),
            Self::Adapter(error) => Some(error),
            Self::Device(error) => Some(error),
            Self::UnsupportedSurface | Self::SurfaceValidation => None,
        }
    }
}

impl From<EventLoopError> for ViewerError {
    fn from(error: EventLoopError) -> Self {
        Self::EventLoop(error)
    }
}

impl From<OsError> for ViewerError {
    fn from(error: OsError) -> Self {
        Self::Window(error)
    }
}

impl From<CreateSurfaceError> for ViewerError {
    fn from(error: CreateSurfaceError) -> Self {
        Self::Surface(error)
    }
}

impl From<RequestAdapterError> for ViewerError {
    fn from(error: RequestAdapterError) -> Self {
        Self::Adapter(error)
    }
}

impl From<RequestDeviceError> for ViewerError {
    fn from(error: RequestDeviceError) -> Self {
        Self::Device(error)
    }
}
