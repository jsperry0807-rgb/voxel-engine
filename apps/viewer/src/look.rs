//! Mouse-look source negotiation.
//!
//! A locked pointer gives raw `DeviceEvent::MouseMotion` deltas in pixels; a
//! confined pointer on a platform without relative motion (WSLg's Xwayland)
//! only reports cursor positions, and its raw deltas are in device units. This
//! module decides which source to trust for a capture and holds the constants
//! that decision uses.

use std::time::Duration;

use crate::tracker::MotionSource;

/// How long after the first motion the negotiation waits for the second source
/// before choosing the one it has seen.
pub(crate) const SOURCE_GRACE: Duration = Duration::from_millis(50);
/// Raw deltas may be this many times larger than the cursor deltas for the
/// same movement before they are treated as device units instead of pixels
/// (Xwayland reports them that way).
const RAW_TO_CURSOR_MAX: f64 = 4.0;

/// How mouse motion is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseMode {
    /// Raw motion when it is in pixels, cursor deltas otherwise.
    Auto,
    /// Horizon's model: `DeviceEvent::MouseMotion` only.
    Raw,
    /// `CursorMoved` deltas only.
    Cursor,
}

/// Reads `VOXEL_MOUSE`: `auto` (default), `raw` or `cursor`.
pub(crate) fn load_mouse_mode() -> MouseMode {
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
pub(crate) fn choose_source(raw: (f64, f64), cursor: (f64, f64)) -> MotionSource {
    let raw_len = raw.0.abs() + raw.1.abs();
    let cursor_len = cursor.0.abs() + cursor.1.abs();
    if raw_len <= cursor_len * RAW_TO_CURSOR_MAX + 8.0 {
        MotionSource::Raw
    } else {
        MotionSource::Cursor
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
