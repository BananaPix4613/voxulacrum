use glam::{Mat4, Vec3};
use voxel_core::{AreaOfInterest, Footprint, CHUNK_WORLD_SIZE};

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

    /// What this camera's observer needs resident (design §12, "Residency"):
    /// the chunk columns in view over the layers `chunk_y` covers, loading
    /// within `params.load_margin` and staying loaded within
    /// `params.unload_margin`.
    pub fn area_of_interest(&self, chunk_y: (i32, i32), params: &CameraParams) -> AreaOfInterest {
        let center = (
            (self.smooth_target.x / CHUNK_WORLD_SIZE).floor() as i32,
            (self.smooth_target.z / CHUNK_WORLD_SIZE).floor() as i32,
        );
        AreaOfInterest {
            center,
            chunk_y,
            load: self.view_footprint(chunk_y, params.load_margin),
            retain: self.view_footprint(chunk_y, params.unload_margin),
        }
    }

    /// The chunk columns in view: the screen rectangle projected onto the
    /// ground, which turns with the camera, widened by `margin_fraction`.
    ///
    /// For rotation `r` and isometric pitch θ = atan(1/√2), the rectangle's axes
    /// are the screen's on the ground:
    ///   right = (sin(r), -cos(r))   - screen X on the ground
    ///   up    = (-cos(r), -sin(r))  - screen Y on the ground (foreshortened)
    ///
    /// Altitude shifts a chunk up-screen, so a column whose footprint sits
    /// outside the band can still have raised geometry on screen - tall terrain
    /// at the near edge, which once failed to load at all. Each column therefore
    /// spans the up-screen shift of the lowest and highest layers, and is in view
    /// when that span meets the band.
    ///
    /// This is column-granular by necessity: a per-chunk Y test would be
    /// tighter, but skipping a middle layer would leave the layer above it
    /// permanently unmeshable, since meshing requires both Y neighbors resident.
    fn view_footprint(&self, chunk_y: (i32, i32), margin_fraction: f32) -> Footprint {
        let (half_right, half_up) = self.view_half_extents(margin_fraction);
        Footprint::Rect {
            yaw: self.rotation,
            right: (-half_right, half_right),
            up: (-half_up, half_up),
            span: (
                self.altitude_shift_chunks(chunk_y.0 as f32 * CHUNK_WORLD_SIZE),
                self.altitude_shift_chunks(chunk_y.1 as f32 * CHUNK_WORLD_SIZE),
            ),
        }
    }

    /// Ground-equivalent offset, in chunks, along the screen-up axis produced by
    /// a point at altitude `world_y`.
    ///
    /// Under the isometric projection, moving `d` along the ground's away-axis
    /// shifts a point up-screen by `d·sin(pitch)`, while raising it by `Δy`
    /// shifts it up-screen by `Δy·cos(pitch)`. An altitude is therefore
    /// screen-equivalent to `Δy/tan(pitch)` = `Δy·√2` of ground. Altitude has no
    /// screen-X component - the camera's right vector is horizontal - which is
    /// why only the up axis needs correcting.
    fn altitude_shift_chunks(&self, world_y: f32) -> f32 {
        // 1/tan(atan(1/√2)) = √2
        const INV_TAN_PITCH: f32 = std::f32::consts::SQRT_2;
        (world_y - self.smooth_target.y) * INV_TAN_PITCH / CHUNK_WORLD_SIZE
    }

    /// The half-extents of the view rectangle with proportional margin, as
    /// `(half_right, half_up)` in chunks.
    ///
    /// The margin is computed as a fraction of the larger visible axis and then
    /// applied equally to both axes. This ensures the forward/backward direction
    /// (foreshortened by the isometric pitch) gets the same absolute buffer as
    /// the horizontal direction, preventing pop-in when the camera moves forward.
    fn view_half_extents(&self, margin_fraction: f32) -> (f32, f32) {
        const MIN_MARGIN: f32 = 2.0; // minimum buffer in chunks, prevents pop-in at low zoom

        // `zoom` is the orthographic half-height in world units, so the screen-X
        // ground axis covers `zoom * aspect`. The screen-Y ground axis is
        // foreshortened by the isometric tilt, so filling the same screen height
        // takes more ground: `zoom / sin(pitch)`, as `visible_radius` states.
        let sin_pitch = (1.0_f32 / 3.0).sqrt(); // sin(atan(1/√2)) = 1/√3
        let half_right_vis = self.zoom * self.aspect / CHUNK_WORLD_SIZE;
        let half_up_vis = self.zoom / sin_pitch / CHUNK_WORLD_SIZE;

        let margin_chunks = (half_right_vis.max(half_up_vis) * margin_fraction).max(MIN_MARGIN);
        (half_right_vis + margin_chunks, half_up_vis + margin_chunks)
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

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;
    use std::f32::consts::FRAC_PI_4;

    /// The view test streaming ran before R6 (`CameraView` in
    /// `world/streaming.rs` at 35bef22), kept as the reference the camera's area
    /// is checked against.
    struct OldCameraView {
        zoom: f32,
        aspect: f32,
        rotation: f32,
        camera_chunk: IVec3,
        camera_world_pos: Vec3,
        world_y_range: (f32, f32),
    }

    impl OldCameraView {
        fn altitude_shift_chunks(&self, world_y: f32) -> f32 {
            const INV_TAN_PITCH: f32 = std::f32::consts::SQRT_2;
            (world_y - self.camera_world_pos.y) * INV_TAN_PITCH / CHUNK_WORLD_SIZE
        }

        fn is_in_view_rect(&self, chunk_x: i32, chunk_z: i32, margin_fraction: f32) -> bool {
            let (half_right, half_up) = self.half_extents(margin_fraction);
            let dx = (chunk_x - self.camera_chunk.x) as f32;
            let dz = (chunk_z - self.camera_chunk.z) as f32;
            let cos_r = self.rotation.cos();
            let sin_r = self.rotation.sin();
            let local_right = dx * sin_r - dz * cos_r;
            let local_up = -dx * cos_r - dz * sin_r;
            if local_right.abs() > half_right {
                return false;
            }
            let span_lo = local_up + self.altitude_shift_chunks(self.world_y_range.0);
            let span_hi = local_up + self.altitude_shift_chunks(self.world_y_range.1);
            span_lo <= half_up && span_hi >= -half_up
        }

        fn half_extents(&self, margin_fraction: f32) -> (f32, f32) {
            const MIN_MARGIN: f32 = 2.0;
            let sin_pitch = (1.0_f32 / 3.0).sqrt();
            let half_right_vis = self.zoom * self.aspect / CHUNK_WORLD_SIZE;
            let half_up_vis = self.zoom / sin_pitch / CHUNK_WORLD_SIZE;
            let margin_chunks = (half_right_vis.max(half_up_vis) * margin_fraction).max(MIN_MARGIN);
            (half_right_vis + margin_chunks, half_up_vis + margin_chunks)
        }

        fn bounding_radius(&self, margin_fraction: f32) -> i32 {
            let (half_right, half_up) = self.half_extents(margin_fraction);
            let lift = self
                .altitude_shift_chunks(self.world_y_range.0)
                .abs()
                .max(self.altitude_shift_chunks(self.world_y_range.1).abs());
            let half_up = half_up + lift;
            let cos_r = self.rotation.cos().abs();
            let sin_r = self.rotation.sin().abs();
            let aabb_half_x = half_right * sin_r + half_up * cos_r;
            let aabb_half_z = half_right * cos_r + half_up * sin_r;
            aabb_half_x.max(aabb_half_z).ceil() as i32
        }
    }

    /// The camera's area holds exactly the columns the old view test streamed,
    /// for loading and for keeping, and its scan reaches exactly as far.
    #[test]
    fn the_area_holds_the_columns_the_old_view_test_streamed() {
        let params = CameraParams::default();
        let mut camera = IsometricCamera::new(&params);
        let zooms = [params.zoom_min, 12.0, params.initial_zoom, 70.0, params.zoom_max];
        let aspects = [16.0 / 9.0, 4.0 / 3.0, 21.0 / 9.0];
        let rotations = [FRAC_PI_4, 3.0 * FRAC_PI_4, -FRAC_PI_4, -3.0 * FRAC_PI_4, 0.3, 1.9];
        // Inside the layers, far from the origin, below the lowest and above the highest.
        let targets = [
            Vec3::new(64.0, 32.0, 64.0),
            Vec3::new(-517.3, 140.0, 2049.9),
            Vec3::new(10.0, -60.0, -10.0),
            Vec3::new(0.0, 400.0, 0.0),
        ];
        let layers = [(0, 8), (-4, 12)];
        let (mut inside, mut outside) = (0usize, 0usize);
        for &zoom in &zooms {
            for &aspect in &aspects {
                for &rotation in &rotations {
                    for &target in &targets {
                        for &chunk_y in &layers {
                            camera.zoom = zoom;
                            camera.aspect = aspect;
                            camera.rotation = rotation;
                            camera.smooth_target = target;
                            let area = camera.area_of_interest(chunk_y, &params);
                            let old = OldCameraView {
                                zoom,
                                aspect,
                                rotation,
                                camera_chunk: IVec3::new(
                                    (target.x / CHUNK_WORLD_SIZE).floor() as i32,
                                    0,
                                    (target.z / CHUNK_WORLD_SIZE).floor() as i32,
                                ),
                                camera_world_pos: target,
                                world_y_range: (
                                    chunk_y.0 as f32 * CHUNK_WORLD_SIZE,
                                    chunk_y.1 as f32 * CHUNK_WORLD_SIZE,
                                ),
                            };
                            assert_eq!(area.center, (old.camera_chunk.x, old.camera_chunk.z));
                            assert_eq!(area.chunk_y, chunk_y);
                            for (footprint, margin) in [
                                (area.load, params.load_margin),
                                (area.retain, params.unload_margin),
                            ] {
                                let reach = old.bounding_radius(margin);
                                assert_eq!(footprint.reach(), reach, "zoom {zoom}, rotation {rotation}");
                                for dz in -reach - 2..=reach + 2 {
                                    for dx in -reach - 2..=reach + 2 {
                                        let old_in = old.is_in_view_rect(
                                            old.camera_chunk.x + dx,
                                            old.camera_chunk.z + dz,
                                            margin,
                                        );
                                        assert_eq!(
                                            footprint.contains(dx, dz),
                                            old_in,
                                            "zoom {zoom}, aspect {aspect}, rotation {rotation}, \
                                             target {target}, layers {chunk_y:?}, margin {margin}, \
                                             column ({dx}, {dz})",
                                        );
                                        if old_in { inside += 1 } else { outside += 1 }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(inside > 0 && outside > 0, "{inside} inside, {outside} outside");
    }
}
