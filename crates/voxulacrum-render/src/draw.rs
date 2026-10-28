//! The render world's draw: render target upkeep, the render graph, the egui
//! paint and present, plus the stats the main side reads back. Runs in the
//! render world's schedule; what it reports goes back through `RenderFeedback`.

use bevy_ecs::prelude::{Res, ResMut};
use crate::resources::{
    GlobalUniformBindGroup, ReflectionUniformBindGroup, ShadowBindGroup, ShadowDepthView,
};
use crate::canopy_pass::CanopyPass;
use crate::egui_painter::{EguiFrame, EguiPainter};
use crate::cap_pass::CapPass;
use crate::capsule_pass::CapsulePass;
use crate::debug_lines::DebugLinePass;
use crate::detail_paint_pass::DetailPaintPass;
use crate::frustum::Frustum;
use crate::main_scene_pass::{CapConfig, MainScenePassNode};
use crate::outline_pass::OutlinePass;
use crate::palette_pass::PalettePass;
use crate::pipelines::{PipelineRegistry, PipelineResources};
use crate::player_pass::PlayerPass;
use crate::post_process::PostProcessPass;
use crate::reflection_pass::ReflectionPassNode;
use crate::render_context::RenderContext;
use crate::render_graph::{RenderGraph, ResourceId, ResourceMap};
use crate::render_targets::RenderTargets;
use crate::render_world::{RenderFeedback, RenderParams};
use crate::scatter_pass::ScatterPass;
use crate::shadow_pass::ShadowPassNode;
use crate::skeleton_preview_pass::SkeletonPreviewPass;
use crate::surface_state::SurfaceState;
use crate::upscale_pass::UpscalePass;
use crate::water_pass::WaterPass;
use crate::chunk_meshes::{ChunkMesh, ChunkMeshes};
use crate::frame_state::FrameState;

/// Count what the renderer holds, for the main side's stats: meshed chunks,
/// those inside the camera frustum and their triangles, and the mesh bytes on
/// the GPU. The draw count comes from the render graph.
pub fn render_stats_system(
    meshes: Res<ChunkMeshes>,
    frame: Res<FrameState>,
    mut feedback: ResMut<RenderFeedback>,
) {
    let stats = &mut feedback.stats;
    for (&pos, mesh) in &meshes.meshes {
        stats.chunks_meshed += 1;
        if frame.camera.frustum.is_chunk_visible(pos) {
            stats.chunks_visible += 1;
            stats.triangles += mesh.index_count as u64 / 3;
        }
    }
    stats.gpu_mesh_bytes = meshes.gpu_bytes();
}

// ==========================================================================
// The draw - exclusive system (takes the render world)
// ==========================================================================

pub fn render_present_system(ecs: &mut bevy_ecs::world::World) {
    // Skip rendering when the window has zero dimensions (e.g. during shutdown).
    {
        let surface = ecs.resource::<SurfaceState>();
        if surface.surface_config.width == 0 || surface.surface_config.height == 0 {
            return;
        }
    }

    // Clone is cheap — Device/Queue are Arc handles internally.
    // This avoids all borrow conflicts with resource_mut calls below.
    let ctx = ecs.resource::<RenderContext>().clone();

    // --- Render target resize check ---
    // The lattice this frame is drawn on was sized with the camera.
    let dims = ecs.resource::<FrameState>().dims;

    if ecs.resource::<RenderTargets>().needs_recreate(&dims) {
        let new_rt = RenderTargets::new(&ctx, &dims);

        let water_layout = &ecs.resource::<PipelineResources>().water_bind_group_layout.clone();
        ecs.resource_mut::<WaterPass>().rebuild_bind_group(
            &ctx.device,
            water_layout,
            &new_rt.scene_copy_view,
            &new_rt.depth_sample_view,
            &new_rt.reflection_color_view,
            &new_rt.reflection_depth_sample_view,
        );
        ecs.resource_mut::<OutlinePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
            &new_rt.depth_sample_view,
            &new_rt.normal_view,
        );
        ecs.resource_mut::<PostProcessPass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
            &new_rt.depth_sample_view,
        );
        ecs.resource_mut::<PalettePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
        );
        ecs.resource_mut::<UpscalePass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
        );

        *ecs.resource_mut::<RenderTargets>() = new_rt;
    }

    // --- Acquire swapchain ---
    // Timed separately from the rest of the Render stage: under Fifo this call
    // is where the frame blocks on vsync, which is waiting rather than work.
    let acquire_start = std::time::Instant::now();
    let surface_result = ecs.resource::<SurfaceState>().surface.get_current_texture();
    let acquire_ms = acquire_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<RenderFeedback>().present_ms += acquire_ms;
    let surface_frame = match surface_result {
        Ok(f) => f,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            // Reconfigure at the size last configured. A resize the window
            // reported since then arrives as a request on the next frame.
            let mut surface = ecs.resource_mut::<SurfaceState>();
            let (width, height) = (surface.surface_config.width, surface.surface_config.height);
            surface.resize(&ctx.device, width, height);
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => {
            log::warn!("Surface timeout");
            return;
        }
        Err(e) => {
            log::error!("Surface error: {:?}", e);
            return;
        }
    };

    let surface_view = surface_frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // --- Cap draw flag ---
    // The cap mesh is rebuilt main-side (`cross_section_cap_system`); this only
    // decides whether to draw it.
    let (cap_enabled, clip_dirs) = {
        let frame = ecs.resource::<FrameState>();
        let cs_enabled = ecs.resource::<RenderParams>().scene.cross_section;
        (cs_enabled && frame.clip_enabled == 1, frame.clip_dirs)
    };

    // --- Build and execute render graph ---
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render_encoder"),
    });

    let draws = {
        let render_params = ecs.resource::<RenderParams>();
        let pipeline_registry = ecs.resource::<PipelineRegistry>();
        let shadow_bind_group = ecs.resource::<ShadowBindGroup>();
        let shadow_depth_view = ecs.resource::<ShadowDepthView>();
        let uniform_bind_group = ecs.resource::<GlobalUniformBindGroup>();
        let render_targets = ecs.resource::<RenderTargets>();
        let detail_paint_pass = ecs.resource::<DetailPaintPass>();
        let scatter_pass = ecs.resource::<ScatterPass>();
        let capsule_pass = ecs.resource::<CapsulePass>();
        let canopy_pass = ecs.resource::<CanopyPass>();
        let skeleton_preview_pass = ecs.resource::<SkeletonPreviewPass>();
        let water_pass = ecs.resource::<WaterPass>();
        let debug_line_pass = ecs.resource::<DebugLinePass>();
        let cap_pass = ecs.resource::<CapPass>();
        let outline_pass = ecs.resource::<OutlinePass>();
        let post_process = ecs.resource::<PostProcessPass>();
        let palette_pass = ecs.resource::<PalettePass>();
        let upscale_pass = ecs.resource::<UpscalePass>();

        let terrain_pipeline = if render_params.scene.wireframe {
            &pipeline_registry.terrain_wireframe_pipeline
        } else {
            &pipeline_registry.terrain_pipeline
        };

        // Compute shadow frustum from the light-space (shadow VP) matrix.
        // Frustum::from_view_projection works for orthographic projections too.
        let frame = ecs.resource::<FrameState>();
        let shadow_frustum = Frustum::from_view_projection(frame.light_space);

        // Pre-filter meshes: one list for the camera frustum (main scene), one
        // for the shadow frustum (shadow pass).
        let meshes = ecs.resource::<ChunkMeshes>();
        let visible_chunks: Vec<&ChunkMesh> = meshes.meshes.iter()
            .filter(|(pos, _)| frame.camera.frustum.is_chunk_visible(**pos))
            .map(|(_, mesh)| mesh)
            .collect();
        let shadow_chunks: Vec<&ChunkMesh> = meshes.meshes.iter()
            .filter(|(pos, _)| shadow_frustum.is_chunk_visible(**pos))
            .map(|(_, mesh)| mesh)
            .collect();

        let shadow_node = ShadowPassNode {
            pipeline: &pipeline_registry.shadow_pipeline,
            bind_group: &shadow_bind_group.0,
            shadow_depth_view: &shadow_depth_view.0,
            chunks: &shadow_chunks,
            capsule_pass: ecs.resource::<CapsulePass>(),
            capsule_shadow_pipeline: &pipeline_registry.capsule_shadow_pipeline,
        };
        
        let player_pass = ecs.resource::<PlayerPass>();

        let room_enclosed = render_params.room_enclosed;

        let sky = if room_enclosed {
            [0.02, 0.02, 0.028] // dark void-gray while enclosed; the wireframe sits just above this
        } else {
            frame.sky_color
        };
        let scene_node = MainScenePassNode {
            sky_color: wgpu::Color {
                r: sky[0] as f64,
                g: sky[1] as f64,
                b: sky[2] as f64,
                a: 1.0,
            },
            terrain_pipeline,
            uniform_bind_group: &uniform_bind_group.0,
            chunks: &visible_chunks,
            frustum: &frame.camera.frustum,
            cap_pass: &cap_pass,
            cap_config: CapConfig {
                enabled: cap_enabled,
                clip_dirs,
                clip_pos: [0.0; 3],
            },
            detail_paint_pass: &detail_paint_pass,
            detail_paint_pipeline: &pipeline_registry.detail_paint_pipeline,
            scatter_pass: &scatter_pass,
            scatter_pipeline: &pipeline_registry.scatter_pipeline,
            capsule_pass: &capsule_pass,
            capsule_pipeline: &pipeline_registry.capsule_pipeline,
            canopy_pass: &canopy_pass,
            canopy_pipeline: &pipeline_registry.canopy_core_pipeline,
            skeleton_preview_pass: &skeleton_preview_pass,
            show_canopy_cores: render_params.scene.canopy_cores,
            debug_line_pass: &debug_line_pass,
            show_debug_lines: render_params.scene.chunk_boundaries,
            hide_foliage: render_params.scene.hide_foliage,
            player_pass: &player_pass,
        };

        let reflection_bind_group = ecs.resource::<ReflectionUniformBindGroup>();
        let reflection_node = ReflectionPassNode {
            color_view: &render_targets.reflection_color_view,
            normal_view: &render_targets.reflection_normal_view,
            depth_view: &render_targets.reflection_depth_view,
            sky_color: wgpu::Color { r: sky[0] as f64, g: sky[1] as f64, b: sky[2] as f64, a: 1.0 },
            terrain_pipeline: &pipeline_registry.terrain_reflection_pipeline,
            reflection_bind_group: &reflection_bind_group.0,
            chunks: &visible_chunks,
            frustum: &frame.camera.frustum,
            detail_paint_pass: &detail_paint_pass,
            detail_paint_pipeline: &pipeline_registry.detail_paint_pipeline,
            scatter_pass: &scatter_pass,
            scatter_pipeline: &pipeline_registry.scatter_pipeline,
            capsule_pass: &capsule_pass,
            capsule_pipeline: &pipeline_registry.capsule_pipeline,
            player_pass: &player_pass,
            hide_foliage: render_params.scene.hide_foliage,
            enabled: !render_params.scene.hide_water && !water_pass.chunk_meshes.is_empty(),
        };

        let water_node = crate::water_scene_pass::WaterScenePassNode {
            pipeline: &pipeline_registry.water_pipeline,
            global_bind_group: &uniform_bind_group.0,
            water_pass: &water_pass,
            frustum: &frame.camera.frustum,
            hide_water: render_params.scene.hide_water,
            scene_tex: &render_targets.scene,
            scene_copy_tex: &render_targets.scene_copy,
            tex_size: wgpu::Extent3d {
                width: render_targets.alloc_w,
                height: render_targets.alloc_h,
                depth_or_array_layers: 1,
            },
        };

        let mut graph = RenderGraph::new();
        graph.add_pass(&shadow_node);
        graph.add_pass(&reflection_node);
        graph.add_pass(&scene_node);
        graph.add_pass(&water_node);
        graph.add_pass(&*outline_pass);
        graph.add_pass(&*post_process);
        graph.add_pass(&*palette_pass);
        graph.add_pass(&*upscale_pass);

        #[cfg(debug_assertions)]
        graph
            .validate(&[
                ResourceId::SHADOW_DEPTH,
                ResourceId::SCENE,
                ResourceId::SCENE_COPY,
                ResourceId::NORMAL,
                ResourceId::DEPTH,
                ResourceId::PROCESSED,
                ResourceId::SURFACE,
            ])
            .unwrap();

        let mut resources = ResourceMap::new();
        resources.insert(ResourceId::SCENE, &render_targets.scene_view);
        resources.insert(ResourceId::SCENE_COPY, &render_targets.scene_copy_view);
        resources.insert(ResourceId::NORMAL, &render_targets.normal_view);
        resources.insert(ResourceId::DEPTH, &render_targets.depth_view);
        resources.insert(ResourceId::PROCESSED, &render_targets.processed_view);
        resources.insert(ResourceId::SHADOW_DEPTH, &shadow_depth_view.0);
        resources.insert(ResourceId::SURFACE, &surface_view);

        graph.execute(&mut encoder, &resources)
    };
    ecs.resource_mut::<RenderFeedback>().stats.draws = draws;

    // --- Egui paint ---
    // Laid out main-side by `egui_layout_system`; this only paints it.
    ecs.resource_scope::<EguiFrame, _>(|ecs, mut egui_frame| {
        ecs.resource_mut::<EguiPainter>().paint(
            &ctx.device,
            &ctx.queue,
            &mut encoder,
            &surface_view,
            &mut egui_frame,
        );
    });

    // --- Submit + present ---
    ctx.queue.submit(std::iter::once(encoder.finish()));
    let present_start = std::time::Instant::now();
    surface_frame.present();
    let present_ms = present_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<RenderFeedback>().present_ms += present_ms;
}
