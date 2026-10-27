//! Errors surfaced by the viewer.
//!
//! The viewer is a binary, so errors only need to reach the terminal with a
//! useful chain. They stay typed until then: platform failures are kept as the
//! `winit` error that produced them rather than flattened into a string.

use std::error::Error;
use std::fmt;

use winit::error::{EventLoopError, OsError};

/// A failure that prevented the viewer from starting or running.
#[derive(Debug)]
pub enum ViewerError {
    /// The event loop could not be created, or it stopped with an error.
    EventLoop(EventLoopError),
    /// The platform could not create the window.
    Window(OsError),
}

impl fmt::Display for ViewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoop(error) => write!(f, "event loop failed: {error}"),
            Self::Window(error) => write!(f, "could not create the window: {error}"),
        }
    }
}

impl Error for ViewerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EventLoop(error) => Some(error),
            Self::Window(error) => Some(error),
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
