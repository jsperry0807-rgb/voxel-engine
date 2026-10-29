//! Yaw/pitch tracking for diagnosing mouse look.
//!
//! Set `VOXEL_TRACK=1` to log one `track:` line per second with the camera's
//! angles and the motion applied since the previous line. Set it to a path
//! (`VOXEL_TRACK=look.csv`) to also write one row per frame that turned the
//! camera:
//!
//! ```text
//! t_ms,source,dx,dy,yaw_deg,pitch_deg
//! ```
//!
//! `dx`/`dy` are the counts that drove the turn after the motion source was
//! chosen (`raw` or `cursor`), and `yaw_deg`/`pitch_deg` are the camera angles
//! after applying them. Comparing the two columns shows immediately whether a
//! source is scaled or duplicated.
//!
//! The tracker does nothing when the variable is unset: it is meant for
//! collecting data from a real run, not as an always-on log.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use winit::window::CursorGrabMode;

use crate::camera::Camera;
use crate::input::Look;

/// How often the summary line is logged.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Where a frame's motion came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MotionSource {
    /// No motion since the previous frame.
    #[default]
    None,
    /// `DeviceEvent::MouseMotion`, the relative-motion path.
    Raw,
    /// `CursorMoved` deltas, the position path.
    Cursor,
}

impl MotionSource {
    /// The name used in the CSV.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Raw => "raw",
            Self::Cursor => "cursor",
        }
    }
}

/// Records camera angles and the motion that changed them.
pub struct Tracker {
    started: Instant,
    logging: bool,
    file: Option<BufWriter<File>>,
    last_report: Instant,
    frames: u32,
    looks: u32,
    dx: f64,
    dy: f64,
    yaw: f32,
    pitch: f32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::disabled()
    }
}

impl Tracker {
    /// A tracker that records nothing.
    pub fn disabled() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            logging: false,
            file: None,
            last_report: now,
            frames: 0,
            looks: 0,
            dx: 0.0,
            dy: 0.0,
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    /// Reads `VOXEL_TRACK`: unset or `0` is off, `1` logs, any other value is
    /// a CSV path (which also logs).
    pub fn from_env() -> Self {
        let mut tracker = Self::disabled();
        let Some(value) = std::env::var_os("VOXEL_TRACK") else {
            return tracker;
        };
        let value = value.to_string_lossy();
        let value = value.trim();
        if value.is_empty() || value == "0" || value == "false" {
            return tracker;
        }
        tracker.logging = true;
        if !matches!(value, "1" | "true" | "yes" | "on") {
            match File::create(Path::new(value)) {
                Ok(file) => {
                    let mut file = BufWriter::new(file);
                    if writeln!(file, "t_ms,source,dx,dy,yaw_deg,pitch_deg").is_ok() {
                        tracker.file = Some(file);
                        log::info!("tracker: writing {value}");
                    }
                }
                Err(error) => log::warn!("tracker: cannot create {value}: {error}"),
            }
        }
        log::info!("tracker: on; one line per second, VOXEL_TRACK=<path> for CSV");
        tracker
    }

    /// Records one frame that turned the camera.
    ///
    /// `look.dx`/`look.dy` are the counts used, `camera` is already turned.
    pub fn look(&mut self, source: MotionSource, look: Look, camera: &Camera) {
        if !self.logging || (look.yaw == 0.0 && look.pitch == 0.0) {
            return;
        }
        self.looks += 1;
        self.dx += look.dx;
        self.dy += look.dy;
        self.yaw += look.yaw;
        self.pitch += look.pitch;
        if let Some(file) = &mut self.file {
            let t_ms = self.started.elapsed().as_secs_f64() * 1_000.0;
            let result = writeln!(
                file,
                "{t_ms:.0},{},{:.1},{:.1},{:.3},{:.3}",
                source.name(),
                look.dx,
                look.dy,
                camera.yaw().to_degrees(),
                camera.pitch().to_degrees()
            );
            if let Err(error) = result {
                log::warn!("track log write failed: {error}");
                self.file = None;
            }
        }
    }

    /// Counts a frame, whether or not it turned the camera.
    pub fn frame(&mut self) {
        if self.logging {
            self.frames += 1;
        }
    }

    /// Logs the summary line once per [`REPORT_INTERVAL`] and clears the
    /// counters.
    pub fn report(&mut self, camera: &Camera, captured: bool, grab: Option<CursorGrabMode>) {
        if !self.logging {
            return;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_report);
        if elapsed < REPORT_INTERVAL {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        log::info!(
            "track: yaw={:.2} pitch={:.2} deg | {:.0} fps, {} turning frames | \
             dx={:+.0} dy={:+.0} applied yaw={:+.2} pitch={:+.2} deg | \
             captured={captured} grab={grab:?}",
            camera.yaw().to_degrees(),
            camera.pitch().to_degrees(),
            f64::from(self.frames) / seconds,
            self.looks,
            self.dx,
            self.dy,
            self.yaw.to_degrees(),
            self.pitch.to_degrees(),
        );
        if let Some(file) = &mut self.file
            && let Err(error) = file.flush()
        {
            log::warn!("track log flush failed: {error}");
            self.file = None;
        }
        self.last_report = now;
        self.frames = 0;
        self.looks = 0;
        self.dx = 0.0;
        self.dy = 0.0;
        self.yaw = 0.0;
        self.pitch = 0.0;
    }
}
