//! The render stage: render target upkeep, the render graph, egui, and present.

use glam::IVec3;
use crate::app::compute_render_dimensions;
use crate::diagnostics::FrameTimings;
use crate::ecs::resources::{
    GlobalUniformBindGroup, ReflectionUniformBindGroup, ShadowBindGroup, ShadowDepthView,
    VoxelWorld, WindowHandle,
};
use crate::materials::MaterialRegistryRes;
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::frustum::Frustum;
use crate::rendering::main_scene_pass::{CapConfig, MainScenePassNode};
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::reflection_pass::ReflectionPassNode;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_graph::{RenderGraph, ResourceId, ResourceMap};
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::shadow_pass::ShadowPassNode;
use crate::rendering::skeleton_preview_pass::SkeletonPreviewPass;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui;
use crate::ui::panels::UiState;
use crate::world::chunk::LoadedChunk;

// ==========================================================================
// Render stage — exclusive system (takes &mut World)
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
    let dims = {
        let surface = ecs.resource::<SurfaceState>();
        let sim = ecs.resource::<SimulationManager>();
        let ui = ecs.resource::<UiState>();
        compute_render_dimensions(
            surface.surface_config.width,
            surface.surface_config.height,
            sim.camera.zoom,
            ui.params.render_pipeline.world_pixel_density,
        )
    };

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
    // Zoom moves view/k/s continuously without reallocating - keep fields fresh.
    ecs.resource_mut::<RenderTargets>().update_view(&dims);

    // --- Acquire swapchain ---
    // Timed separately from the rest of the Render stage: under Fifo this call
    // is where the frame blocks on vsync, which is waiting rather than work.
    let acquire_start = std::time::Instant::now();
    let surface_result = ecs.resource::<SurfaceState>().surface.get_current_texture();
    let acquire_ms = acquire_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<FrameTimings>().add_present(acquire_ms);
    let surface_frame = match surface_result {
        Ok(f) => f,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            let size = ecs.resource::<WindowHandle>().0.inner_size();
            ecs.resource_mut::<SurfaceState>().resize(&ctx.device, size.width, size.height);
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

    // --- Debug lines update ---
    let show_debug = ecs.resource::<UiState>().params.debug.show_chunk_boundaries;
    if show_debug {
        let chunk_positions: Vec<IVec3> = ecs
            .resource::<VoxelWorld>()
            .0
            .chunks
            .keys()
            .map(|c| IVec3::from(*c))
            .collect();
        ecs.resource_scope::<DebugLinePass, _>(|ecs, mut debug| {
            let sim = ecs.resource::<SimulationManager>();
            debug.update(&ctx, &chunk_positions, &sim.frustum);
        });
    }

    // --- Cap mesh update ---
    let (cap_enabled, cos_r, sin_r, clip_max, clip_min) = {
        let frame = ecs.resource::<FrameState>();
        let cs_enabled = ecs.resource::<UiState>().params.cross_section.enabled;
        (
            cs_enabled && frame.clip_enabled == 1,
            frame.cos_r,
            frame.sin_r,
            frame.clip_max,
            frame.clip_min,
        )
    };

    if cap_enabled {
        let clip_dirs: [f32; 3] = [
            if cos_r >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            if sin_r >= 0.0 { 1.0 } else { -1.0 },
        ];
        let clip_pos = [
            if clip_dirs[0] > 0.0 { clip_max[0] } else { clip_min[0] },
            clip_max[1],
            if clip_dirs[2] > 0.0 { clip_max[2] } else { clip_min[2] },
        ];
        ecs.resource_scope::<CapPass, _>(|ecs, mut cap| {
            let world = ecs.resource::<VoxelWorld>();
            let ui = ecs.resource::<UiState>();
            cap.maybe_rebuild(&ctx, &world.0, &ui.params.cross_section, clip_pos, clip_dirs);
        });
    }

    // --- Build and execute render graph ---
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render_encoder"),
    });

    {
        let ui = ecs.resource::<UiState>();
        let world = ecs.resource::<VoxelWorld>();
        let sim = ecs.resource::<SimulationManager>();
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

        let terrain_pipeline = if ui.params.debug.show_wireframe {
            &pipeline_registry.terrain_wireframe_pipeline
        } else {
            &pipeline_registry.terrain_pipeline
        };

        // Compute shadow frustum from the light-space (shadow VP) matrix.
        // Frustum::from_view_projection works for orthographic projections too.
        let frame = ecs.resource::<FrameState>();
        let shadow_frustum = Frustum::from_view_projection(frame.light_space);

        // Pre-filter chunks: one list for the camera frustum (main scene),
        // one for the shadow frustum (shadow pass).
        let visible_chunks: Vec<&LoadedChunk> = world.0.chunks.values()
            .filter(|c| c.mesh.is_some() && sim.frustum.is_chunk_visible(c.data.coord.into()))
            .collect();
        let shadow_chunks: Vec<&LoadedChunk> = world.0.chunks.values()
            .filter(|c| c.mesh.is_some() && shadow_frustum.is_chunk_visible(c.data.coord.into()))
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

        let room_enclosed = ecs.resource::<crate::world::room::RoomState>().enclosed;

        let sky = if room_enclosed {
            [0.02, 0.02, 0.028] // dark void-gray while enclosed; the wireframe sits just above this
        } else {
            ui.params.render_pipeline.sky_color
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
            frustum: &sim.frustum,
            cap_pass: &cap_pass,
            cap_config: CapConfig {
                enabled: cap_enabled,
                clip_dirs: [
                    if cos_r >= 0.0 { 1.0 } else { -1.0 },
                    1.0,
                    if sin_r >= 0.0 { 1.0 } else { -1.0 },
                ],
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
            show_canopy_cores: ui.params.debug.show_canopy_cores,
            debug_line_pass: &debug_line_pass,
            show_debug_lines: ui.params.debug.show_chunk_boundaries,
            hide_foliage: ui.params.debug.hide_foliage,
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
            frustum: &sim.frustum,
            detail_paint_pass: &detail_paint_pass,
            detail_paint_pipeline: &pipeline_registry.detail_paint_pipeline,
            scatter_pass: &scatter_pass,
            scatter_pipeline: &pipeline_registry.scatter_pipeline,
            capsule_pass: &capsule_pass,
            capsule_pipeline: &pipeline_registry.capsule_pipeline,
            player_pass: &player_pass,
            hide_foliage: ui.params.debug.hide_foliage,
            enabled: !ui.params.debug.hide_water && !water_pass.chunk_meshes.is_empty(),
        };

        let water_node = crate::rendering::water_scene_pass::WaterScenePassNode {
            pipeline: &pipeline_registry.water_pipeline,
            global_bind_group: &uniform_bind_group.0,
            water_pass: &water_pass,
            frustum: &sim.frustum,
            hide_water: ui.params.debug.hide_water,
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

        graph.execute(&mut encoder, &resources);
    }

    // --- Egui draw ---
    let window_arc = ecs.resource::<WindowHandle>().0.clone();
    let registry = ecs.resource::<MaterialRegistryRes>().0.clone();
    ecs.resource_scope::<UiState, _>(|ecs, mut ui_state| {
        ecs.resource_scope::<ui::field_probe::FieldProbe, _>(|ecs, mut probe| {
            // The build tool's selected material, when the package shows it in Play.
            let in_play = ecs.resource::<VoxelWorld>().0.mode == crate::world::mutation::EngineMode::Play;
            let selected = ecs.resource::<crate::interaction::PlayerToolState>().selected;
            let play_hud: Option<(String, [f32; 3])> = ecs
                .get_resource::<crate::play::PlaySettings>()
                .filter(|play| in_play && play.hud)
                .and_then(|play| play.build.as_ref())
                .and_then(|build| build.materials.get(selected))
                .and_then(|material| material.id())
                .and_then(|id| registry.get(id))
                .map(|def| (def.display_name.clone(), def.color));

            let mut egui = ecs.non_send_resource_mut::<ui::EguiRenderer>();
            egui.draw(
                &mut *ui_state,
                &mut *probe,
                &registry,
                &ctx,
                &mut encoder,
                &surface_view,
                &window_arc,
                play_hud.as_ref().map(|(name, color)| (name.as_str(), *color)),
            );
        });
    });

    // --- Submit + present ---
    ctx.queue.submit(std::iter::once(encoder.finish()));
    let present_start = std::time::Instant::now();
    surface_frame.present();
    let present_ms = present_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<FrameTimings>().add_present(present_ms);
}
