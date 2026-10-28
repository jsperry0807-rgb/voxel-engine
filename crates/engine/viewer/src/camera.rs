//! First-person fly camera.
//!
//! The world is right-handed and Y-up. Yaw is measured from `-Z` toward `+X`,
//! and pitch is elevation above the horizon. `W`/`S` move along the view
//! direction (including pitch) and `A`/`D` strafe, so the camera can fly.
//!
//! Matrices follow the WebGPU convention: a right-handed view matrix and a
//! projection with a `[0, 1]` depth range.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// Default vertical field of view, in degrees.
pub const DEFAULT_FOV_Y: f32 = 70.0;
/// Near plane distance.
pub const NEAR: f32 = 0.1;
/// Far plane distance.
pub const FAR: f32 = 1_024.0;

/// Pitch is clamped just short of straight up/down so the view's up vector
/// never becomes parallel to the view direction.
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.001;
const TAU: f32 = std::f32::consts::TAU;

/// A fly camera with a position and Euler yaw/pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    aspect: f32,
    fov_y: f32,
    near: f32,
    far: f32,
}

impl Camera {
    /// Creates a camera at `position` looking down `-Z`, with `aspect` as the
    /// viewport width divided by height.
    pub fn new(position: Vec3, aspect: f32) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            aspect,
            fov_y: DEFAULT_FOV_Y.to_radians(),
            near: NEAR,
            far: FAR,
        }
    }

    /// Sets the viewport aspect ratio, ignoring non-finite or non-positive
    /// values.
    pub fn set_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect > 0.0 {
            self.aspect = aspect;
        }
    }

    /// Moves the camera in world space.
    pub fn translate(&mut self, offset: Vec3) {
        self.position += offset;
    }

    /// Rotates by `delta_yaw` and `delta_pitch` radians.
    ///
    /// Yaw wraps into `[0, 2π)`; pitch is clamped just short of the poles.
    pub fn rotate(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = (self.yaw + delta_yaw).rem_euclid(TAU);
        self.pitch = (self.pitch + delta_pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Unit forward vector, including pitch.
    pub fn forward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// Unit right vector, horizontal regardless of pitch.
    pub fn right(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(cos_yaw, 0.0, sin_yaw)
    }

    /// Yaw in radians, measured from `-Z` toward `+X`, wrapped into `[0, 2π)`.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// Pitch in radians, elevation above the horizon, clamped short of the
    /// poles.
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// World-to-view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.position + self.forward(), Vec3::Y)
    }

    /// View-to-clip matrix with a `[0, 1]` depth range.
    pub fn projection(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far)
    }

    /// Combined world-to-clip matrix.
    pub fn view_projection(&self) -> Mat4 {
        self.projection() * self.view()
    }

    /// The per-frame uniform payload for the GPU.
    pub fn uniform(&self) -> CameraUniform {
        CameraUniform {
            view_proj: self.view_projection().to_cols_array_2d(),
            view: self.view().to_cols_array_2d(),
            proj: self.projection().to_cols_array_2d(),
            position: self.position.extend(1.0).to_array(),
        }
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new(Vec3::new(0.0, 0.0, 8.0), 16.0 / 9.0)
    }
}

/// Camera data uploaded to the GPU once per frame.
///
/// Three column-major `mat4x4<f32>` values (combined, view, projection) are
/// followed by the world-space position in a `vec4<f32>` whose fourth
/// component is 1.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub position: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-5,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn assert_matrix_close(actual: Mat4, expected: Mat4) {
        let difference = (actual - expected).to_cols_array();
        assert!(
            difference.iter().all(|value| value.abs() < 1e-5),
            "matrices differ:\nactual: {actual:?}\nexpected: {expected:?}"
        );
    }

    #[test]
    fn view_matches_the_known_look_at_case() {
        let eye = Vec3::new(0.0, 0.0, 5.0);
        let camera = Camera::new(eye, 1.0);

        assert_close(camera.forward(), Vec3::NEG_Z);
        assert_matrix_close(camera.view(), Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y));
    }

    #[test]
    fn view_places_the_camera_at_the_origin() {
        let camera = Camera::new(Vec3::new(3.0, 4.0, 5.0), 1.0);
        let origin = camera.view().transform_point3(camera.position);

        assert_close(origin, Vec3::ZERO);
    }

    #[test]
    fn projection_maps_near_and_far_to_zero_and_one() {
        let camera = Camera::new(Vec3::ZERO, 1.0);
        let view_projection = camera.view_projection();
        let on_near = view_projection.project_point3(Vec3::new(0.0, 0.0, -camera.near));
        let on_far = view_projection.project_point3(Vec3::new(0.0, 0.0, -camera.far));

        assert!(on_near.z.abs() < 1e-5, "near depth was {}", on_near.z);
        assert!((on_far.z - 1.0).abs() < 1e-4, "far depth was {}", on_far.z);
    }

    #[test]
    fn center_of_view_projects_to_the_screen_center() {
        let camera = Camera::new(Vec3::new(1.0, 2.0, 3.0), 16.0 / 9.0);
        let target = camera.position + camera.forward() * 10.0;
        let ndc = camera.view_projection().project_point3(target);

        assert!(ndc.x.abs() < 1e-5, "x was {}", ndc.x);
        assert!(ndc.y.abs() < 1e-5, "y was {}", ndc.y);
        assert!((0.0..=1.0).contains(&ndc.z), "z was {}", ndc.z);
    }

    #[test]
    fn yaw_turns_right_and_pitch_is_clamped() {
        let mut camera = Camera::default();

        camera.rotate(std::f32::consts::FRAC_PI_2, 0.0);
        assert_close(camera.forward(), Vec3::X);

        camera.rotate(0.0, 10.0);
        assert!(camera.pitch <= MAX_PITCH);
        assert!(camera.forward().y > 0.999);
    }

    #[test]
    fn translate_moves_the_position() {
        let mut camera = Camera::default();
        let start = camera.position;

        camera.translate(Vec3::new(1.0, -2.0, 3.0));

        assert_eq!(camera.position, start + Vec3::new(1.0, -2.0, 3.0));
    }

    #[test]
    fn aspect_ignores_invalid_values() {
        let mut camera = Camera::default();

        camera.set_aspect(2.0);
        assert_eq!(camera.aspect, 2.0);

        camera.set_aspect(0.0);
        camera.set_aspect(-1.0);
        camera.set_aspect(f32::NAN);
        assert_eq!(camera.aspect, 2.0);
    }

    #[test]
    fn uniform_matches_the_camera() {
        let camera = Camera::default();
        let uniform = camera.uniform();
        let position = camera.position.to_array();

        assert_eq!(
            uniform.view_proj,
            camera.view_projection().to_cols_array_2d()
        );
        assert_eq!(uniform.view, camera.view().to_cols_array_2d());
        assert_eq!(uniform.proj, camera.projection().to_cols_array_2d());
        assert_eq!(
            uniform.position,
            [position[0], position[1], position[2], 1.0]
        );
        assert_eq!(std::mem::size_of::<CameraUniform>(), 208);
    }
}
