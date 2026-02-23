use glam::{Mat4, Vec3, Vec2, Vec4Swizzles};
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::params::CameraParams;

pub struct IsometricCamera {
    pub target: Vec3,
    pub zoom: f32,
    pub rotation: f32,
    pub aspect: f32,
    pan_speed: f32,
    forward_pressed: bool,
    backward_pressed: bool,
    left_pressed: bool,
    right_pressed: bool,
}

/// Result of camera snapping: a texel-aligned VP matrix and a sub-pixel
/// correction to apply during the upscale blit.
pub struct SnappedCamera {
    /// View-projection matrix with the projection translated so that the
    /// camera target falls exactly on a texel center.
    pub view_proj: [[f32; 4]; 4],
    /// Sub-pixel offset in **native-resolution pixels**.  The upscale shader
    /// shifts its UV by this amount to restore smooth apparent motion.
    pub subpixel_offset: [f32; 2],
}

impl IsometricCamera {
    pub fn new(params: &CameraParams) -> Self {
        Self {
            target: Vec3::new(64.0, 32.0, 64.0),
            zoom: params.initial_zoom,
            rotation: std::f32::consts::FRAC_PI_4,
            aspect: 1.0,
            pan_speed: params.pan_speed,
            forward_pressed: false,
            backward_pressed: false,
            left_pressed: false,
            right_pressed: false,
        }
    }

    /// Sync mutable fields with current params each frame.
    pub fn apply_params(&mut self, params: &CameraParams) {
        self.pan_speed = params.pan_speed;
    }

    pub fn view_matrix(&self) -> Mat4 {
        let pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        let direction = Vec3::new(
            self.rotation.cos() * pitch.cos(),
            pitch.sin(),
            self.rotation.sin() * pitch.cos(),
        );
        let eye = self.target + direction * 150.0;
        Mat4::look_at_rh(eye, self.target, Vec3::Y)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        let half_height = self.zoom;
        let half_width = half_height * self.aspect;
        Mat4::orthographic_rh(-half_width, half_width, -half_height, half_height, 0.1, 300.0)
    }

    pub fn view_projection(&self) -> [[f32; 4]; 4] {
        (self.projection_matrix() * self.view_matrix()).to_cols_array_2d()
    }

    pub fn process_keyboard(&mut self, key: PhysicalKey, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if let PhysicalKey::Code(code) = key {
            match code {
                KeyCode::KeyW | KeyCode::ArrowUp => self.forward_pressed = pressed,
                KeyCode::KeyS | KeyCode::ArrowDown => self.backward_pressed = pressed,
                KeyCode::KeyA | KeyCode::ArrowLeft => self.left_pressed = pressed,
                KeyCode::KeyD | KeyCode::ArrowRight => self.right_pressed = pressed,
                _ => {}
            }
        }
    }

    pub fn process_scroll(&mut self, delta: &MouseScrollDelta, params: &CameraParams) {
        let scroll = match delta {
            MouseScrollDelta::LineDelta(_, y) => *y,
            MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.1,
        };
        self.zoom = (self.zoom - scroll * params.scroll_speed).clamp(params.zoom_min, params.zoom_max);
    }

    pub fn update(&mut self, dt: f32) {
        let forward_dir = Vec3::new(-self.rotation.cos(), 0.0, -self.rotation.sin());
        let right_dir = Vec3::new(self.rotation.sin(), 0.0, -self.rotation.cos());

        let mut move_dir = Vec3::ZERO;
        if self.forward_pressed { move_dir += forward_dir; }
        if self.backward_pressed { move_dir -= forward_dir; }
        if self.left_pressed { move_dir -= right_dir; }
        if self.right_pressed { move_dir += right_dir; }

        if move_dir.length_squared() > 0.0 {
            move_dir = move_dir.normalize();
        }

        self.target += move_dir * self.pan_speed * dt;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }

    /// Compute a texel-snapped view-projection matrix.
    ///
    /// The idea: project the camera target into clip space, round to the
    /// nearest texel of the low-res render target, then nudge the projection
    /// matrix by the rounding error.  This guarantees that world-space voxel
    /// edges land on the same texel boundaries every frame, eliminating the
    /// "pixel swimming" artifact at low resolution.
    ///
    /// `pixel_scale` is needed to convert the snap error back into
    /// native-resolution pixels for the upscale sub-pixel correction.
    pub fn snap_camera(
        &self,
        render_width: u32,
        render_height: u32,
        pixel_scale: f32,
    ) -> SnappedCamera {
        let view = self.view_matrix();
        let proj = self.projection_matrix();
        let vp = proj * view;
        
        // Project camera target into NDC (clip space, [-1, 1]).
        let target_clip = vp.project_point3(self.target);
        
        // Convert NDC to texel coordinates in the low-res target.
        let half_w = render_width as f32 * 0.5;
        let half_h = render_height as f32 * 0.5;
        let pixel_x = target_clip.x * half_w;
        let pixel_y = target_clip.y * half_h;
        
        // Snap to nearest texel center.
        let snapped_x = pixel_x.round();
        let snapped_y = pixel_y.round();
        
        // Error in NDC units.
        let error_x = (pixel_x - snapped_x) / half_w;
        let error_y = (pixel_y - snapped_y) / half_h;
        
        // Nudge the projection so the target lands on the snapped texel.
        let mut snapped_proj = proj;
        // Column-major: w_axis is column 3 (the translation column).
        snapped_proj.w_axis.x -= error_x;
        snapped_proj.w_axis.y -= error_y;
        
        let snapped_vp = snapped_proj * view;
        
        // Sub-pixel offset for the upscale blit, in native-resolution pixels.
        let subpixel_offset = [
            (pixel_x - snapped_x) * pixel_scale,
            (pixel_y - snapped_y) * pixel_scale,
        ];
        
        SnappedCamera {
            view_proj: snapped_vp.to_cols_array_2d(),
            subpixel_offset,
        }
    }
}