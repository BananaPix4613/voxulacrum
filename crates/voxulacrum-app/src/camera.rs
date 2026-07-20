use glam::{Mat4, Vec3};

use crate::input::{GameAction, InputState};
use crate::params::CameraParams;

pub struct IsometricCamera {
    pub target: Vec3,
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
        // Scale eye distance with zoom so terrain never clips the near plane
        let eye_dist = 150.0_f32.max(self.zoom * 3.0);
        let eye = target + direction * eye_dist;
        Mat4::look_at_rh(eye, target, Vec3::Y)
    }

    pub fn view_matrix(&self) -> Mat4 {
        self.view_matrix_for(self.smooth_target)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        let half_height = self.zoom;
        let half_width = half_height * self.aspect;
        Mat4::orthographic_rh(-half_width, half_width, -half_height, half_height, 0.001, 1000.0)
    }

    pub fn view_projection(&self) -> [[f32; 4]; 4] {
        (self.projection_matrix() * self.view_matrix()).to_cols_array_2d()
    }

    /// Half-extent (world units) of a cube around the camera target that's
    /// guaranteed to cover everything visible on screen, for any camera
    /// rotation. Ground-plane reach in the camera's "away" direction is
    /// larger than half_height by 1/sin(pitch), since the isometric tilt
    /// foreshortens that axis; as the camera rotates around Y, "away" sweeps
    /// through every world axis, so the cube has to be sized to the larger of
    /// the two rather than either alone.
    pub fn visible_radius(&self) -> f32 {
        let half_height = self.zoom;
        let half_width = half_height * self.aspect;
        let pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        let ground_reach = half_height / pitch.sin();
        ground_reach.max(half_width)
    }

    /// Advance camera state. In Play mode `follow` is the interpolated player
    /// position to pin to (the sim-tick interpolation is the smoothing, so no
    /// exponential lag); in Authoring it is `None` and WASD free-pans the target.
    pub fn update(
        &mut self,
        dt: f32,
        input: &InputState,
        cam_params: &CameraParams,
        follow: Option<Vec3>,
    ) {
        let forward_dir = Vec3::new(-self.rotation.cos(), 0.0, -self.rotation.sin());
        let right_dir = Vec3::new(self.rotation.sin(), 0.0, -self.rotation.cos());

        // --- Movement source depends on mode ---
        match follow {
            Some(pos) => {
                // Play: pin to the interpolated player position (already smooth).
                self.target = pos;
                self.smooth_target = pos;
            }
            None => {
                // Authoring: WASD free-pan toward `target`, exponentially smoothed.
                let mv = input.move_vector();
                let move_dir = right_dir * mv.x + forward_dir * mv.y;
                self.target += move_dir * self.pan_speed * dt;
                let alpha = (self.smooth_speed * dt).exp().recip().mul_add(-1.0, 1.0).clamp(0.0, 1.0);
                self.smooth_target = self.smooth_target.lerp(self.target, alpha);
            }
        }

        // --- Rotation (Q/E) - edge-triggered, both modes ---
        if input.just_pressed(GameAction::CameraRotateLeft) {
            self.target_rotation += std::f32::consts::FRAC_PI_2;
        }
        if input.just_pressed(GameAction::CameraRotateRight) {
            self.target_rotation -= std::f32::consts::FRAC_PI_2;
        }
        
        // --- Zoom (scroll) - both modes ---
        let zoom_in = input.value(GameAction::CameraZoomIn);
        let zoom_out = input.value(GameAction::CameraZoomOut);
        let scroll = zoom_in - zoom_out;
        if scroll.abs() > f32::EPSILON {
            self.zoom = (self.zoom - scroll * cam_params.scroll_speed)
                .clamp(cam_params.zoom_min, cam_params.zoom_max);
        }

        // --- Rotation smoothing (shortest path) ---
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

    /// Compute a texel-snapped view-projection matrix on the fixed pixel lattice.
    ///
    /// The projection spans the ALLOCATED texel grid at exactly `1/k` world
    /// units per texel - zoom never scales this projection directly (it acts
    /// only through `k`/`s`), so voxel edges land on the same texel boundaries
    /// at every zoom level. The origin snap then cancels sub-texel translation,
    /// and the remainder is returned for the upscale blit to slide smoothly.
    pub fn snap_camera(&self, alloc_w: u32, alloc_h: u32, k: u32, s: f32) -> SnappedCamera {
        let kf = k as f32;
        let half_w = alloc_w as f32 / (2.0 * kf);
        let half_h = alloc_h as f32 / (2.0 * kf);
        let proj = Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, 0.001, 1000.0);

        let view = self.view_matrix(); // centered on smooth_target
        let vp = proj * view;

        // Project the world origin into texel space.
        // NDC [-1,+1] spans the full allocated texture.
        let origin_clip = vp.project_point3(Vec3::ZERO);
        let tex_half_w = alloc_w as f32 * 0.5;
        let tex_half_h = alloc_h as f32 * 0.5;
        let texel_x = origin_clip.x * tex_half_w;
        let texel_y = origin_clip.y * tex_half_h;

        // Fractional offset from the nearest texel center.
        let frac_x = texel_x - texel_x.round();
        let frac_y = texel_y - texel_y.round();

        // Nudge the projection so integer world coordinates land on texel centers.
        let mut snapped_proj = proj;
        snapped_proj.w_axis.x -= frac_x / tex_half_w;
        snapped_proj.w_axis.y -= frac_y / tex_half_h;

        let snapped_vp = snapped_proj * view;
        
        let subpixel_offset = [-frac_x * s, frac_y * s];

        SnappedCamera {
            view_proj: snapped_vp.to_cols_array_2d(),
            subpixel_offset,
        }
    }
}