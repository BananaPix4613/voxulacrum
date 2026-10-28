//! What the render world reads about each frame beyond the render settings:
//! the camera, the lattice, lighting, wind, the clip bounds and the mask.

use bevy_ecs::prelude::Resource;
use glam::Mat4;

use crate::rendering::camera_desc::CameraDesc;
use crate::rendering::render_dims::RenderDims;

/// All computed per-frame data that rendering needs. `SimulationManager` fills
/// it on the main side, and Extract copies it into the render world.
#[derive(Resource, Clone)]
pub struct FrameState {
    pub dt: f32,
    pub elapsed: f32,
    /// This frame's camera description.
    pub camera: CameraDesc,
    /// The pixel lattice this frame is drawn on, sized from the window and the
    /// camera's zoom. Beside the camera rather than in its block, because the
    /// render targets are sized from it; V8 decides who owns it.
    pub dims: RenderDims,
    pub light_space: Mat4,
    pub sun_direction: [f32; 3],
    pub sun_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub wind_vector: [f32; 2],
    pub clip_min: [f32; 3],
    pub clip_max: [f32; 3],
    pub clip_enabled: u32,
    pub mask_origin: [f32; 3],
    pub mask_enabled: u32,
    pub volume_radius: f32,
    pub debug_mode: u32,
    /// The scene's clear color, from the panel's sky color.
    pub sky_color: [f32; 3],
    pub warm_tint_color: [f32; 3],
    pub warm_tint_strength: f32,
    /// Which way the cross-section cap faces on each axis (+1 or -1), from the
    /// camera's settled rotation.
    pub clip_dirs: [f32; 3],
}
