//! The render world (design §12, "Render world and extract"): a second ECS world
//! that owns every GPU resource, with a schedule of its own. The main world
//! holds it as a resource; `extract_system` hands it each frame's data, and
//! `render_world_system` runs it and reads back what it reports.

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{ExecutorKind, IntoScheduleConfigs, Schedule};

use crate::{draw, prepare};
use crate::settings::{RenderSettings, SceneOptions};
use crate::palette_pass::PaletteUniforms;
use crate::uniform_writer::CloudFrame;

#[derive(Resource)]
pub struct RenderWorld {
    pub world: World,
    schedule: Schedule,
}

impl RenderWorld {
    pub fn new(world: World, schedule: Schedule) -> Self {
        Self { world, schedule }
    }

    /// Draw one frame: apply its requests and uploads, write the uniforms, run
    /// the render graph and egui, and present.
    pub fn run(&mut self) {
        self.schedule.run(&mut self.world);
    }
}

/// Build the render world's schedule: apply the frame's requests and uploads,
/// count stats, write the uniforms, then draw and present. Single-threaded and
/// chained: the work is one sequence ending in an exclusive draw.
pub fn build_render_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set_executor_kind(ExecutorKind::SingleThreaded);
    schedule.add_systems(
        (
            prepare::surface_resize_system,
            prepare::shader_reload_system,
            prepare::chunk_upload_system,
            draw::render_stats_system,
            prepare::write_uniforms_system,
            draw::render_present_system,
        )
            .chain(),
    );
    schedule
}

/// What the main side's settings and state say about this frame's render,
/// beyond `FrameState`. Copied at Extract.
#[derive(Resource)]
pub struct RenderParams {
    /// The render settings groups as the panel has them this frame.
    pub settings: RenderSettings,
    pub scene: SceneOptions,
    pub cloud: CloudFrame,
    /// The palette pass's uniforms, built main-side from the loaded palette.
    pub palette: PaletteUniforms,
    /// Whether the player is enclosed underground, which darkens the sky.
    pub room_enclosed: bool,
}

/// What the renderer drew and holds. The main side reads it one frame behind.
#[derive(Resource, Clone, Copy, Default)]
pub struct RenderStats {
    pub draws: u32,
    pub chunks_meshed: u32,
    /// Meshed chunks inside the camera frustum.
    pub chunks_visible: u32,
    /// Triangles of the visible chunks.
    pub triangles: u64,
    pub gpu_mesh_bytes: u64,
}

/// What one frame's render reports back. Taken after every run.
#[derive(Resource, Default)]
pub struct RenderFeedback {
    pub stats: RenderStats,
    /// Swapchain acquire and present, in milliseconds: waiting, not work.
    pub present_ms: f32,
    /// Shader reload results as (message, is_error), oldest first.
    pub shader_log: Vec<(String, bool)>,
}

impl RenderFeedback {
    pub fn push_shader_log(&mut self, message: String, is_error: bool) {
        self.shader_log.push((message, is_error));
    }
}
