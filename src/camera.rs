use glam::{Mat4, Vec3};

use crate::input::{GameAction, InputState};
use crate::params::CameraParams;

pub struct IsometricCamera {
    pub target: Vec3,
    /// Smoothed render position - exponentially follows `target`.
    /// All rendering (view matrix, snap) uses this, keeping the
    /// sub-pixel offset continuous and eliminating pixel jitter.
    pub smooth_target: Vec3,
    pub zoom: f32,
    pub rotation: f32,
    pub target_rotation: f32,
    pub aspect: f32,
    pan_speed: f32,
    smooth_speed: f32,
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

    /// Advance camera state using abstract input actions.
    pub fn update(&mut self, dt: f32, input: &InputState, cam_params: &CameraParams) {
        let forward_dir = Vec3::new(-self.rotation.cos(), 0.0, -self.rotation.sin());
        let right_dir = Vec3::new(self.rotation.sin(), 0.0, -self.rotation.cos());

        let mut move_dir = Vec3::ZERO;
        if input.pressed(GameAction::CameraPanForward) { move_dir += forward_dir; }
        if input.pressed(GameAction::CameraPanBackward) { move_dir -= forward_dir; }
        if input.pressed(GameAction::CameraPanLeft) { move_dir -= right_dir; }
        if input.pressed(GameAction::CameraPanRight) { move_dir += right_dir; }

        if move_dir.length_squared() > 0.0 {
            move_dir = move_dir.normalize();
        }

        self.target += move_dir * self.pan_speed * dt;

        // Edge-triggered rotation
        if input.just_pressed(GameAction::CameraRotateLeft) {
            self.target_rotation += std::f32::consts::FRAC_PI_2;
        }
        if input.just_pressed(GameAction::CameraRotateRight) {
            self.target_rotation -= std::f32::consts::FRAC_PI_2;
        }
        
        // Scroll zoom
        let zoom_in = input.value(GameAction::CameraZoomIn);
        let zoom_out = input.value(GameAction::CameraZoomOut);
        let scroll = zoom_in - zoom_out;
        if scroll.abs() > f32::EPSILON {
            self.zoom = (self.zoom - scroll * cam_params.scroll_speed)
                .clamp(cam_params.zoom_min, cam_params.zoom_max);
        }
        
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