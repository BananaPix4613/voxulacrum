use glam::{Mat4, Vec3};
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::params::CameraParams;

pub struct IsometricCamera {
    pub target: Vec3,
    /// Smoothed render position — exponentially follows `target`.
    /// All rendering (view matrix, snap) uses this, keeping the
    /// sub-pixel offset continuous and eliminating pixel jitter.
    pub smooth_target: Vec3,
    pub zoom: f32,
    pub rotation: f32,
    pub target_rotation: f32,
    pub aspect: f32,
    pan_speed: f32,
    smooth_speed: f32,
    forward_pressed: bool,
    backward_pressed: bool,
    left_pressed: bool,
    right_pressed: bool,
    rotate_left_pressed: bool,
    rotate_right_pressed: bool,
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
        let initial = Vec3::new(64.0, 32.0, 64.0);
        Self {
            target: initial,
            smooth_target: initial,
            zoom: params.initial_zoom,
            rotation: std::f32::consts::FRAC_PI_4,
            target_rotation: std::f32::consts::FRAC_PI_4,
            aspect: 1.0,
            pan_speed: params.pan_speed,
            smooth_speed: params.smooth_speed,
            forward_pressed: false,
            backward_pressed: false,
            left_pressed: false,
            right_pressed: false,
            rotate_left_pressed: false,
            rotate_right_pressed: false,
        }
    }

    /// Sync mutable fields with current params each frame.
    pub fn apply_params(&mut self, params: &CameraParams) {
        self.pan_speed = params.pan_speed;
        self.smooth_speed = params.smooth_speed;
    }

    fn view_matrix_for(&self, target: Vec3) -> Mat4 {
        let pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        let direction = Vec3::new(
            self.rotation.cos() * pitch.cos(),
            pitch.sin(),
            self.rotation.sin() * pitch.cos(),
        );
        let eye = target + direction * 150.0;
        Mat4::look_at_rh(eye, target, Vec3::Y)
    }

    pub fn view_matrix(&self) -> Mat4 {
        self.view_matrix_for(self.smooth_target)
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
                KeyCode::KeyQ => {
                    if pressed && !self.rotate_left_pressed {
                        self.target_rotation += std::f32::consts::FRAC_PI_2;
                    }
                    self.rotate_left_pressed = pressed;
                }
                KeyCode::KeyE => {
                    if pressed && !self.rotate_right_pressed {
                        self.target_rotation -= std::f32::consts::FRAC_PI_2;
                    }
                    self.rotate_right_pressed = pressed;
                }
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

        // Exponentially smooth the render position toward the logical target.
        // alpha approaches 1 as dt grows, clamped so it never overshoots.
        let alpha = (self.smooth_speed * dt).exp().recip().mul_add(-1.0, 1.0).clamp(0.0, 1.0);
        self.smooth_target = self.smooth_target.lerp(self.target, alpha);

        // Smooth rotation toward target (shortest path via angle wrapping)
        use std::f32::consts::{PI, TAU};
        let rot_alpha = (self.smooth_speed * dt).exp().recip().mul_add(-1.0, 1.0).clamp(0.0, 1.0);
        let mut delta = self.target_rotation - self.rotation;
        delta = (delta + PI).rem_euclid(TAU) - PI;
        self.rotation += delta * rot_alpha;
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }

    /// Compute a texel-snapped view-projection matrix.
    ///
    /// Projects a fixed world reference point (the origin) into texel space,
    /// computes the fractional offset from the nearest texel center, and nudges
    /// the projection matrix to cancel that fraction.  This locks the world-space
    /// grid to the texel grid so voxel edges land on stable texel boundaries
    /// every frame, eliminating "pixel swimming" at low resolution.
    ///
    /// The previous approach projected the camera *target* through its own VP,
    /// which always lands at clip-space (0,0) in an orthographic projection —
    /// making the snap a no-op.  Using a fixed reference point that actually
    /// moves in clip space as the camera pans is what makes this work.
    pub fn snap_camera(
        &self,
        render_width: u32,
        render_height: u32,
        pixel_scale: f32,
    ) -> SnappedCamera {
        let content_w = render_width as f32;
        let content_h = render_height as f32;
        let tex_w = content_w + 2.0;  // 1-texel border each side
        let tex_h = content_h + 2.0;

        // Expand the orthographic projection to fill the border area.
        let mut proj = self.projection_matrix();
        proj.x_axis.x *= content_w / tex_w;
        proj.y_axis.y *= content_h / tex_h;

        let view = self.view_matrix(); // centered on smooth_target
        let vp = proj * view;

        // Project the world origin into texel space.
        // NDC [-1,+1] spans tex_w texels with the expanded projection.
        let origin_clip = vp.project_point3(Vec3::ZERO);
        let tex_half_w = tex_w * 0.5;
        let tex_half_h = tex_h * 0.5;
        let texel_x = origin_clip.x * tex_half_w;
        let texel_y = origin_clip.y * tex_half_h;

        // Fractional offset from the nearest texel center.
        let frac_x = texel_x - texel_x.round();
        let frac_y = texel_y - texel_y.round();

        // Nudge the projection to cancel the fractional offset.
        // This shifts clip space so the origin (and every integer world
        // coordinate) lands exactly on a texel center.
        let mut snapped_proj = proj;
        snapped_proj.w_axis.x -= frac_x / tex_half_w;
        snapped_proj.w_axis.y -= frac_y / tex_half_h;

        let snapped_vp = snapped_proj * view;

        // Sub-pixel offset in native window pixels, expressed in the upscale
        // shader's UV coordinate system (X = right, Y = down).
        //
        // The snap operates in clip/NDC space (X = right, Y = UP), but the
        // shader compensates in UV space (X = right, Y = DOWN).  Y is flipped
        // between the two, so frac_y's sign naturally inverts when the shader
        // adds the offset.  X shares orientation, so we negate frac_x here to
        // make `texel_coord + texel_shift` correct on both axes.
        let subpixel_offset = [
            -frac_x * pixel_scale,
            frac_y * pixel_scale,
        ];

        SnappedCamera {
            view_proj: snapped_vp.to_cols_array_2d(),
            subpixel_offset,
        }
    }
}