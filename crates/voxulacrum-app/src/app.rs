use std::path::PathBuf;
use std::sync::Arc;
use bevy_ecs::prelude::*;
use bevy_ecs::event::Events;
use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::{
    bench, core_budget, diagnostics, graph_reload, interaction, jobs, libraries, materials,
    paths, prefabs, rendering, simulation, ui, validate, verify, world, determinism,
};
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::render_context::RenderContext;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::uniforms::{self, GlobalUniforms, ShadowUniforms};
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::water_pass::WaterPass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::cap_pass::CapPass;
use crate::shader_reload::ShaderWatcher;
use crate::simulation::manager::SimulationManager;
use crate::meshing::coordinator::{ChunkMeshes, MeshingCoordinator};
use crate::meshing::MeshingPipeline;
use crate::world::regen::WorldRegenCoordinator;
use crate::world::persistence::WorldPersistence;
use crate::world::config::ServerConfig;
use crate::camera::IsometricCamera;
use crate::cloud_shadow::{CloudShadowState, DETAIL_TEX_SIZE, ENV_SIZE};
use crate::rendering::cloud_textures::CloudTextures;
use crate::params::EngineParams;
use crate::ui::panels::UiState;
use crate::materials::MaterialRegistryRes;

use crate::ecs::resources::*;
use crate::ecs::events::*;
use crate::ecs::clocks::{PresentationClock, TickDriver};
use crate::ecs::schedule::{build_frame_schedule, build_tick_schedule};
use crate::input::{InputMap, InputState, PointerButton, PointerState, RawInputBuffer, RawInputEvent};
use crate::world::room::RoomState;

pub const SHADOW_MAP_SIZE: u32 = 8192;

// ---------------------------------------------------------------------------
// FrameCounter
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct FrameCounter {
    last_second: std::time::Instant,
    frames_this_second: u32,
    pub fps: f32,
}

impl FrameCounter {
    fn new() -> Self {
        Self {
            last_second: std::time::Instant::now(),
            frames_this_second: 0,
            fps: 0.0,
        }
    }

    pub fn tick(&mut self) {
        self.frames_this_second += 1;
        if self.last_second.elapsed().as_secs_f32() >= 1.0 {
            self.fps = self.frames_this_second as f32;
            self.frames_this_second = 0;
            self.last_second = std::time::Instant::now();
        }
    }
}

// ---------------------------------------------------------------------------
// compute_render_dimensions
// ---------------------------------------------------------------------------

/// The pixel-art lattice for one frame. `k` fixes the world->texel mapping
/// (integer texels per voxel - the image is always painted at this density);
/// `s` is how big one texel appears on screen (continuous, non-integer fine).
/// Zoom only ever changes `s` (and, in octaves, `k`) - never the lattice phase.
#[derive(Clone, Copy)]
pub struct RenderDims {
    /// Texels per voxel — integer, stepped in octaves.
    pub k: u32,
    /// Native pixels per texel — continuous screen magnification.
    pub s: f32,
    /// Visible content in texels (= surface / s, fractional).
    pub view_w: f32,
    pub view_h: f32,
    /// Texture allocation in texels (bucketed so zoom doesn't reallocate).
    pub alloc_w: u32,
    pub alloc_h: u32,
}

pub fn compute_render_dimensions(
    surface_w: u32,
    surface_h: u32,
    zoom: f32,
    world_pixel_density: f32
) -> RenderDims {
    let base_k = world_pixel_density.round().max(1.0) as u32;
    let mut k = base_k;
    let mut s = surface_h as f32 / (2.0 * zoom * k as f32);
    // Octave ladder: partial minify (s < 1 downsamples the texel image).
    while s < 0.85 && k > 1 {
        k /= 2;
        s *= 2.0;
    }
    let view_w = surface_w as f32 / s;
    let view_h = surface_h as f32 / s;
    // Bucket allocation to 64-texel steps (+2 guard) so continuous zoom only
    // reallocates at bucket boundaries, not every tick.
    let alloc_w = (view_w.ceil() as u32 + 2).div_ceil(64) * 64;
    let alloc_h = (view_h.ceil() as u32 + 2).div_ceil(64) * 64;
    RenderDims { k, s, view_w, view_h, alloc_w, alloc_h }
}

// ---------------------------------------------------------------------------
// ECS initialization
// ---------------------------------------------------------------------------

fn init_ecs(
    window: Arc<Window>,
    play: Option<crate::play::PlaySettings>,
    gpu: wgpu::PowerPreference,
    bench_setup: Option<bench::BenchSetup>,
) -> (World, Schedule) {
    let mut ecs = World::new();

    // --- wgpu initialization ---
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let surface = instance
        .create_surface(window.clone())
        .expect("Failed to create surface");
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: gpu,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .expect("No suitable GPU adapter found");
    // Named in the log and the Performance panel, so a reading always says
    // which GPU it came from.
    let info = adapter.get_info();
    let gpu_label = format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);
    log::info!("GPU: {gpu_label}, from a {gpu:?} request");

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("voxulacrum_device"),
            required_features: wgpu::Features::POLYGON_MODE_LINE,
            required_limits: wgpu::Limits::default(),
            memory_hints: Default::default(),
            trace: wgpu::Trace::Off,
        },
    ))
    .expect("Failed to request device");

    let size = window.inner_size();
    let surface_caps = surface.get_capabilities(&adapter);
    let surface_format = surface_caps
        .formats
        .iter()
        .find(|f| f.is_srgb())
        .copied()
        .unwrap_or(surface_caps.formats[0]);

    let surface_config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: surface_format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: surface_caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &surface_config);

    let ctx = RenderContext {
        device,
        queue,
        surface_format,
    };

    let surface_state = SurfaceState {
        surface,
        surface_config,
    };

    // Params. The presets directory is anchored like the other asset paths, not
    // to the working directory.
    let presets_dir = paths::presets_dir();
    let initial_params = if bench_setup.is_some() {
        bench::params()
    } else {
        EngineParams::load(&presets_dir.join("default.json")).unwrap_or_default()
    };

    let mut camera = IsometricCamera::new(&initial_params.camera);
    camera.resize(surface_state.surface_config.width, surface_state.surface_config.height);

    // Render targets
    let dims = compute_render_dimensions(
        surface_state.surface_config.width,
        surface_state.surface_config.height,
        camera.zoom,
        initial_params.render_pipeline.world_pixel_density,
    );
    let render_targets = RenderTargets::new(&ctx, &dims);

    // Shadow map
    let shadow_depth_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow_depth_texture"),
        size: wgpu::Extent3d {
            width: SHADOW_MAP_SIZE,
            height: SHADOW_MAP_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_depth_view =
        shadow_depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let shadow_sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow_comparison_sampler"),
        compare: Some(wgpu::CompareFunction::LessEqual),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let global_bind_group_layout = uniforms::create_bind_group_layout(&ctx.device);

    let uniform_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("global_uniform_buffer"),
        size: size_of::<GlobalUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    
    // Data-driven material registry: `packages/sandbox/materials.ron` is the only source
    // of material identity and color. Loaded here because the color table below
    // is built from it; shared with the foliage passes and any system needing
    // material metadata.
    let material_registry = materials::load_registry();
    
    // Material color table (group 0, binding 5), written once from the registry.
    let material_color_data = uniforms::material_color_table(&material_registry);
    let material_color_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("material_color_buffer"),
        contents: bytemuck::cast_slice(&material_color_data),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    // The cloud state bakes Layer B's texels; the textures start empty and take
    // them on the first frame's upload.
    let cloud_shadow = CloudShadowState::new();
    let cloud_textures = CloudTextures::new(&ctx.device, DETAIL_TEX_SIZE, ENV_SIZE as u32);

    // Visibility mask (group 0, binding 6): a 64^3 window of one-byte cell states
    // centered on the player, rewritten on movement from VisibilityMaskTexels.
    let visibility_mask_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("visibility_mask"),
        size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 64 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let visibility_mask_view =
        visibility_mask_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let uniform_bind_group = uniforms::create_bind_group(
        &ctx.device,
        &global_bind_group_layout,
        &uniform_buffer,
        &cloud_textures.detail_view,
        &cloud_textures.detail_sampler,
        &cloud_textures.envelope_view,
        &cloud_textures.envelope_sampler,
        &shadow_depth_view,
        &shadow_sampler,
        &material_color_buffer,
        &visibility_mask_view,
    );

    let reflection_uniform_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("reflection_uniform_buffer"),
        size: size_of::<GlobalUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let reflection_bind_group = uniforms::create_bind_group(
        &ctx.device,
        &global_bind_group_layout,
        &reflection_uniform_buffer,
        &cloud_textures.detail_view,
        &cloud_textures.detail_sampler,
        &cloud_textures.envelope_view,
        &cloud_textures.envelope_sampler,
        &shadow_depth_view,
        &shadow_sampler,
        &material_color_buffer,
        &visibility_mask_view,
    );

    let simulation = SimulationManager::new(camera, cloud_shadow);

    let shadow_bind_group_layout =
        uniforms::create_shadow_bind_group_layout(&ctx.device);
    let shadow_uniform_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shadow_uniform_buffer"),
        size: size_of::<ShadowUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let shadow_bind_group = uniforms::create_shadow_bind_group(
        &ctx.device, &shadow_bind_group_layout, &shadow_uniform_buffer,
    );

    // Shaders
    let shader_dir = paths::asset_root().join("shaders");
    let shader_watcher = ShaderWatcher::new(&shader_dir);
    
    // Graph hot-reload: watch the directory containing the active world graph.
    let graph_dir = world::world_generator::default_graph_path()
        .parent()
        .expect("default graph path has a parent directory")
        .to_path_buf();
    let graph_watcher = graph_reload::GraphWatcherRes::new(&graph_dir);
    
    let post_process_bind_group_layout =
        uniforms::create_post_process_bind_group_layout(&ctx.device);

    let water_bind_group_layout = uniforms::create_water_bind_group_layout(&ctx.device);

    let pipeline_resources = PipelineResources {
        surface_format: ctx.surface_format,
        global_bind_group_layout,
        water_bind_group_layout,
        shadow_bind_group_layout,
        post_process_bind_group_layout,
    };

    let pipeline_registry = PipelineRegistry::new(
        &ctx, &pipeline_resources, &shader_dir,
    );

    let debug_lines_source =
        std::fs::read_to_string(shader_dir.join("debug_lines.wgsl"))
            .expect("Failed to read debug_lines.wgsl");
    let debug_line_pass = DebugLinePass::new(
        &ctx,
        &pipeline_resources.global_bind_group_layout,
        &debug_lines_source,
    );

    let cap_source = std::fs::read_to_string(shader_dir.join("cap.wgsl"))
        .expect("Failed to read cap.wgsl");
    let cap_pass = CapPass::new(
        &ctx,
        &pipeline_resources.global_bind_group_layout,
        &cap_source,
    );
    
    let player_source = std::fs::read_to_string(shader_dir.join("player.wgsl"))
        .expect("Failed to read player.wgsl");
    let player_pass = rendering::player_pass::PlayerPass::new(
        &ctx,
        &pipeline_resources.global_bind_group_layout,
        &player_source,
    );

    // World
    let gen_start = std::time::Instant::now();
    
    // The single graph-backed generator, shared (via Arc) by the World, the
    // streaming workers, and background regeneration.
    let generator = world::world_generator::load_default()
        .expect("Failed to load default_biome graph generator");
    
    // The manifest owns the world's height. Reading it off the generator keeps
    // streaming and generation on one number rather than two that agree by
    // habit.
    let y_range = generator.chunk_y_range();
    let (min_y, max_y) = (y_range.start, y_range.end);
    
    let prefab_registry = prefabs::load_prefab_registry();
    let libraries = libraries::load_libraries();
    
    // One bounded rayon pool shared by the startup fill, streaming generation,
    // and background regeneration - the single worker-pool model for all chunk
    // generation (engine-design.md §12).
    let gen_pool: Arc<rayon::ThreadPool> = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(core_budget::CoreBudget::detect().pool_threads)
            .thread_name(|i| format!("chunk-gen-{i}"))
            .build()
            .expect("failed to build chunk generation thread pool"),
    );

    // Persistence opens *before* the world is built, for two reasons: the
    // startup fill needs the DB to overlay persisted player edits (without it,
    // spawn-area edits were discarded and then overwritten on the next save),
    // and `open` performs the VOXEL_FORMAT_VERSION save wipe, which must happen
    // before anything reads a record. The save name follows the content root.
    // A benchmark keeps its world apart from the package's saves.
    let save_name = if bench_setup.is_some() { bench::SAVE_NAME } else { paths::save_name() };
    let persistence = match WorldPersistence::open(save_name, generator.world_seed()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Persistence init failed: {e}. Running without saves.");
            WorldPersistence::disabled()
        }
    };

    // World cache disabled: it stored only voxels (no foliage) and went stale as
    // the generation pipeline grew (graphs, slabs, foliage), so cached chunks
    // loaded without grass/scatter. Always regenerate - the generate path
    // produces slabs + foliage identical to streamed reloads.
    let world = world::World::generate(
        generator.clone(), &gen_pool, min_y, max_y, &persistence, material_registry.clone(),
    );
    log::info!("World generated in {:.2?}", gen_start.elapsed());
    world.print_debug_stats(&material_registry);

    // Tier-1 detail paint + Tier-2/3 scatter + water. The passes start empty:
    // each chunk's entries are built as its mesh lands, startup chunks included.
    let foliage_registry = crate::foliage::load_foliage_registry();
    let detail_paint_pass = DetailPaintPass::new(&ctx, &foliage_registry.species_color_table());
    let scatter_pass = ScatterPass::new(
        &ctx,
        &crate::meshing::foliage::prefab_meshes(&prefab_registry),
    );
    let capsule_pass = CapsulePass::default();
    let canopy_pass = CanopyPass::default();
    let water_pass = WaterPass::new(
        &ctx.device,
        &pipeline_resources.water_bind_group_layout,
        &render_targets.scene_copy_view,
        &render_targets.depth_sample_view,
        &render_targets.reflection_color_view,
        &render_targets.reflection_depth_sample_view,
    );

    // Post-process pass
    let pp_source = std::fs::read_to_string(shader_dir.join("post_process.wgsl"))
        .expect("Failed to read post_process.wgsl");
    let post_process = PostProcessPass::new(
        &ctx,
        &render_targets.processed_view,
        &render_targets.depth_sample_view,
        &pp_source,
    );

    let outline_source =
        std::fs::read_to_string(shader_dir.join("outline.wgsl"))
            .expect("Failed to read outline.wgsl");
    let outline_pass = OutlinePass::new(
        &ctx,
        &render_targets.scene_view,
        &render_targets.depth_sample_view,
        &render_targets.normal_view,
        &outline_source,
    );

    let palette_source =
        std::fs::read_to_string(shader_dir.join("palette.wgsl"))
            .expect("Failed to read palette.wgsl");
    let palette_pass = PalettePass::new(
        &ctx,
        &render_targets.scene_view,
        &palette_source,
    );

    let upscale_source =
        std::fs::read_to_string(shader_dir.join("upscale.wgsl"))
            .expect("Failed to read upscale.wgsl");
    let upscale_pass = UpscalePass::new(
        &ctx,
        &render_targets.processed_view,
        &upscale_source,
    );

    // Egui: laid out on the main side, painted by the render side.
    let mut egui_ui = ui::EguiUi::new(&window, ctx.device.limits().max_texture_dimension_2d as usize);
    let egui_painter = rendering::egui_painter::EguiPainter::new(&ctx.device, ctx.surface_format);
    // Load the world's graph hierarchy into the embedded editor's selector;
    // the canvas opens on the primary biome.
    let manifest_path = world::world_generator::world_manifest_path();
    match world::world_generator::load_world_graphs(&manifest_path) {
        Ok(graphs) => egui_ui.graph_editor.load(manifest_path, graphs),
        Err(e) => log::error!("Failed to load editor graph hierarchy: {e}"),
    }
    let mut ui_state = UiState::new(initial_params, presets_dir);
    ui_state.gpu = gpu_label;

    // Meshing
    // The startup chunks reach it through the change feed, like any other load.
    let meshing_pipeline = MeshingPipeline::new(&ui_state.params.mesh_cache);
    let meshing = MeshingCoordinator::new(meshing_pipeline);

    // -----------------------------------------------------------------------
    // Insert all resources into the ECS world
    // -----------------------------------------------------------------------

    // Newtype wrappers
    ecs.insert_resource(WindowHandle(window));
    ecs.insert_resource(GlobalUniformBuffer(uniform_buffer));
    ecs.insert_resource(GlobalUniformBindGroup(uniform_bind_group));
    ecs.insert_resource(ShadowUniformBuffer(shadow_uniform_buffer));
    ecs.insert_resource(ReflectionUniformBuffer(reflection_uniform_buffer));
    ecs.insert_resource(ReflectionUniformBindGroup(reflection_bind_group));
    ecs.insert_resource(ShadowBindGroup(shadow_bind_group));
    ecs.insert_resource(ShadowDepthView(shadow_depth_view));
    ecs.insert_resource(MaterialColorBuffer(material_color_buffer));
    ecs.insert_resource(VisibilityMask::new(visibility_mask_texture));
    ecs.insert_resource(VisibilityMaskTexels::default());
    ecs.insert_resource(ShaderDir(shader_dir));
    ecs.insert_resource(LoadedPalette(None));
    ecs.insert_resource(VoxelWorld(world));
    ecs.insert_resource(PresentationClock::new());
    ecs.insert_resource(TickDriver::default());
    ecs.insert_resource(world::tick::CurrentTick::default());
    ecs.insert_resource(world::tick::TickTimings::new());
    if let Some(play) = play {
        ecs.insert_resource(play);
    }
    ecs.insert_resource(RoomState::default());
    ecs.insert_resource(MaterialRegistryRes(material_registry.clone()));
    ecs.insert_resource(prefabs::PrefabRegistryRes(prefab_registry));
    ecs.insert_resource(crate::foliage::FoliageRegistryRes(foliage_registry));
    ecs.insert_resource(libraries::LibrariesRes(libraries));

    // Direct Resource types
    ecs.insert_resource(ctx);
    ecs.insert_resource(surface_state);
    ecs.insert_resource(simulation);
    ecs.insert_resource(cloud_textures);
    ecs.insert_resource(pipeline_registry);
    ecs.insert_resource(pipeline_resources);
    ecs.insert_resource(render_targets);
    ecs.insert_resource(debug_line_pass);
    ecs.insert_resource(upscale_pass);
    ecs.insert_resource(detail_paint_pass);
    ecs.insert_resource(scatter_pass);
    ecs.insert_resource(capsule_pass);
    ecs.insert_resource(canopy_pass);
    ecs.insert_resource(rendering::skeleton_preview_pass::SkeletonPreviewPass::default());
    // Main-side render data; `render_upload_system` copies it into the passes.
    ecs.insert_resource(crate::meshing::overlay::DebugLines::default());
    ecs.insert_resource(crate::meshing::overlay::SkeletonPreview::default());
    ecs.insert_resource(crate::meshing::player::PlayerMesh::default());
    ecs.insert_resource(crate::meshing::cap::CapMesh::default());
    ecs.insert_resource(water_pass);
    ecs.insert_resource(post_process);
    ecs.insert_resource(outline_pass);
    ecs.insert_resource(palette_pass);
    ecs.insert_resource(cap_pass);
    ecs.insert_resource(player_pass);
    ecs.insert_resource(shader_watcher);
    ecs.insert_resource(graph_watcher);
    ecs.insert_resource(meshing);
    ecs.insert_resource(ChunkMeshes::default());
    ecs.insert_resource(WorldRegenCoordinator::new(gen_pool.clone()));
    // `persistence` was opened earlier, before world generation (see above).
    let db_path = persistence.db_path.clone();
    let dict_bytes = persistence.dictionary_bytes().map(|b| Arc::new(b));
    ecs.insert_resource(persistence);
    // Defaults until the headless server (R3c) supplies its own; nothing loads
    // the world's settings from a file yet.
    let server_config = ServerConfig::default();
    let streaming_manager = world::streaming::ChunkStreamingManager::new(
        &server_config.streaming,
        generator.clone(),
        db_path,
        dict_bytes, 
    );
    ecs.insert_resource(streaming_manager);
    ecs.insert_resource(server_config);
    ecs.insert_resource(jobs::JobSystem::new(gen_pool.clone()));
    ecs.insert_resource(ui_state);
    ecs.insert_resource(ui::field_probe::FieldProbe::new(gen_pool.clone()));
    ecs.insert_resource(FrameCounter::new());
    ecs.insert_resource(diagnostics::FrameTimings::new());
    ecs.insert_resource(RawInputBuffer::default());
    let input_bindings_path = paths::asset_root().join("assets").join("input.ron");
    ecs.insert_resource(InputMap::load_or_default(&input_bindings_path));
    ecs.insert_resource(InputState::new());
    ecs.insert_resource(PointerState::default());
    ecs.insert_resource(interaction::PickState::default());
    ecs.insert_resource(interaction::TargetState::default());
    ecs.insert_resource(interaction::PlayerToolState::default());

    // EguiUi - NonSend because egui_winit::State may be !Send
    ecs.insert_non_send_resource(egui_ui);
    ecs.insert_resource(egui_painter);
    ecs.insert_resource(rendering::egui_painter::EguiFrame::default());

    // Initialize a default FrameState so systems don't panic on first frame
    ecs.insert_resource(simulation::manager::FrameState {
        dt: 0.0,
        elapsed: 0.0,
        camera: crate::rendering::camera_desc::CameraDesc::default(),
        dims,
        light_space: glam::Mat4::IDENTITY,
        sun_direction: [0.0, 1.0, 0.0],
        sun_color: [1.0; 3],
        ambient_color: [0.1; 3],
        wind_vector: [0.0; 2],
        clip_min: [-10000.0; 3],
        clip_max: [10000.0; 3],
        clip_enabled: 0,
        mask_origin: [0.0; 3],
        mask_enabled: 0,
        volume_radius: 0.0,
        debug_mode: 0,
        sky_color: [0.5, 0.7, 1.0],
        warm_tint_color: [1.0; 3],
        warm_tint_strength: 0.0,
        clip_dirs: [1.0, 1.0, 1.0],
    });

    // Initialize event queues
    ecs.init_resource::<Events<RegenerateWorld>>();
    ecs.init_resource::<Events<RemeshAll>>();
    ecs.init_resource::<Events<world::events::WorldEvent>>();
    ecs.init_resource::<Events<ClearMeshCache>>();
    ecs.init_resource::<Events<LoadPaletteRequest>>();

    // -----------------------------------------------------------------------
    // Build the schedule
    // -----------------------------------------------------------------------

    ecs.add_schedule(build_tick_schedule());
    let mut schedule = build_frame_schedule();
    if let Some(setup) = bench_setup {
        ecs.insert_resource(bench::BenchRun::new(setup));
        bench::add_systems(&mut schedule);
    }

    (ecs, schedule)
}

// ---------------------------------------------------------------------------
// App struct + ApplicationHandler
// ---------------------------------------------------------------------------

struct App {
    ecs_world: Option<World>,
    schedule: Option<Schedule>,
    exiting: bool,
    /// The world package's play settings, until the ECS world takes them.
    play: Option<crate::play::PlaySettings>,
    /// The GPU power preference the command line asked for.
    gpu: wgpu::PowerPreference,
    /// The benchmark the command line asked for, until the ECS world takes it.
    bench: Option<bench::BenchSetup>,
    /// The process's exit code once the event loop ends: a benchmark's outcome.
    exit_code: i32,
}

impl App {
    fn new(
        play: Option<crate::play::PlaySettings>,
        gpu: wgpu::PowerPreference,
        bench: Option<bench::BenchSetup>,
    ) -> Self {
        Self {
            ecs_world: None,
            schedule: None,
            exiting: false,
            play,
            gpu,
            bench,
            exit_code: 0,
        }
    }
}

impl ApplicationHandler for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ecs_world.is_some() {
            return;
        }

        // A benchmark's window keeps its launch size, which its readings
        // depend on.
        let attrs = WindowAttributes::default()
            .with_title("Voxulacrum")
            .with_inner_size(winit::dpi::PhysicalSize::new(1280u32, 720u32))
            .with_resizable(self.bench.is_none());

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        );

        let (ecs_world, schedule) = init_ecs(window, self.play.take(), self.gpu, self.bench.take());
        self.ecs_world = Some(ecs_world);
        self.schedule = Some(schedule);
        log::info!("ECS world initialized.");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.exiting {
            return; // no more frames once exit is requested - the surface is tearing down
        }
        let Some(ecs) = &mut self.ecs_world else { return; };

        // A benchmark ignores the keyboard and mouse, so nothing typed or moved
        // during a run changes what it measures.
        let user_input = matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
        );
        if user_input && ecs.contains_resource::<bench::BenchRun>() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                if ecs.contains_resource::<bench::BenchRun>() {
                    eprintln!("bench: the window closed before the run finished; no results written");
                    self.exit_code = 1;
                }
                // Save dirty chunks to persistence DB
                {
                    let persistence = ecs.remove_resource::<WorldPersistence>();
                    if let Some(persistence) = persistence {
                        let mut voxel_world = ecs.resource_mut::<VoxelWorld>();
                        match persistence.save_dirty_chunks(&mut voxel_world.0) {
                            Ok(n) if n > 0 => log::info!("Saved {n} dirty chunks on exit"),
                            Err(e) => log::error!("Exit save failed: {e}"),
                            _ => {}
                        }
                    }
                }

                self.exiting = true;
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                // Clone RenderContext (cheap Arc bumps) to avoid borrow conflicts.
                let ctx = ecs.resource::<RenderContext>().clone();

                ecs.resource_mut::<SurfaceState>()
                    .resize(&ctx.device, new_size.width, new_size.height);
                ecs.resource_mut::<SimulationManager>()
                    .camera
                    .resize(new_size.width, new_size.height);

                let zoom = ecs.resource::<SimulationManager>().camera.zoom;
                let density = ecs
                    .resource::<UiState>()
                    .params
                    .render_pipeline
                    .world_pixel_density;
                let dims = compute_render_dimensions(
                    new_size.width,
                    new_size.height,
                    zoom,
                    density,
                );
                let water_layout = ecs
                    .resource::<PipelineResources>()
                    .water_bind_group_layout
                    .clone();

                let new_rt = RenderTargets::new(&ctx, &dims);

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
                ecs.resource_mut::<PalettePass>()
                    .rebuild_bind_group(&ctx, &new_rt.scene_view);
                ecs.resource_mut::<UpscalePass>()
                    .rebuild_bind_group(&ctx, &new_rt.processed_view);
                ecs.resource_mut::<WaterPass>().rebuild_bind_group(
                    &ctx.device,
                    &water_layout,
                    &new_rt.scene_copy_view,
                    &new_rt.depth_sample_view,
                    &new_rt.reflection_color_view,
                    &new_rt.reflection_depth_sample_view,
                );

                *ecs.resource_mut::<RenderTargets>() = new_rt;
            }

            ref e @ WindowEvent::KeyboardInput { ref event, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
               ecs.non_send_resource_mut::<ui::EguiUi>()
                   .handle_event(&window, e);
                if let PhysicalKey::Code(code) = event.physical_key {
                    let raw = if event.state == ElementState::Pressed {
                        RawInputEvent::KeyPressed(code)
                    } else {
                        RawInputEvent::KeyReleased(code)
                    };
                    ecs.resource_mut::<RawInputBuffer>().events.push(raw);
                }
            }

            ref e @ WindowEvent::MouseWheel { ref delta, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiUi>()
                    .handle_event(&window, e);
                let scroll = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => pos.y as f32 * 0.1,
                };
                if scroll.abs() > f32::EPSILON {
                    ecs.resource_mut::<RawInputBuffer>()
                        .events
                        .push(RawInputEvent::Scroll(scroll));
                }
            }

            ref e @ WindowEvent::CursorMoved { position, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiUi>()
                    .handle_event(&window, e);
                ecs.resource_mut::<RawInputBuffer>()
                    .events
                    .push(RawInputEvent::CursorMoved(position.x as f32, position.y as f32));
            }

            ref e @ WindowEvent::MouseInput { state, button, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiUi>()
                    .handle_event(&window, e);
                let mapped = match button {
                    winit::event::MouseButton::Left => Some(PointerButton::Left),
                    winit::event::MouseButton::Right => Some(PointerButton::Right),
                    _ => None,
                };
                if let Some(btn) = mapped {
                    let pressed = matches!(state, ElementState::Pressed);
                    ecs.resource_mut::<RawInputBuffer>()
                        .events
                        .push(RawInputEvent::MouseButton(btn, pressed));
                }
            }

            WindowEvent::RedrawRequested => {
                // Update egui consumption flags before the frame schedule.
                {
                    let egui = ecs.non_send_resource::<ui::EguiUi>();
                    let wants_kb = egui.ctx.wants_keyboard_input();
                    let wants_ptr = egui.ctx.wants_pointer_input();
                    let mut buf = ecs.resource_mut::<RawInputBuffer>();
                    buf.egui_wants_keyboard = wants_kb;
                    buf.egui_wants_pointer = wants_ptr;
                }
                {
                    // Mark this thread as the schedule thread so any accidental
                    // inline graph evaluation trips the debug assertion in
                    // Evaluator::evaluate (generation must run on the pool).
                    let _schedule_guard = nodegraph_eval::enter_schedule_thread();
                    self.schedule
                        .as_mut()
                        .unwrap()
                        .run(self.ecs_world.as_mut().unwrap());
                }
                // A finished benchmark ends the process. Its world is
                // throwaway, so nothing is saved on the way out.
                let finished = self
                    .ecs_world
                    .as_ref()
                    .and_then(|ecs| ecs.get_resource::<bench::BenchRun>())
                    .and_then(|run| run.exit_code());
                if let Some(code) = finished {
                    self.exit_code = code;
                    self.exiting = true;
                    event_loop.exit();
                }
            }

            ref other => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiUi>()
                    .handle_event(&window, other);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ecs) = &self.ecs_world {
            ecs.resource::<WindowHandle>().0.request_redraw();
        }
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

/// How [`run_with`] starts the engine. The default runs the shipped sandbox
/// world from `packages/sandbox/`.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    package: Option<PathBuf>,
}

impl RunOptions {
    /// The defaults: the shipped sandbox package.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start the world of the package at `root`: a directory holding a
    /// `package.ron` manifest beside its content (graphs, materials, prefabs,
    /// libraries, blueprints, species, foliage and play settings). The manifest
    /// names the package's id, its save directory under `saves/` (the id unless
    /// it names another), and the content namespace its names resolve in. Its
    /// `play.ron` gives the world its Play mode; a package without one has none.
    /// Shaders, palettes, input bindings and parameter presets still come from
    /// the engine.
    ///
    /// The package loads when the engine starts. One that is malformed, needs a
    /// different engine version, declares code modules, names no world, or has
    /// a `play.ron` that doesn't validate stops startup with a message saying
    /// why. Its dependencies load first, from `packages/<id>/` beside the
    /// engine, and one that is missing, the wrong version, part of a cycle,
    /// shares another package's namespace, or ships `materials.ron`,
    /// `prefabs.ron` or `foliage.ron` stops startup too.
    ///
    /// A relative `root` is resolved against the working directory.
    pub fn package(mut self, root: impl Into<PathBuf>) -> Self {
        self.package = Some(root.into());
        self
    }
}

/// Runs the engine with the default [`RunOptions`]: the shipped sandbox package.
pub fn run() {
    run_with(RunOptions::default());
}

/// Runs the engine. `--validate-graphs`, `--verify-generation` and
/// `--determinism-manifest` run headless and exit; otherwise this opens the
/// window and enters the event loop. `--bench <scenario>` runs a benchmark
/// scenario in the window, writes its results and exits
/// (`docs/perf-baseline.md`); with `--runs <n>` it runs the scenario `n` times,
/// each in a child process, and summarizes them. Exits with code 1 if the
/// package fails to load or a benchmark fails.
pub fn run_with(options: RunOptions) {
    env_logger::init();
    
    // Before anything reads content, including the headless checks below, so
    // `--verify-generation` checks whichever world this run loads.
    let root = options.package.unwrap_or_else(paths::default_package_root);
    let packages = match crate::package::Package::load_world(&root, &paths::packages_dir()) {
        Ok(packages) => packages,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let (world, dependencies) = packages.split_last().expect("the load order ends with the world's own package");
    let m = &world.manifest;
    log::info!(
        "Package {} {} ({:?}, engine {}) from {} (saves/{})",
        m.id,
        m.version,
        m.kind,
        m.engine,
        root.display(),
        m.saves,
    );
    for d in dependencies {
        log::info!("Dependency {} {} from {}", d.manifest.id, d.manifest.version, d.root.display());
    }
    let mut play = match crate::play::PlaySettings::load(&world.root) {
        Ok(play) => play,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if play.is_none() {
        log::info!("{} has no {}: Play mode is off", m.id, crate::play::PLAY_FILE);
    }
    // Read before `set_packages` takes the load order, which `m` borrows.
    let package_id = m.id.clone();
    paths::set_packages(packages);
    if let Some(play) = &mut play {
        let registry = crate::materials::load_registry();
        if let Err(e) = play.resolve_materials(&registry, paths::namespace()) {
            eprintln!("error: {}: {e}", paths::content_root().join(crate::play::PLAY_FILE).display());
            std::process::exit(1);
        }
    }

    // Headless determinism check for CI (`--verify-generation [N] [--seed S]`).
    // Handled before the event loop exists, so this path never creates a window
    // or a GPU device - and, being outside the frame schedule, it is exempt from
    // `nodegraph_eval`'s schedule-thread evaluation guard for the same reason
    // the startup world fill is.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let gpu = match gpu_preference(&args) {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    if let Some(code) = validate::run_from_args(&args) {
        std::process::exit(code);
    }
    if let Some(code) = verify::run_from_args(&args) {
        std::process::exit(code);
    }
    if let Some(code) = determinism::run_from_args(&args) {
        std::process::exit(code);
    }
    let bench_setup = match bench::BenchSetup::from_args(&args, &package_id) {
        Ok(setup) => setup,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    // A repeated benchmark opens no window of its own: it runs each pass as a
    // child process and exits once they are done.
    match (bench::repeat::Repeat::from_args(&args), &bench_setup) {
        (Err(e), _) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        (Ok(Some(repeat)), Some(setup)) => {
            std::process::exit(bench::repeat::run(repeat, &args, setup));
        }
        (Ok(Some(_)), None) => {
            eprintln!("error: --runs repeats a benchmark, so it needs --bench <scenario>");
            std::process::exit(1);
        }
        (Ok(None), _) => {}
    }
    if bench_setup.is_some() {
        if let Err(e) = bench::clear_save() {
            eprintln!("error: the benchmark's save could not be cleared: {e}");
            std::process::exit(1);
        }
    }
    
    log::info!("Voxulacrum engine starting...");

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new(play, gpu, bench_setup);
    event_loop.run_app(&mut app).expect("Event loop failed");
    if app.exit_code != 0 {
        std::process::exit(app.exit_code);
    }
}


/// The GPU power preference `--gpu <low-power|high-performance>` asks for:
/// high performance unless the command line names the other. `low-power`
/// picks the integrated GPU on a machine with two, for the minimum-spec
/// baseline (roadmap §7.3).
fn gpu_preference(args: &[String]) -> Result<wgpu::PowerPreference, String> {
    let Some(i) = args.iter().position(|a| a == "--gpu") else {
        return Ok(wgpu::PowerPreference::HighPerformance);
    };
    match args.get(i + 1).map(String::as_str) {
        Some("high-performance") => Ok(wgpu::PowerPreference::HighPerformance),
        Some("low-power") => Ok(wgpu::PowerPreference::LowPower),
        Some(other) => Err(format!("--gpu takes `low-power` or `high-performance`, not {other:?}")),
        None => Err("--gpu takes `low-power` or `high-performance`".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_gpu_defaults_to_high_performance() {
        assert_eq!(gpu_preference(&args("--validate-graphs")), Ok(wgpu::PowerPreference::HighPerformance));
    }

    #[test]
    fn the_gpu_flag_picks_a_power_preference() {
        assert_eq!(gpu_preference(&args("--gpu low-power")), Ok(wgpu::PowerPreference::LowPower));
        assert_eq!(
            gpu_preference(&args("--verify-generation 64 --gpu high-performance")),
            Ok(wgpu::PowerPreference::HighPerformance)
        );
    }

    #[test]
    fn a_gpu_flag_without_a_known_value_is_refused() {
        assert!(gpu_preference(&args("--gpu fast")).unwrap_err().contains("\"fast\""));
        assert!(gpu_preference(&args("--gpu")).is_err());
    }
}
