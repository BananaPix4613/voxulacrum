//! Simple orbit camera around the chunk center.

use glam::{Mat4, Vec3};

/// Orbit camera state. Eye = `target + spherical(yaw, pitch, distance)`.
#[derive(Clone, Copy)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::splat(16.0), // chunk center
            yaw: 0.6,
            pitch: -0.5,
            distance: 60.0,
            fov_y: 45f32.to_radians(),
        }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let r = self.distance;
        let cp = self.pitch.cos();
        let dir = Vec3::new(
            self.yaw.cos() * cp,
            self.pitch.sin(),
            self.yaw.sin() * cp,
        );
        self.target + dir * r
    }
    
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, aspect, 0.1, 1024.0);
        proj * view
    }
    
    /// `dx`, `dy` are pointer drag deltas in pixels (egui's drag_delta:
    /// positive y when the pointer moves down). Drag up → camera rises
    /// (eye goes above target, looking down more); drag down → camera
    /// descends. This matches the typical "grab the model and rotate it"
    /// orbit feel.
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        const SENS: f32 = 0.005;
        self.yaw += dx * SENS;
        self.pitch = (self.pitch + dy * SENS).clamp(-1.5, 1.5);
    }
    
    /// `scroll` is a positive-toward-camera wheel delta.
    pub fn zoom(&mut self, scroll: f32) {
        let factor = (scroll * -0.002).exp();
        self.distance = (self.distance * factor).clamp(8.0, 256.0);
    }
}
