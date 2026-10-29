//! The bridge between the main world and the render world (design §12, "Render
//! world and extract"). `extract_system` ends the Extract stage by handing the
//! render world this frame's data; `render_world_system` is the Render stage: it
//! runs the render world's schedule and reads back what the render reported.

use bevy_ecs::prelude::*;

use crate::diagnostics::FrameTimings;
use crate::ecs::resources::{LoadedPalette, VisibilityMaskTexels};
use crate::meshing::cap::CapMesh;
use crate::meshing::overlay::{DebugLines, SkeletonPreview};
use crate::meshing::player::PlayerMesh;
use crate::rendering::resources::{RenderRequests, VisibilityMask};
use crate::rendering::uploads::ChunkUploads;
use crate::params::EngineParams;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::cloud_textures::CloudTextures;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::egui_painter::EguiFrame;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_world::{RenderFeedback, RenderParams, RenderStats, RenderWorld};
use crate::rendering::settings::SceneOptions;
use crate::rendering::skeleton_preview_pass::SkeletonPreviewPass;
use crate::rendering::frame_state::FrameState;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::room::RoomState;

/// Hand the render world this frame's data: the queued per-chunk uploads,
/// render requests and egui output move across, the versioned data uploads
/// where its version moved, and the frame state and render parameters are
/// copied. Runs last in Extract, after everything that builds that data.
pub fn extract_system(main: &mut World) {
    main.resource_scope::<RenderWorld, _>(|main, mut render_world| {
        let render = &mut render_world.world;
        move_queues(main, render);
        upload_versioned(main, render);
        copy_frame(main, render);
    });
}

/// Move what the main side queued since the last frame into the render world,
/// merged with anything the render world has not applied yet.
fn move_queues(main: &mut World, render: &mut World) {
    // The render world drains its copy every frame, so the swap hands the main
    // side back an empty queue to fill.
    std::mem::swap(
        &mut *main.resource_mut::<ChunkUploads>(),
        &mut *render.resource_mut::<ChunkUploads>(),
    );

    let requests = std::mem::take(&mut *main.resource_mut::<RenderRequests>());
    let mut pending = render.resource_mut::<RenderRequests>();
    if requests.surface_size.is_some() {
        pending.surface_size = requests.surface_size;
    }
    pending.shader_changes.extend(requests.shader_changes);

    let frame = std::mem::take(&mut *main.resource_mut::<EguiFrame>());
    let mut pending = render.resource_mut::<EguiFrame>();
    pending.primitives = frame.primitives;
    pending.size_in_pixels = frame.size_in_pixels;
    pending.pixels_per_point = frame.pixels_per_point;
    // Texture changes the painter has not applied yet stay ahead of this frame's.
    pending.textures.append(frame.textures);
}

/// Upload the main side's versioned data, each only where its version moved.
fn upload_versioned(main: &mut World, render: &mut World) {
    let ctx = render.resource::<RenderContext>().clone();

    let lines = main.resource::<DebugLines>();
    render.resource_mut::<DebugLinePass>().upload(
        &ctx.device,
        &lines.boundaries,
        &lines.highlight,
        &lines.selection,
        &lines.overlay,
    );
    render
        .resource_mut::<SkeletonPreviewPass>()
        .upload(&ctx.device, &main.resource::<SkeletonPreview>().0);
    render
        .resource_mut::<PlayerPass>()
        .upload(&ctx.queue, &main.resource::<PlayerMesh>().0);
    render
        .resource_mut::<CapPass>()
        .upload(&ctx.device, &main.resource::<CapMesh>().mesh);
    render
        .resource_mut::<VisibilityMask>()
        .upload(&ctx.queue, &main.resource::<VisibilityMaskTexels>().0);

    let cloud = &main.resource::<SimulationManager>().cloud_shadow;
    render.resource_mut::<CloudTextures>().upload(
        &ctx.queue,
        &cloud.detail_texels,
        &cloud.envelope_texels,
    );
}

/// Copy this frame's state and render parameters into the render world.
fn copy_frame(main: &World, render: &mut World) {
    let ui = main.resource::<UiState>();
    let palette = crate::palette::frame_uniforms(
        main.resource::<LoadedPalette>().0.as_ref(),
        &ui.params.palette,
    );
    let cloud_shadow = &main.resource::<SimulationManager>().cloud_shadow;
    render.insert_resource(main.resource::<FrameState>().clone());
    render.insert_resource(RenderParams {
        settings: ui.params.render.clone(),
        scene: scene_options(&ui.params),
        cloud: cloud_shadow.frame(&ui.params.cloud),
        palette,
        room_enclosed: main.resource::<RoomState>().enclosed,
    });
}

/// The values the renderer reads from settings groups the main side shares with
/// other systems.
fn scene_options(params: &EngineParams) -> SceneOptions {
    let meshing = &params.meshing;
    let cross_section = &params.cross_section;
    let debug = &params.debug;
    SceneOptions {
        edge_strength: meshing.edge_strength,
        ao_strength: if meshing.ortho_ao_enabled { meshing.ortho_ao_strength } else { 0.0 },
        cross_section: cross_section.enabled,
        clip_edges: cross_section.show_edges,
        clip_fog_color: cross_section.fog_color,
        clip_fog_density: cross_section.fog_density,
        wireframe: debug.show_wireframe,
        chunk_boundaries: debug.show_chunk_boundaries,
        canopy_cores: debug.show_canopy_cores,
        hide_foliage: debug.hide_foliage,
        hide_water: debug.hide_water,
    }
}

/// Run the render world's schedule for this frame, then copy back what it
/// reported: the stats, the present time and the shader log.
pub fn render_world_system(main: &mut World) {
    main.resource_scope::<RenderWorld, _>(|main, mut render_world| {
        render_world.run();
        let feedback = std::mem::take(&mut *render_world.world.resource_mut::<RenderFeedback>());
        main.resource_mut::<FrameTimings>().add_present(feedback.present_ms);
        *main.resource_mut::<RenderStats>() = feedback.stats;
        let mut ui = main.resource_mut::<UiState>();
        for (message, is_error) in feedback.shader_log {
            ui.push_shader_log(message, is_error);
        }
    });
}
