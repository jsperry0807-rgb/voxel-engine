//! A fixed camera pose for reproducible captures and measurements.
//!
//! The interactive start camera can be rotated by stale pointer events while
//! the window settles, so a screenshot or a frame-time run that wants to be
//! comparable across runs pins its pose instead. The pose is read from two
//! `x,y,z` environment variables, in world units.

use glam::Vec3;

use crate::camera::Camera;

/// Parses and applies one view override.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    eye: Vec3,
    target: Vec3,
}

impl View {
    /// Reads `eye_var` and `target_var`.
    ///
    /// `Ok(None)` when neither is set; an error when only one is set or a value
    /// is not three finite numbers. The caller decides whether that is fatal.
    pub fn from_env(eye_var: &str, target_var: &str) -> Result<Option<Self>, String> {
        match (std::env::var(eye_var), std::env::var(target_var)) {
            (Ok(eye), Ok(target)) => Ok(Some(Self {
                eye: parse_vec3(eye_var, &eye)?,
                target: parse_vec3(target_var, &target)?,
            })),
            (Ok(_), Err(_)) => Err(format!("{eye_var} is set but {target_var} is not")),
            (Err(_), Ok(_)) => Err(format!("{target_var} is set but {eye_var} is not")),
            (Err(_), Err(_)) => Ok(None),
        }
    }

    /// The pinned eye position.
    pub const fn eye(&self) -> Vec3 {
        self.eye
    }

    /// The pinned look-at point.
    pub const fn target(&self) -> Vec3 {
        self.target
    }

    /// A camera at this pose, keeping `base`'s aspect ratio.
    pub fn camera(&self, base: &Camera) -> Camera {
        Camera::looking_at(self.eye, self.target, base.aspect())
    }
}

/// Parses `"x,y,z"` into a point.
fn parse_vec3(name: &str, text: &str) -> Result<Vec3, String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let [x, y, z] = parts.as_slice() else {
        return Err(format!("{name}={text:?} is not x,y,z"));
    };
    match (x.parse::<f32>(), y.parse::<f32>(), z.parse::<f32>()) {
        (Ok(x), Ok(y), Ok(z)) if x.is_finite() && y.is_finite() && z.is_finite() => {
            Ok(Vec3::new(x, y, z))
        }
        _ => Err(format!("{name}={text:?} is not three finite numbers")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_applies_the_eye_and_target() {
        let view = View {
            eye: Vec3::new(14.0, 6.0, 20.0),
            target: Vec3::new(2.0, 1.0, 2.0),
        };
        let base = Camera::default();
        let camera = view.camera(&base);
        assert_eq!(camera.position(), view.eye());
        assert_eq!(camera.aspect(), base.aspect());
    }

    #[test]
    fn parsing_accepts_spaces_and_rejects_junk() {
        assert_eq!(
            parse_vec3("X", " 1, 2 ,3 ").expect("parses"),
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert!(parse_vec3("X", "1,2").is_err());
        assert!(parse_vec3("X", "1,2,three").is_err());
        assert!(parse_vec3("X", "1,2,inf").is_err());
    }
}
