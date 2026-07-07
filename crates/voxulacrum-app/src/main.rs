mod camera;
mod cloud_shadow;
mod ecs;
mod meshing;
mod params;
mod rendering;
mod shader_reload;
mod graph_reload;
mod simulation;
mod ui;
mod world;
mod palette;
mod input;
mod interaction;
mod paths;
mod materials;
mod prefabs;

use std::path::PathBuf;
use std::sync::Arc;
use bevy_ecs::prelude::*;
use bevy_ecs::event::Events;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowAttributes, WindowId};

use rendering::render_targets::RenderTargets;
use rendering::render_context::RenderContext;
use rendering::surface_state::SurfaceState;
use rendering::pipelines::{PipelineRegistry, PipelineResources};
use rendering::uniforms::{self, GlobalUniforms, ShadowUniforms};
use rendering::debug_lines::DebugLinePass;
use rendering::upscale_pass::UpscalePass;
use rendering::detail_paint_pass::DetailPaintPass;
use rendering::scatter_pass::ScatterPass;
use rendering::water_pass::WaterPass;
use rendering::post_process::PostProcessPass;
use rendering::outline_pass::OutlinePass;
use rendering::palette_pass::PalettePass;
use rendering::cap_pass::CapPass;
use shader_reload::ShaderWatcher;
use simulation::manager::SimulationManager;
use meshing::coordinator::MeshingCoordinator;
use meshing::MeshingPipeline;
use world::regen::WorldRegenCoordinator;
use world::persistence::WorldPersistence;
use camera::IsometricCamera;
use cloud_shadow::CloudShadowState;
use params::EngineParams;
use ui::panels::UiState;
use materials::MaterialRegistryRes;

use ecs::resources::*;
use ecs::events::*;
use ecs::schedule::build_frame_schedule;
use input::{InputMap, InputState, PointerButton, PointerState, RawInputBuffer, RawInputEvent};

const SHADOW_MAP_SIZE: u32 = 4096;

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

pub fn compute_render_dimensions(
    surface_w: u32,
    surface_h: u32,
    zoom: f32,
    world_pixel_density: f32,
) -> (u32, u32, f32) {
    let base_scale = (surface_h as f32 / (zoom * 2.0)) / world_pixel_density;
    let scale = base_scale.max(1.0).round();
    let rw = ((surface_w as f32 / scale).ceil() as u32).max(1);
    let rh = ((surface_h as f32 / scale).ceil() as u32).max(1);
    (rw, rh, scale)
}

// ---------------------------------------------------------------------------
// ECS initialization
// ---------------------------------------------------------------------------

fn init_ecs(window: Arc<Window>) -> (bevy_ecs::world::World, Schedule) {
    let mut ecs = bevy_ecs::world::World::new();

    // --- wgpu initialization ---
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        ..Default::default()
    });
    let surface = instance
        .create_surface(window.clone())
        .expect("Failed to create surface");
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .expect("No suitable GPU adapter found");

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

    // Params
    let presets_dir = PathBuf::from("params");
    let initial_params = EngineParams::load(&presets_dir.join("default.json"))
        .unwrap_or_default();

    let mut camera = IsometricCamera::new(&initial_params.camera);
    camera.resize(surface_state.surface_config.width, surface_state.surface_config.height);

    // Render targets
    let (rw, rh, eff_scale) = compute_render_dimensions(
        surface_state.surface_config.width,
        surface_state.surface_config.height,
        camera.zoom,
        initial_params.render_pipeline.world_pixel_density,
    );
    let render_targets = RenderTargets::new(&ctx, rw, rh, eff_scale);

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
        size: std::mem::size_of::<GlobalUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let cloud_shadow = CloudShadowState::new(&ctx, &initial_params.cloud);

    let uniform_bind_group = uniforms::create_bind_group(
        &ctx.device,
        &global_bind_group_layout,
        &uniform_buffer,
        &cloud_shadow.texture_view,
        &cloud_shadow.sampler,
        &shadow_depth_view,
        &shadow_sampler,
    );

    let simulation = SimulationManager::new(camera, cloud_shadow);

    let shadow_bind_group_layout =
        uniforms::create_shadow_bind_group_layout(&ctx.device);
    let shadow_uniform_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shadow_uniform_buffer"),
        size: std::mem::size_of::<ShadowUniforms>() as u64,
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

    let pipeline_resources = PipelineResources {
        surface_format: ctx.surface_format,
        global_bind_group_layout,
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

    // World
    let gen_start = std::time::Instant::now();

    let min_y = initial_params.streaming.min_chunk_y;
    let max_y = initial_params.streaming.max_chunk_y;

    // The single graph-backed generator, shared (via Arc) by the World, the
    // streaming workers, and background regeneration.
    let generator = world::world_generator::load_default(&initial_params.terrain_gen)
        .expect("Failed to load default_biome graph generator");
    
    // Data-driven material registry (RON primary, built-in fallback). Shared by
    // the vegetation pass and any system needing material metadata.
    let material_registry = materials::load_registry();
    let prefab_registry = prefabs::load_prefab_registry();

    // One bounded rayon pool shared by the startup fill, streaming generation,
    // and background regeneration - the single worker-pool model for all chunk
    // generation (engine-design.md §12).
    let gen_pool: Arc<rayon::ThreadPool> = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_cpus::get().saturating_sub(2).max(2))
            .thread_name(|i| format!("chunk-gen-{i}"))
            .build()
            .expect("failed to build chunk generation thread pool"),
    );

    // World cache disabled: it stored only voxels (no foliage) and went stale as
    // the generation pipeline grew (graphs, slabs, foliage), so cached chunks
    // loaded without grass/scatter. Always regenerate - the generate path
    // produces slabs + foliage identical to streamed reloads.
    let world = world::World::generate(generator.clone(), &gen_pool, min_y, max_y);
    log::info!("World generated in {:.2?}", gen_start.elapsed());
    world.print_debug_stats(&material_registry);

    // Tier-1 detail paint + Tier-2/3 scatter + water
    let detail_paint_pass = DetailPaintPass::new(&ctx, &world);
    let scatter_pass = ScatterPass::new(&ctx, &world, &prefab_registry);
    let water_pass = WaterPass::new();

    // Post-process pass
    let pp_source = std::fs::read_to_string(shader_dir.join("post_process.wgsl"))
        .expect("Failed to read post_process.wgsl");
    let post_process = PostProcessPass::new(
        &ctx,
        &render_targets.processed_view,
        &render_targets.depth_view,
        &pp_source,
    );

    let outline_source =
        std::fs::read_to_string(shader_dir.join("outline.wgsl"))
            .expect("Failed to read outline.wgsl");
    let outline_pass = OutlinePass::new(
        &ctx,
        &render_targets.scene_view,
        &render_targets.depth_view,
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

    // Egui
    let mut egui_renderer = ui::EguiRenderer::new(&ctx, &window);
    // Load the world's graph hierarchy into the embedded editor's selector;
    // the canvas opens on the primary biome.
    match world::world_generator::load_world_graphs(&world::world_generator::world_manifest_path()) {
        Ok(graphs) => egui_renderer.graph_editor.load(graphs),
        Err(e) => log::error!("Failed to load editor graph hierarchy: {e}"),
    }
    let ui_state = UiState::new(initial_params, presets_dir);

    // Meshing
    let mut meshing_pipeline = MeshingPipeline::new(
        &ui_state.params.materials,
        &ui_state.params.meshing,
        &ui_state.params.mesh_cache,
    );
    meshing_pipeline.submit_all_dirty(&world);
    let meshing = MeshingCoordinator::new(meshing_pipeline);

    // -----------------------------------------------------------------------
    // Insert all resources into the ECS world
    // -----------------------------------------------------------------------

    // Newtype wrappers
    ecs.insert_resource(WindowHandle(window));
    ecs.insert_resource(GlobalUniformBuffer(uniform_buffer));
    ecs.insert_resource(GlobalUniformBindGroup(uniform_bind_group));
    ecs.insert_resource(ShadowUniformBuffer(shadow_uniform_buffer));
    ecs.insert_resource(ShadowBindGroup(shadow_bind_group));
    ecs.insert_resource(ShadowDepthView(shadow_depth_view));
    ecs.insert_resource(ShaderDir(shader_dir));
    ecs.insert_resource(LoadedPalette(None));
    ecs.insert_resource(VoxelWorld(world));
    ecs.insert_resource(MaterialRegistryRes(material_registry.clone()));
    ecs.insert_resource(prefabs::PrefabRegistryRes(prefab_registry));

    // Direct Resource types
    ecs.insert_resource(ctx);
    ecs.insert_resource(surface_state);
    ecs.insert_resource(simulation);
    ecs.insert_resource(pipeline_registry);
    ecs.insert_resource(pipeline_resources);
    ecs.insert_resource(render_targets);
    ecs.insert_resource(debug_line_pass);
    ecs.insert_resource(upscale_pass);
    ecs.insert_resource(detail_paint_pass);
    ecs.insert_resource(scatter_pass);
    ecs.insert_resource(water_pass);
    ecs.insert_resource(post_process);
    ecs.insert_resource(outline_pass);
    ecs.insert_resource(palette_pass);
    ecs.insert_resource(cap_pass);
    ecs.insert_resource(shader_watcher);
    ecs.insert_resource(graph_watcher);
    ecs.insert_resource(meshing);
    ecs.insert_resource(WorldRegenCoordinator::new(gen_pool.clone()));
    let persistence = match WorldPersistence::open("default", ui_state.params.terrain_gen.seed) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("Persistence init failed: {e}. Running without saves.");
            WorldPersistence::disabled()
        }
    };
    let db_path = persistence.db_path.clone();
    let dict_bytes = persistence.dictionary_bytes().map(|b| Arc::new(b));
    ecs.insert_resource(persistence);
    let streaming_manager = world::streaming::ChunkStreamingManager::new(
        gen_pool.clone(),
        &ui_state.params.streaming,
        generator.clone(),
        db_path,
        dict_bytes, 
    );
    ecs.insert_resource(streaming_manager);
    ecs.insert_resource(ui_state);
    ecs.insert_resource(ui::field_probe::FieldProbe::new(gen_pool.clone()));
    ecs.insert_resource(FrameCounter::new());
    ecs.insert_resource(RawInputBuffer::default());
    ecs.insert_resource(InputMap::default());
    ecs.insert_resource(InputState::new());
    ecs.insert_resource(PointerState::default());
    ecs.insert_resource(interaction::PickState::default());

    // EguiRenderer — NonSend because egui_winit::State may be !Send
    ecs.insert_non_send_resource(egui_renderer);

    // Initialize a default FrameState so systems don't panic on first frame
    ecs.insert_resource(simulation::manager::FrameState {
        dt: 0.0,
        elapsed: 0.0,
        view_proj: [[0.0; 4]; 4],
        subpixel_offset: [0.0; 2],
        light_space: glam::Mat4::IDENTITY,
        sun_direction: [0.0, 1.0, 0.0],
        sun_color: [1.0; 3],
        ambient_color: [0.1; 3],
        wind_vector: [0.0; 2],
        cloud_shadow_offset: [0.0; 2],
        cloud_coverage: 0.0,
        clip_min: [-10000.0; 3],
        clip_max: [10000.0; 3],
        clip_enabled: 0,
        debug_mode: 0,
        sky_color: [0.5, 0.7, 1.0],
        warm_tint_color: [1.0; 3],
        warm_tint_strength: 0.0,
        cos_r: 1.0,
        sin_r: 0.0,
    });

    // Initialize event queues
    ecs.init_resource::<Events<RegenerateWorld>>();
    ecs.init_resource::<Events<RemeshAll>>();
    ecs.init_resource::<Events<ClearMeshCache>>();
    ecs.init_resource::<Events<LoadPaletteRequest>>();

    // -----------------------------------------------------------------------
    // Build the schedule
    // -----------------------------------------------------------------------

    let schedule = build_frame_schedule();

    (ecs, schedule)
}

// ---------------------------------------------------------------------------
// App struct + ApplicationHandler
// ---------------------------------------------------------------------------

struct App {
    ecs_world: Option<bevy_ecs::world::World>,
    schedule: Option<Schedule>,
}

impl App {
    fn new() -> Self {
        Self {
            ecs_world: None,
            schedule: None,
        }
    }
}

impl ApplicationHandler for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ecs_world.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("Voxulacrum")
            .with_inner_size(winit::dpi::PhysicalSize::new(1280u32, 720u32));

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        );

        let (ecs_world, schedule) = init_ecs(window);
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
        let Some(ecs) = &mut self.ecs_world else { return; };

        match event {
            WindowEvent::CloseRequested => {
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
                let (rw, rh, eff_scale) = compute_render_dimensions(
                    new_size.width,
                    new_size.height,
                    zoom,
                    density,
                );

                let new_rt = RenderTargets::new(&ctx, rw, rh, eff_scale);

                ecs.resource_mut::<OutlinePass>().rebuild_bind_group(
                    &ctx,
                    &new_rt.scene_view,
                    &new_rt.depth_view,
                    &new_rt.normal_view,
                );
                ecs.resource_mut::<PostProcessPass>().rebuild_bind_group(
                    &ctx,
                    &new_rt.processed_view,
                    &new_rt.depth_view,
                );
                ecs.resource_mut::<PalettePass>()
                    .rebuild_bind_group(&ctx, &new_rt.scene_view);
                ecs.resource_mut::<UpscalePass>()
                    .rebuild_bind_group(&ctx, &new_rt.processed_view);

                *ecs.resource_mut::<RenderTargets>() = new_rt;
            }

            ref e @ WindowEvent::KeyboardInput { ref event, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
               ecs.non_send_resource_mut::<ui::EguiRenderer>()
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
                ecs.non_send_resource_mut::<ui::EguiRenderer>()
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
                ecs.non_send_resource_mut::<ui::EguiRenderer>()
                    .handle_event(&window, e);
                ecs.resource_mut::<RawInputBuffer>()
                    .events
                    .push(RawInputEvent::CursorMoved(position.x as f32, position.y as f32));
            }

            ref e @ WindowEvent::MouseInput { state, button, .. } => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiRenderer>()
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
                    let egui = ecs.non_send_resource::<ui::EguiRenderer>();
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
            }

            ref other => {
                let window = ecs.resource::<WindowHandle>().0.clone();
                ecs.non_send_resource_mut::<ui::EguiRenderer>()
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
// main
// ---------------------------------------------------------------------------

fn main() {
    env_logger::init();
    log::info!("Voxulacrum engine starting...");

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop failed");
}