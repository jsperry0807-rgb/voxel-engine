//! Mouse look: accumulated relative motion, the sensitivity curve, cinematic
//! smoothing and wheel accumulation.
//!
//! This is the arithmetic of `horizon-client-input`'s `mouse.rs` without the
//! window-state policy: the caller decides whether the pointer is captured, and
//! feeds motion only while it is. Angles come out in radians with the viewer's
//! conventions: positive yaw turns right, positive pitch looks up. Horizon
//! reports degrees with positive pitch looking down (`Entity.turn`), so the
//! pitch sign is flipped here.
//!
//! Sensitivity follows vanilla: `0.5` turns 0.15 degrees per count, and the
//! curve is `(sensitivity * 0.6 + 0.2)^3 * 8`. At the default that is
//! 2.6 mrad/count, close to the viewer's old fixed `0.0025`.

/// Degrees the view turns per count at sensitivity 1.
const DEGREES_PER_COUNT: f64 = 0.15;
/// Default sensitivity, the middle of vanilla's `0.0..=1.0` slider.
pub const DEFAULT_SENSITIVITY: f64 = 0.5;
/// The scale a pixel delta is divided by before it counts as wheel steps.
/// Matches Horizon, which receives the same winit pixel deltas.
pub const PIXELS_PER_STEP: f64 = 30.0;

/// What the look settings allow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookOptions {
    /// Vanilla's `0.0..=1.0` mouse sensitivity.
    pub sensitivity: f64,
    /// Spread a turn over the following frames instead of applying it at once.
    pub smooth: bool,
    /// Reverse the horizontal axis.
    pub invert_x: bool,
    /// Reverse the vertical axis.
    pub invert_y: bool,
}

impl Default for LookOptions {
    fn default() -> Self {
        Self {
            sensitivity: DEFAULT_SENSITIVITY,
            smooth: false,
            invert_x: false,
            invert_y: false,
        }
    }
}

/// A turn for one frame, in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Positive turns right.
    pub yaw: f32,
    /// Positive looks up.
    pub pitch: f32,
    /// The counts behind this turn, after the motion source was chosen and
    /// before the sensitivity curve.
    pub dx: f64,
    pub dy: f64,
}

/// `SmoothDouble`: the cinematic camera's low-pass over the remaining turn.
///
/// Each frame moves the accumulated target part of the way towards it, so the
/// total is preserved and the tail is spread over later frames.
#[derive(Clone, Debug, Default)]
struct SmoothDouble {
    target: f64,
    remaining: f64,
    last_amount: f64,
}

impl SmoothDouble {
    /// Adds `target_delta` to the goal and returns this frame's share over
    /// `seconds`.
    fn new_delta(&mut self, target_delta: f64, seconds: f64) -> f64 {
        self.target += target_delta;
        let mut delta = self.target - self.remaining;
        let new_last_amount = self.last_amount + 0.5 * (delta - self.last_amount);
        let sign = java_signum(delta);
        if sign * delta > sign * self.last_amount {
            delta = new_last_amount;
        }
        self.last_amount = new_last_amount;
        self.remaining += delta * seconds;
        delta * seconds
    }
}

/// `Math.signum(double)`: keeps the sign of zero and NaN.
fn java_signum(value: f64) -> f64 {
    if value == 0.0 || value.is_nan() {
        value
    } else {
        value.signum()
    }
}

/// Fractional wheel motion accumulated into whole steps.
#[derive(Clone, Debug, Default)]
struct ScrollWheel {
    x: f64,
    y: f64,
}

impl ScrollWheel {
    /// Adds `(x, y)` steps and returns the whole steps now available. A change
    /// of direction drops what was left of the old one, so direction changes
    /// do not need to cancel a stale fraction first.
    fn accumulate(&mut self, x: f64, y: f64) -> (i32, i32) {
        if self.x != 0.0 && java_signum(x) != java_signum(self.x) {
            self.x = 0.0;
        }
        if self.y != 0.0 && java_signum(y) != java_signum(self.y) {
            self.y = 0.0;
        }
        self.x += x;
        self.y += y;
        // Java's double-to-int cast saturates and maps NaN to zero; so does
        // Rust's `as`.
        let steps_x = self.x as i32;
        let steps_y = self.y as i32;
        self.x -= f64::from(steps_x);
        self.y -= f64::from(steps_y);
        (steps_x, steps_y)
    }
}

/// Mouse motion and wheel, turned into per-frame view deltas.
#[derive(Clone, Debug)]
pub struct MouseLook {
    options: LookOptions,
    accumulated_x: f64,
    accumulated_y: f64,
    smooth_yaw: SmoothDouble,
    smooth_pitch: SmoothDouble,
    scroll: ScrollWheel,
}

impl Default for MouseLook {
    fn default() -> Self {
        Self::new(LookOptions::default())
    }
}

impl MouseLook {
    /// Creates a look with `options`.
    pub fn new(options: LookOptions) -> Self {
        Self {
            options,
            accumulated_x: 0.0,
            accumulated_y: 0.0,
            smooth_yaw: SmoothDouble::default(),
            smooth_pitch: SmoothDouble::default(),
            scroll: ScrollWheel::default(),
        }
    }

    /// Adds relative pointer motion, in physical pixels (winit's `MouseMotion`
    /// deltas). Motion is kept until the next [`MouseLook::turn`] call.
    pub fn add_motion(&mut self, dx: f64, dy: f64) {
        self.accumulated_x += dx;
        self.accumulated_y += dy;
    }

    /// Consumes the motion accumulated since the last call and returns this
    /// frame's turn, or `None` when the pointer is not active.
    ///
    /// Motion is dropped either way: a turn that arrives while the pointer is
    /// free must not be applied when it is captured again. With `smooth`, the
    /// turn may continue over later frames even when no new motion arrives.
    pub fn turn(&mut self, seconds: f64, active: bool) -> Option<Look> {
        let (dx, dy) = (self.accumulated_x, self.accumulated_y);
        self.accumulated_x = 0.0;
        self.accumulated_y = 0.0;
        if !active {
            self.smooth_yaw = SmoothDouble::default();
            self.smooth_pitch = SmoothDouble::default();
            return None;
        }

        let scale = sensitivity_scale(self.options.sensitivity);
        let (x, y) = if self.options.smooth {
            (
                self.smooth_yaw.new_delta(dx * scale, seconds),
                self.smooth_pitch.new_delta(dy * scale, seconds),
            )
        } else {
            self.smooth_yaw = SmoothDouble::default();
            self.smooth_pitch = SmoothDouble::default();
            (dx * scale, dy * scale)
        };

        // Horizon's `MouseHandler.turnPlayer` applies the invert there; the
        // sign of pitch is flipped because the camera's pitch is elevation.
        let yaw = if self.options.invert_x { -x } else { x };
        let pitch = if self.options.invert_y { -y } else { y };
        let radians = std::f64::consts::PI / 180.0;
        Some(Look {
            yaw: (yaw * DEGREES_PER_COUNT * radians) as f32,
            pitch: (-pitch * DEGREES_PER_COUNT * radians) as f32,
            dx,
            dy,
        })
    }

    /// Adds wheel motion, in steps, and returns the whole vertical steps now
    /// available (negative scrolls down). Horizontal motion counts when it is
    /// the only axis that moved, as it does for Horizon's hotbar.
    pub fn scroll(&mut self, x: f64, y: f64) -> i32 {
        let (steps_x, steps_y) = self.scroll.accumulate(x, y);
        if steps_y != 0 { steps_y } else { -steps_x }
    }

    /// Replaces the look settings, e.g. to suit how the pointer is captured.
    pub fn set_options(&mut self, options: LookOptions) {
        self.options = options;
    }
}

/// `MouseHandler.turnPlayer`'s `sens`: `(sensitivity * 0.6F + 0.2F)^3 * 8`.
/// Horizon also keeps the `sensitivityMod` (a further `/8`) for the spyglass;
/// the viewer has nothing scoped, so it is not modelled.
fn sensitivity_scale(sensitivity: f64) -> f64 {
    let scaled = sensitivity.clamp(0.0, 1.0) * f64::from(0.6f32) + f64::from(0.2f32);
    scaled * scaled * scaled * 8.0
}

/// How far a full sweep across a confined window should turn the view.
pub const CONFINED_SWEEP_DEGREES: f64 = 120.0;

/// The sensitivity at which travelling `width` counts turns `degrees`.
///
/// The inverse of the vanilla curve: with `scale = degrees / width / 0.15`,
/// `sensitivity = ((scale / 8)^(1/3) - 0.2) / 0.6`, clamped to the slider.
/// A confined pointer can only cross the window once, so the viewer uses this
/// to keep a full sweep from spinning the view.
pub fn sensitivity_for_sweep(width: u32, degrees: f64) -> f64 {
    let width = f64::from(width.max(1));
    let scale = degrees / width / DEGREES_PER_COUNT;
    let root = (scale / 8.0).cbrt();
    ((root - 0.2) / 0.6).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RADIANS_PER_DEGREE: f64 = std::f64::consts::PI / 180.0;

    fn plain() -> LookOptions {
        LookOptions::default()
    }

    fn turn(look: &mut MouseLook, dx: f64, dy: f64) -> Look {
        look.add_motion(dx, dy);
        look.turn(1.0 / 60.0, true).unwrap()
    }

    #[test]
    fn default_sensitivity_turns_point_fifteen_degrees_per_count() {
        // The curve is built from `0.6f32`, so it lands a float epsilon off 1.
        assert!((sensitivity_scale(DEFAULT_SENSITIVITY) - 1.0).abs() < 1e-6);
        let mut look = MouseLook::default();
        let look = turn(&mut look, 100.0, 0.0);
        let expected = (15.0 * RADIANS_PER_DEGREE) as f32;
        assert!((look.yaw - expected).abs() < 1e-6, "{look:?}");
        assert_eq!(look.pitch, 0.0);
    }

    #[test]
    fn pitch_is_positive_up() {
        let mut look = MouseLook::default();
        // winit reports positive `dy` for motion down.
        assert!(turn(&mut look, 0.0, 10.0).pitch < 0.0);
        assert!(turn(&mut look, 0.0, -10.0).pitch > 0.0);
    }

    #[test]
    fn inversion_flips_one_axis() {
        let mut options = plain();
        options.invert_y = true;
        let mut look = MouseLook::new(options);
        assert!(turn(&mut look, 0.0, 10.0).pitch > 0.0);
        assert!(turn(&mut look, 10.0, 0.0).yaw > 0.0);

        options = plain();
        options.invert_x = true;
        let mut look = MouseLook::new(options);
        assert!(turn(&mut look, 10.0, 0.0).yaw < 0.0);
    }

    #[test]
    fn sensitivity_curve_matches_vanilla_at_the_ends() {
        for sensitivity in [0.0, 0.5, 1.0] {
            let scaled = sensitivity * f64::from(0.6f32) + f64::from(0.2f32);
            assert!(
                (sensitivity_scale(sensitivity) - scaled.powi(3) * 8.0).abs() < 1e-12,
                "{sensitivity}"
            );
        }
        // The slider is clamped, so a hand-edited 2.0 behaves like 1.0.
        assert_eq!(sensitivity_scale(2.0), sensitivity_scale(1.0));
        assert_eq!(sensitivity_scale(-1.0), sensitivity_scale(0.0));
    }

    #[test]
    fn inactive_frames_drop_motion() {
        let mut look = MouseLook::default();
        look.add_motion(50.0, 50.0);
        assert_eq!(look.turn(1.0 / 60.0, false), None);
        assert_eq!(look.turn(1.0 / 60.0, false), None);
        // Nothing was banked: the first active frame has no motion.
        assert_eq!(
            look.turn(1.0 / 60.0, true),
            Some(Look {
                yaw: 0.0,
                pitch: 0.0,
                dx: 0.0,
                dy: 0.0
            })
        );
    }

    #[test]
    fn smoothing_spreads_the_same_total_over_frames() {
        let options = LookOptions {
            smooth: true,
            ..plain()
        };
        let mut look = MouseLook::new(options);
        let total = 30.0;

        look.add_motion(0.0, 0.0);
        look.turn(0.0, true);
        // The default sensitivity turns 0.15 degrees per count.
        look.add_motion(total / 0.15, 0.0);
        let first = look.turn(1.0 / 60.0, true).unwrap().yaw;
        let expected = (total * RADIANS_PER_DEGREE) as f32;
        assert!(first > 0.0 && first < expected, "{first}");

        let mut sum = first;
        for _ in 0..600 {
            sum += look.turn(1.0 / 60.0, true).unwrap().yaw;
        }
        assert!((sum - expected).abs() < 1e-3, "{sum} vs {expected}");
    }

    #[test]
    fn sweep_sensitivity_turns_the_requested_angle() {
        for width in [320, 1280, 2560] {
            let sensitivity = sensitivity_for_sweep(width, 120.0);
            let turn = DEGREES_PER_COUNT * sensitivity_scale(sensitivity) * f64::from(width);
            assert!((turn - 120.0).abs() < 1e-3, "{width}: {turn}");
        }
    }

    #[test]
    fn wheel_accumulates_fractions_and_whole_steps() {
        let mut look = MouseLook::default();
        assert_eq!(look.scroll(0.0, 0.4), 0);
        assert_eq!(look.scroll(0.0, 0.7), 1);
        assert_eq!(look.scroll(0.0, 0.0), 0);
        assert_eq!(look.scroll(0.0, -2.0), -2);
    }

    #[test]
    fn horizontal_motion_counts_only_without_vertical() {
        let mut look = MouseLook::default();
        assert_eq!(look.scroll(1.0, 0.0), -1);
        assert_eq!(look.scroll(0.0, 1.0), 1);
        assert_eq!(look.scroll(0.5, 0.0), 0);
        assert_eq!(look.scroll(0.5, 0.0), -1);
    }

    #[test]
    fn direction_change_drops_the_old_fraction() {
        let mut look = MouseLook::default();
        // 0.9 of a step up is forgotten when the wheel turns the other way, so
        // a full step down takes effect at once.
        assert_eq!(look.scroll(0.0, 0.9), 0);
        assert_eq!(look.scroll(0.0, -1.1), -1);
    }
}
