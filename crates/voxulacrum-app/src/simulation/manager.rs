use std::time::Instant;
use glam::Mat4;
use bevy_ecs::prelude::Resource;

use crate::camera::{self, IsometricCamera, SnappedCamera};
use crate::input::InputState;
use crate::cloud_shadow::CloudShadowState;
use crate::params::EngineParams;
use crate::rendering::frustum::Frustum;
use crate::rendering::render_targets::RenderTargets;
use crate::simulation::time_of_day::TimeOfDay;
use crate::simulation::wind::WindState;
use crate::world::chunk::VOXEL_SCALE;

/// All computed per-frame data that rendering needs.
#[derive(Resource)]
pub struct FrameState {
    pub dt: f32,
    pub elapsed: f32,
    pub view_proj: [[f32; 4]; 4],
    pub subpixel_offset: [f32; 2],
    pub light_space: Mat4,
    pub sun_direction: [f32; 3],
    pub sun_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub wind_vector: [f32; 2],
    pub cloud_shadow_offset: [f32; 2],
    pub cloud_coverage: f32,
    pub clip_min: [f32; 3],
    pub clip_max: [f32; 3],
    pub clip_enabled: u32,
    pub debug_mode: u32,
    pub sky_color: [f32; 3],
    pub warm_tint_color: [f32; 3],
    pub warm_tint_strength: f32,
    pub cos_r: f32,
    pub sin_r: f32,
}

#[derive(Resource)]
pub struct SimulationManager {
    pub camera: IsometricCamera,
    pub time_of_day: TimeOfDay,
    pub wind: WindState,
    pub cloud_shadow: CloudShadowState,
    pub frustum: Frustum,
    pub elapsed: f32,
    last_frame: Instant,
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
            elapsed: 0.0,
            last_frame: Instant::now(),
        }
    }

    /// Advance all simulation systems by one frame.
    pub fn tick(&mut self, params: &EngineParams, rt: &RenderTargets, input: &InputState) -> FrameState {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.elapsed += dt;

        // Camera
        self.camera.apply_params(&params.camera);
        self.camera.update(dt, input, &params.camera);

        let snapped = if params.render_pipeline.camera_snap_enabled {
            self.camera.snap_camera(
                rt.render_width,
                rt.render_height,
                rt.effective_pixel_scale,
            )
        } else {
            SnappedCamera {
                view_proj: self.camera.view_projection(),
                subpixel_offset: [0.0; 2],
            }
        };

        // Time, wind, clouds
        self.time_of_day.update(dt, &params.time_control);
        self.wind.update(dt, &params.wind);
        self.cloud_shadow.update(
            dt,
            &self.wind.wind_vector,
            self.elapsed,
            &params.cloud,
        );

        // Frustum
        if !params.debug.freeze_culling {
            let vp = self.camera.projection_matrix() * self.camera.view_matrix();
            self.frustum = Frustum::from_view_projection(vp);
        }

        // Cross-section clip bounds
        let cos_r = self.camera.target_rotation.cos();
        let sin_r = self.camera.target_rotation.sin();
        let (clip_min, clip_max, clip_enabled) = compute_clip_bounds(
            &self.camera, params, cos_r, sin_r,
        );

        // Debug mode
        let debug_mode = compute_debug_mode(&params.debug);

        // Warm tint
        let (warm_tint_color, warm_tint_strength) = self.time_of_day.warm_tint();

        FrameState {
            dt,
            elapsed: self.elapsed,
            view_proj: snapped.view_proj,
            subpixel_offset: snapped.subpixel_offset,
            light_space: self.time_of_day.light_space_matrix(self.camera.smooth_target, 128.0),
            sun_direction: self.time_of_day.sun_direction().into(),
            sun_color: self.time_of_day.sun_color(&params.lighting).into(),
            ambient_color: self.time_of_day.ambient_color(&params.lighting).into(),
            wind_vector: self.wind.wind_vector.into(),
            cloud_shadow_offset: self.cloud_shadow.offset.into(),
            cloud_coverage: self.cloud_shadow.coverage,
            clip_min,
            clip_max,
            clip_enabled,
            debug_mode,
            sky_color: params.render_pipeline.sky_color,
            warm_tint_color: warm_tint_color.into(),
            warm_tint_strength,
            cos_r,
            sin_r,
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