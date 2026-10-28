use glam::{Mat4, Vec3};
use bevy_ecs::prelude::Resource;

use crate::rendering::frame_state::FrameState;
use crate::rendering::render_dims::compute_render_dimensions;
use crate::camera::{IsometricCamera, SnappedCamera};
use crate::input::InputState;
use crate::cloud_shadow::CloudShadowState;
use crate::params::EngineParams;
use crate::rendering::camera_desc::{AreaOfInterest, CameraDesc, IsoCameraBlock};
use crate::rendering::frustum::Frustum;
use crate::simulation::time_of_day::TimeOfDay;
use crate::simulation::wind::WindState;
use crate::world::chunk::VOXEL_SCALE;

#[derive(Resource)]
pub struct SimulationManager {
    pub camera: IsometricCamera,
    pub time_of_day: TimeOfDay,
    pub wind: WindState,
    pub cloud_shadow: CloudShadowState,
    /// What the camera culls against, held still while culling is frozen.
    /// Read through `FrameState::camera`.
    frustum: Frustum,
    /// The simulation time `advance_world_time` last moved world time to, in
    /// seconds.
    sim_seconds: f64,
}

impl SimulationManager {
    pub fn new(camera: IsometricCamera, cloud_shadow: CloudShadowState) -> Self {
        let vp = camera.projection_matrix() * camera.view_matrix();
        Self {
            camera,
            time_of_day: TimeOfDay::new(),
            wind: WindState::new(),
            cloud_shadow,
            frustum: Frustum::from_view_projection(vp),
            sim_seconds: 0.0,
        }
    }

    /// Move time of day and wind to `sim_seconds` of simulation time: the ticks
    /// run so far plus the frame's interpolation fraction. Both follow the
    /// tick, so they stop when the simulation stops and slow when it drops
    /// ticks. They live here, on the client, until world time crosses the
    /// boundary in `ServerUpdate` (R5).
    pub fn advance_world_time(&mut self, sim_seconds: f64, params: &EngineParams) {
        let dt = (sim_seconds - self.sim_seconds) as f32;
        self.sim_seconds = sim_seconds;
        self.time_of_day.update(dt, &params.time_control);
        self.wind.update(sim_seconds as f32, &params.wind);
    }

    /// Advance the camera and frustum by one frame and build its `FrameState`.
    /// Time of day and wind come from [`Self::advance_world_time`], called
    /// first. `surface` is the window's drawable size in pixels, at least 1 on
    /// each axis. `dt` and `elapsed` are the frame's presentation time
    /// (`PresentationClock`).
    pub fn tick(
        &mut self,
        params: &EngineParams,
        surface: (u32, u32),
        input: &InputState,
        follow: Option<Vec3>,
        dt: f32,
        elapsed: f32,
    ) -> FrameState {
        // Camera
        self.camera.apply_params(&params.camera);
        self.camera.update(dt, input, &params.camera, follow);

        // The lattice this frame is drawn on, from the zoom just updated.
        let dims = compute_render_dimensions(
            surface.0,
            surface.1,
            self.camera.zoom,
            params.render_pipeline.world_pixel_density,
        );
        
        let snapped = if params.render_pipeline.camera_snap_enabled {
            self.camera.snap_camera(dims.alloc_w, dims.alloc_h, dims.k, dims.s)
        } else {
            SnappedCamera {
                view_proj: self.camera.view_projection(),
                subpixel_offset: [0.0; 2],
            }
        };

        // Frustum
        if !params.debug.freeze_culling {
            // Cull against the actual rendered lattice (snapped, alloc-sized
            // projection) — a conservative superset of the visible view, and
            // consistent with the stepped magnification rather than the
            // continuous control zoom.
            let vp = glam::Mat4::from_cols_array_2d(&snapped.view_proj);
            self.frustum = Frustum::from_view_projection(vp);
        }

        // Cross-section clip bounds
        let cos_r = self.camera.target_rotation.cos();
        let sin_r = self.camera.target_rotation.sin();
        let (clip_min, clip_max, clip_enabled) = compute_clip_bounds(
            &self.camera, params, cos_r, sin_r,
        );
        let clip_dirs = [
            if cos_r >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            if sin_r >= 0.0 { 1.0 } else { -1.0 },
        ];

        // Camera view direction (into the scene). Uses the smoothed `rotation` so it
        // matches the rendered view exactly during a rotation lerp — it's a plain
        // uniform now, so tracking it per frame costs nothing.
        let view_pitch = (1.0_f32 / 2.0_f32.sqrt()).atan();
        let view_dir = [
            -(self.camera.rotation.cos() * view_pitch.cos()),
            -view_pitch.sin(),
            -(self.camera.rotation.sin() * view_pitch.cos()),
        ];
        // right = normalize(forward x world_up); the pitch terms cancel, leaving
        // a pure yaw. up = right x forward. Both unit by construction, so the
        // shader can use them without renormalizing.
        let view_right = [
            self.camera.rotation.sin(),
            0.0,
            -self.camera.rotation.cos(),
        ];
        let view_up = [
            -self.camera.rotation.cos() * view_pitch.sin(),
            view_pitch.cos(),
            -self.camera.rotation.sin() * view_pitch.sin(),
        ];

        // Debug mode
        let debug_mode = compute_debug_mode(&params.debug);

        // Warm tint
        let (warm_tint_color, warm_tint_strength) = self.time_of_day.warm_tint();

        // Shadow frustum coverage: scaled to the camera's actual current zoom
        // rather than a fixed worst-case constant, so the shadow map's fixed
        // texel budget isn't spent on ground far outside what's ever drawn.
        // The 1.25x/+15 margin covers casters just outside the visible
        // frustum (so their shadows don't pop in/out at the frustum edge) and
        // the lag between `target` and the smoothed `smooth_target` actually
        // used here. Floored so a fully zoomed-in camera doesn't shrink the
        // frustum enough to clip tall/deep terrain or cause visible pop-in
        // while panning.
        let shadow_radius = (self.camera.visible_radius() * 1.25 + 15.0).max(40.0);

        let view = self.camera.view_matrix();
        let projection = self.camera.projection_matrix();
        // The cube `visible_radius` describes. Streaming still runs its own view
        // test; R6 derives this area from it.
        let center = self.camera.smooth_target;
        let reach = self.camera.visible_radius();
        let camera = CameraDesc {
            view,
            projection,
            view_proj: projection * view,
            frustum: self.frustum,
            right: Vec3::from(view_right),
            up: Vec3::from(view_up),
            forward: Vec3::from(view_dir),
            area_of_interest: AreaOfInterest {
                center,
                horizontal_radius: reach,
                vertical_range: (center.y - reach, center.y + reach),
            },
            iso: IsoCameraBlock {
                snapped_view_proj: Mat4::from_cols_array_2d(&snapped.view_proj),
                snap_offset: snapped.subpixel_offset,
                rotation: self.camera.rotation,
                pitch: view_pitch,
            },
        };
        
        FrameState {
            dt,
            elapsed,
            camera,
            dims,
            light_space: self.time_of_day.light_space_matrix(self.camera.smooth_target, shadow_radius),
            sun_direction: self.time_of_day.sun_direction().into(),
            sun_color: self.time_of_day.sun_color(&params.lighting).into(),
            ambient_color: self.time_of_day.ambient_color(&params.lighting).into(),
            wind_vector: self.wind.wind_vector.into(),
            clip_min,
            clip_max,
            clip_enabled,
            mask_origin: [0.0; 3],
            mask_enabled: 0,
            volume_radius: 0.0,
            debug_mode,
            sky_color: params.render_pipeline.sky_color,
            warm_tint_color: warm_tint_color.into(),
            warm_tint_strength,
            clip_dirs,
        }
    }
}

fn compute_clip_bounds(
    camera: &IsometricCamera,
    params: &EngineParams,
    cos_r: f32,
    sin_r: f32,
) -> ([f32; 3], [f32; 3], u32) {
    if !params.cross_section.enabled {
        return ([-10000.0_f32; 3], [10000.0_f32; 3], 0);
    }

    let cs = &params.cross_section;
    let cam = camera.smooth_target;
    let half = params.camera.zoom_max;

    // X-axis
    let (clip_min_x, clip_max_x) = if cs.x_offset > 0.0 {
        if cos_r >= 0.0 {
            let raw = cam.x + half - cs.x_offset;
            (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
        } else {
            let raw = cam.x - half + cs.x_offset;
            ((raw / VOXEL_SCALE).ceil() * VOXEL_SCALE, 10000.0)
        }
    } else {
        (-10000.0, 10000.0)
    };

    // Y-axis
    let (clip_min_y, clip_max_y) = if cs.y_offset > 0.0 {
        let raw = cam.y + half - cs.y_offset;
        (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
    } else {
        (-10000.0, 10000.0)
    };

    // Z-axis
    let (clip_min_z, clip_max_z) = if cs.z_offset > 0.0 {
        if sin_r >= 0.0 {
            let raw = cam.z + half - cs.z_offset;
            (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
        } else {
            let raw = cam.z - half + cs.z_offset;
            ((raw / VOXEL_SCALE).ceil() * VOXEL_SCALE, 10000.0)
        }
    } else {
        (-10000.0, 10000.0)
    };

    (
        [clip_min_x, clip_min_y, clip_min_z],
        [clip_max_x, clip_max_y, clip_max_z],
        1,
    )
}

fn compute_debug_mode(debug: &crate::params::DebugParams) -> u32 {
    if debug.show_material_ids {
        1
    } else if debug.show_ao_only {
        2
    } else if debug.show_normals {
        3
    } else if debug.show_greedy_debug {
        4
    } else {
        0
    }
}