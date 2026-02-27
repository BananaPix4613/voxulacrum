mod camera;
mod cloud_shadow;
mod meshing;
mod params;
mod rendering;
mod shader_reload;
mod simulation;
mod ui;
mod world;
mod palette;

use std::path::PathBuf;
use std::sync::Arc;
use glam::IVec3;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use rendering::render_graph::{RenderGraph, ResourceId, ResourceMap};
use rendering::shadow_pass::ShadowPassNode;
use rendering::main_scene_pass::{MainScenePassNode, CapConfig};
use simulation::manager::SimulationManager;
use meshing::coordinator::MeshingCoordinator;
use world::regen::WorldRegenCoordinator;
use camera::IsometricCamera;
use cloud_shadow::CloudShadowState;
use params::{EngineParams};
use rendering::gpu_state::GpuState;
use rendering::pipelines::{PipelineRegistry, PipelineResources};
use rendering::render_targets::RenderTargets;
use rendering::uniforms::{self, GlobalUniforms, ShadowUniforms};
use rendering::debug_lines::DebugLinePass;
use rendering::upscale_pass::UpscalePass;
use rendering::vegetation_pass::VegetationPass;
use rendering::water_pass::WaterPass;
use rendering::post_process::PostProcessPass;
use rendering::outline_pass::OutlinePass;
use rendering::palette_pass::PalettePass;
use rendering::cap_pass::CapPass;
use palette::Palette;
use shader_reload::ShaderWatcher;
use simulation::water::StaticWater;
use ui::panels::UiState;
use meshing::MeshingPipeline;

const SHADOW_MAP_SIZE: u32 = 4096;

struct FrameCounter {
    last_second: std::time::Instant,
    frames_this_second: u32,
    fps: f32,
}

impl FrameCounter {
    fn new() -> Self {
        Self {
            last_second: std::time::Instant::now(),
            frames_this_second: 0,
            fps: 0.0,
        }
    }

    fn tick(&mut self) {
        self.frames_this_second += 1;
        if self.last_second.elapsed().as_secs_f32() >= 1.0 {
            self.fps = self.frames_this_second as f32;
            self.frames_this_second = 0;
            self.last_second = std::time::Instant::now();
        }
    }
}

struct AppState {
    window: Arc<Window>,
    gpu: GpuState,
    simulation: SimulationManager,
    pipeline_registry: PipelineRegistry,
    pipeline_resources: PipelineResources,
    render_targets: RenderTargets,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    shadow_uniform_buffer: wgpu::Buffer,
    shadow_bind_group: wgpu::BindGroup,
    shadow_depth_view: wgpu::TextureView,
    world: world::World,
    debug_line_pass: DebugLinePass,
    upscale_pass: UpscalePass,
    vegetation_pass: VegetationPass,
    static_water: StaticWater,
    water_pass: WaterPass,
    post_process: PostProcessPass,
    outline_pass: OutlinePass,
    palette_pass: PalettePass,
    loaded_palette: Option<Palette>,
    egui_renderer: ui::EguiRenderer,
    ui_state: UiState,
    frame_counter: FrameCounter,
    shader_watcher: ShaderWatcher,
    shader_dir: PathBuf,
    meshing: MeshingCoordinator,
    world_regen: WorldRegenCoordinator,
    cap_pass: CapPass,
}

impl AppState {
    fn new(window: Arc<Window>) -> Self {
        let gpu = GpuState::new(window.clone());

        // Initialize params (try loading saved, fall back to defaults)
        let presets_dir = PathBuf::from("params");
        let initial_params = EngineParams::load(&presets_dir.join("default.json"))
            .unwrap_or_default();

        let mut camera = IsometricCamera::new(&initial_params.camera);
        camera.resize(gpu.surface_config.width, gpu.surface_config.height);

        // Low-res render targets
        let (rw, rh, eff_scale) = compute_render_dimensions(
            gpu.surface_config.width, gpu.surface_config.height,
            camera.zoom, initial_params.render_pipeline.world_pixel_density,
        );
        let render_targets = RenderTargets::new(
            &gpu.device, rw, rh, eff_scale, gpu.surface_format,
        );

        // Shadow map
        let shadow_depth_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_depth_view =
            shadow_depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let shadow_sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow_comparison_sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let global_bind_group_layout = uniforms::create_bind_group_layout(&gpu.device);

        let uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("global_uniform_buffer"),
            size: size_of::<GlobalUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cloud_shadow = CloudShadowState::new(&gpu.device, &gpu.queue, &initial_params.cloud);

        let uniform_bind_group = uniforms::create_bind_group(
            &gpu.device,
            &global_bind_group_layout,
            &uniform_buffer,
            &cloud_shadow.texture_view,
            &cloud_shadow.sampler,
            &shadow_depth_view,
            &shadow_sampler,
        );
        
        // Simulation manager
        let simulation = SimulationManager::new(camera, cloud_shadow);

        let shadow_bind_group_layout = uniforms::create_shadow_bind_group_layout(&gpu.device);
        let shadow_uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_uniform_buffer"),
            size: size_of::<ShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_bind_group = uniforms::create_shadow_bind_group(
            &gpu.device, &shadow_bind_group_layout, &shadow_uniform_buffer,
        );

        // Shader directory and watcher
        let shader_dir = PathBuf::from("shaders");
        let shader_watcher = ShaderWatcher::new(&shader_dir);

        // Pipeline resources for hot-reloading
        let post_process_bind_group_layout =
            uniforms::create_post_process_bind_group_layout(&gpu.device);

        let pipeline_resources = PipelineResources {
            surface_format: gpu.surface_format,
            global_bind_group_layout,
            shadow_bind_group_layout,
            post_process_bind_group_layout,
        };

        // Pipeline registry loads shaders from disk
        let pipeline_registry = PipelineRegistry::new(
            &gpu.device,
            &pipeline_resources,
            &shader_dir,
        );

        let debug_lines_source = std::fs::read_to_string(shader_dir.join("debug_lines.wgsl"))
            .expect("Failed to read debug_lines.wgsl");
        let debug_line_pass = DebugLinePass::new(
            &gpu.device,
            gpu.surface_format,
            &pipeline_resources.global_bind_group_layout,
            &debug_lines_source,
        );

        let cap_source = std::fs::read_to_string(shader_dir.join("cap.wgsl"))
            .expect("Failed to read cap.wgsl");
        let cap_pass = CapPass::new(
            &gpu.device,
            gpu.surface_format,
            &pipeline_resources.global_bind_group_layout,
            &cap_source,
        );

        // Generate or load world using params
        let gen_start = std::time::Instant::now();
        let cache_dir = PathBuf::from("cache/meshes");
        let world_key = meshing::cache::compute_world_cache_key(&initial_params.terrain_gen);
        let world_cache_path = meshing::cache::world_cache_path(&cache_dir, world_key);

        let world = if let Some(chunks) = meshing::cache::load_world_cache(&world_cache_path, world_key) {
            log::info!("Loaded world from cache in {:.2?} ({} chunks)", gen_start.elapsed(), chunks.len());
            world::World::from_cached_chunks(chunks, &initial_params.terrain_gen)
        } else {
            let w = world::World::generate(&initial_params.terrain_gen);
            log::info!("World generated in {:.2?}", gen_start.elapsed());
            // Save to cache for next launch
            if let Err(e) = meshing::cache::save_world_cache(&world_cache_path, world_key, &w) {
                log::warn!("Failed to save world cache: {}", e);
            } else {
                log::info!("World saved to cache");
            }
            w
        };
        world.print_debug_stats();

        // Vegetation
        let veg_start = std::time::Instant::now();
        let vegetation_pass = VegetationPass::new(&gpu.device, &world, &initial_params.vegetation);
        log::info!("Vegetation pass in {:.2?}", veg_start.elapsed());

        // Water
        let static_water = StaticWater::new(&world, &initial_params.water, &initial_params.terrain_gen);
        let water_pass = WaterPass::new(&gpu.device, &static_water);
        log::info!("Water simulation initialized");

        // Post-processing (load shader from disk)
        let pp_shader_source = std::fs::read_to_string(shader_dir.join("post_process.wgsl"))
            .expect("Failed to read post_process.wgsl");
        let post_process = PostProcessPass::new(
            &gpu.device, gpu.surface_format, &render_targets.processed_view,
            &render_targets.depth_view, &pp_shader_source,
        );

        // Outline pass
        let outline_shader_source = std::fs::read_to_string(shader_dir.join("outline.wgsl"))
            .expect("Failed to read outline.wgsl");
        let outline_pass = OutlinePass::new(
            &gpu.device,
            gpu.surface_format,
            &render_targets.scene_view,
            &render_targets.depth_view,
            &render_targets.normal_view,
            &outline_shader_source,
        );

        // Palette quantization pass
        let palette_shader_source = std::fs::read_to_string(shader_dir.join("palette.wgsl"))
            .expect("Failed to read palette.wgsl");
        let palette_pass = PalettePass::new(
            &gpu.device, gpu.surface_format, &render_targets.scene_view, &palette_shader_source,
        );

        // Upscaling
        let upscale_shader_source = std::fs::read_to_string(shader_dir.join("upscale.wgsl"))
            .expect("Failed to read upscale.wgsl");
        let upscale_pass = UpscalePass::new(
            &gpu.device,
            gpu.surface_format,
            &render_targets.processed_view,
            &upscale_shader_source,
        );

        // Egui
        let egui_renderer = ui::EguiRenderer::new(&gpu.device, gpu.surface_format, &window);
        let ui_state = UiState::new(initial_params, presets_dir);

        let mut meshing_pipeline = MeshingPipeline::new(
            world.chunks_x, world.chunks_y, world.chunks_z,
            &ui_state.params.materials, &ui_state.params.meshing,
        );
        meshing_pipeline.submit_all_dirty(&world);
        let meshing = MeshingCoordinator::new(meshing_pipeline);

        Self {
            window,
            gpu,
            simulation,
            pipeline_registry,
            pipeline_resources,
            render_targets,
            uniform_buffer,
            uniform_bind_group,
            shadow_uniform_buffer,
            shadow_bind_group,
            shadow_depth_view,
            world,
            debug_line_pass,
            upscale_pass,
            vegetation_pass,
            static_water,
            water_pass,
            post_process,
            outline_pass,
            palette_pass,
            loaded_palette: None,
            egui_renderer,
            ui_state,
            frame_counter: FrameCounter::new(),
            shader_watcher,
            shader_dir,
            meshing,
            world_regen: WorldRegenCoordinator::new(),
            cap_pass,
        }
    }

    fn check_shader_hot_reload(&mut self) {
        let changed = self.shader_watcher.poll_changes();
        for path in changed {
            let filename = match path.file_name().and_then(|f| f.to_str()) {
                Some(f) => f.to_string(),
                None => continue,
            };

            // Handle post_process.wgsl separately since its pipeline lives in PostProcessPass
            if filename == "post_process.wgsl" {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        match self.post_process.try_reload_shader(
                            &self.gpu.device,
                            self.gpu.surface_format,
                            &source,
                        ) {
                            Ok(()) => {
                                log::info!("Reloaded post_process.wgsl");
                                self.ui_state.push_shader_log(
                                    "post_process.wgsl reloaded".to_string(),
                                    false,
                                );
                            }
                            Err(e) => {
                                log::warn!("post_process.wgsl compile error: {}", e);
                                self.ui_state.push_shader_log(
                                    format!("post_process.wgsl: {}", e),
                                    true,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to read post_process.wgsl: {}", e);
                        self.ui_state.push_shader_log(
                            format!("post_process.wgsl read error: {}", e),
                            true,
                        );
                    }
                }
                continue;
            }

            // Handle outline.wgsl separately since its pipeline lives in OutlinePass
            if filename == "outline.wgsl" {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        match self.outline_pass.try_reload_shader(
                            &self.gpu.device,
                            self.gpu.surface_format,
                            &source,
                        ) {
                            Ok(()) => {
                                log::info!("Reloaded outline.wgsl");
                                self.ui_state.push_shader_log(
                                    "outline.wgsl reloaded".to_string(),
                                    false,
                                );
                            }
                            Err(e) => {
                                log::warn!("outline.wgsl compile error: {}", e);
                                self.ui_state.push_shader_log(
                                    format!("outline.wgsl: {}", e),
                                    true,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to read outline.wgsl: {}", e);
                        self.ui_state.push_shader_log(
                            format!("outline.wgsl read error: {}", e),
                            true,
                        );
                    }
                }
                continue;
            }

            // Handle palette.wgsl separately since its pipeline lives in PalettePass
            if filename == "palette.wgsl" {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        match self.palette_pass.try_reload_shader(
                            &self.gpu.device,
                            self.gpu.surface_format,
                            &source,
                        ) {
                            Ok(()) => {
                                log::info!("Reloaded palette.wgsl");
                                self.ui_state.push_shader_log(
                                    "palette.wgsl reloaded".to_string(),
                                    false,
                                );
                            }
                            Err(e) => {
                                log::warn!("palette.wgsl compile error: {}", e);
                                self.ui_state.push_shader_log(
                                    format!("palette.wgsl: {}", e),
                                    true,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to read palette.wgsl: {}", e);
                        self.ui_state.push_shader_log(
                            format!("palette.wgsl read error: {}", e),
                            true,
                        );
                    }
                }
                continue;
            }

            // Handle cap.wgsl separately since its pipeline lives in CapPass
            if filename == "cap.wgsl" {
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        match self.cap_pass.try_reload_shader(
                            &self.gpu.device,
                            self.gpu.surface_format,
                            &self.pipeline_resources.global_bind_group_layout,
                            &source,
                        ) {
                            Ok(()) => {
                                log::info!("Reloaded cap.wgsl");
                                self.ui_state.push_shader_log(
                                    "cap.wgsl reloaded".to_string(),
                                    false,
                                );
                            }
                            Err(e) => {
                                log::warn!("cap.wgsl compile error: {}", e);
                                self.ui_state.push_shader_log(
                                    format!("cap.wgsl: {}", e),
                                    true,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to read cap.wgsl: {}", e);
                        self.ui_state.push_shader_log(
                            format!("cap.wgsl read error: {}", e),
                            true,
                        );
                    }
                }
                continue;
            }

            // All other shaders go through the pipeline registry
            if let Some(result) = self.pipeline_registry.try_reload(
                &self.gpu.device,
                &self.pipeline_resources,
                &path,
            ) {
                if result.success {
                    log::info!("Reloaded {}", result.filename);
                } else {
                    log::warn!("{}: {}", result.filename, result.message);
                }
                self.ui_state.push_shader_log(
                    format!("{}: {}", result.filename, result.message),
                    !result.success,
                );
            }
        }
    }

    fn render(&mut self) {
        // Frame stats
        self.frame_counter.tick();
        self.ui_state.fps = self.frame_counter.fps;

        // Shader hot-reload
        self.check_shader_hot_reload();

        // Render target resize check
        let (rw, rh, eff_scale) = compute_render_dimensions(
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            self.simulation.camera.zoom,
            self.ui_state.params.render_pipeline.world_pixel_density,
        );
        if self.render_targets.needs_recreate(rw, rh) {
            self.render_targets = RenderTargets::new(
                &self.gpu.device, rw, rh, eff_scale, self.gpu.surface_format,
            );
            self.outline_pass.rebuild_bind_group(
                &self.gpu.device,
                &self.render_targets.scene_view,
                &self.render_targets.depth_view,
                &self.render_targets.normal_view,
            );
            self.post_process.rebuild_bind_group(
                &self.gpu.device,
                &self.render_targets.processed_view,
                &self.render_targets.depth_view,
            );
            self.palette_pass.rebuild_bind_group(
                &self.gpu.device,
                &self.render_targets.scene_view
            );
            self.upscale_pass.rebuild_bind_group(
                &self.gpu.device,
                &self.render_targets.processed_view
            );
        }

        // Meshing tick
        self.meshing.tick(&mut self.world, &self.gpu.device, &mut self.ui_state);

        // Palette load request
        if self.ui_state.palette_load_requested {
            self.ui_state.palette_load_requested = false;
            let name = &self.ui_state.params.palette.selected_palette;
            if !name.is_empty() {
                let path = PathBuf::from("palettes").join(format!("{}.json", name));
                match palette::load_palette(&path) {
                    Ok(pal) => {
                        log::info!("Loaded palette: {}", pal.name);
                        self.ui_state.loaded_palette_preview = pal.colors_srgb.clone();
                        self.loaded_palette = Some(pal);
                    }
                    Err(e) => {
                        log::warn!("Failed to load palette {}: {}", name, e);
                        self.loaded_palette = None;
                        self.ui_state.loaded_palette_preview.clear();
                    }
                }
            } else {
                self.loaded_palette = None;
                self.ui_state.loaded_palette_preview.clear();
            }
        }

        // Simulation tick
        let frame = self.simulation.tick(&self.ui_state.params, &self.render_targets);
        self.ui_state.frame_time_ms = frame.dt * 1000.0;

        // Param change detection
        let change_kind = self.ui_state.change_detector.detect(&self.ui_state.params);
        if change_kind == params::ParamChangeKind::MeshInvalidating {
            self.ui_state.mesh_params_pending = true;
        }
        self.ui_state.change_detector.snapshot(&self.ui_state.params);

        // Write all uniform buffers
        rendering::uniform_writer::write_all_uniforms(
            &self.gpu.queue,
            &frame,
            &self.ui_state.params,
            &self.uniform_buffer,
            &self.shadow_uniform_buffer,
            &self.post_process,
            &self.outline_pass,
            &self.palette_pass,
            &self.upscale_pass,
            &self.render_targets,
            &self.loaded_palette,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
        );

        // Performance stats
        let mut total_tris: u64 = 0;
        let mut chunks_visible: u32 = 0;
        let mut chunks_total: u32 = 0;
        for chunk in &self.world.chunks {
            if chunk.mesh.is_some() {
                chunks_total += 1;
                if self.simulation.frustum.is_chunk_visible(chunk.position) {
                    chunks_visible += 1;
                    total_tris += chunk.mesh.as_ref().unwrap().index_count as u64 / 3;
                }
            }
        }
        self.ui_state.total_triangles = total_tris;
        self.ui_state.chunks_visible = chunks_visible;
        self.ui_state.chunks_total = chunks_total;

        // Acquire swapchain
        let surface_frame = match self.gpu.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = self.window.inner_size();
                self.gpu.resize(size.width, size.height);
                let (rw, rh, eff_scale) = compute_render_dimensions(
                    size.width, size.height,
                    self.simulation.camera.zoom,
                    self.ui_state.params.render_pipeline.world_pixel_density,
                );
                self.render_targets = RenderTargets::new(
                    &self.gpu.device, rw, rh, eff_scale, self.gpu.surface_format,
                );
                self.outline_pass.rebuild_bind_group(
                    &self.gpu.device,
                    &self.render_targets.scene_view,
                    &self.render_targets.depth_view,
                    &self.render_targets.normal_view,
                );
                self.post_process.rebuild_bind_group(
                    &self.gpu.device,
                    &self.render_targets.processed_view,
                    &self.render_targets.depth_view,
                );
                self.palette_pass.rebuild_bind_group(
                    &self.gpu.device,
                    &self.render_targets.scene_view
                );
                self.upscale_pass.rebuild_bind_group(
                    &self.gpu.device,
                    &self.render_targets.processed_view,
                );
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

        // Pre-pass: update debug lines and cap mesh (need mutable access before graph)
        if self.ui_state.params.debug.show_chunk_boundaries {
            let chunk_positions: Vec<IVec3> =
                self.world.chunks.iter().map(|c| c.position).collect();
            self.debug_line_pass.update(
                &self.gpu.device,
                &chunk_positions,
                &self.simulation.frustum,
            );
        }

        let cap_enabled = self.ui_state.params.cross_section.enabled
            && frame.clip_enabled != 0;
        if cap_enabled {
            let clip_dirs: [f32; 3] = [
                if frame.cos_r >= 0.0 { 1.0 } else { -1.0 },
                1.0,
                if frame.sin_r >= 0.0 { 1.0 } else { -1.0 },
            ];
            let clip_pos = [
                if clip_dirs[0] > 0.0 { frame.clip_max[0] } else { frame.clip_min[0] },
                frame.clip_max[1],
                if clip_dirs[2] > 0.0 { frame.clip_max[2] } else { frame.clip_min[2] },
            ];
            self.cap_pass.maybe_rebuild(
                &self.gpu.device,
                &self.world,
                &self.ui_state.params.cross_section,
                clip_pos,
                clip_dirs,
            );
        }

        // Build transient pass nodes
        let terrain_pipeline = if self.ui_state.params.debug.show_wireframe {
            &self.pipeline_registry.terrain_wireframe_pipeline
        } else {
            &self.pipeline_registry.terrain_pipeline
        };

        let shadow_node = ShadowPassNode {
            pipeline: &self.pipeline_registry.shadow_pipeline,
            bind_group: &self.shadow_bind_group,
            shadow_depth_view: &self.shadow_depth_view,
            chunks: &self.world.chunks,
        };

        let sky = self.ui_state.params.render_pipeline.sky_color;
        let scene_node = MainScenePassNode {
            sky_color: wgpu::Color {
                r: sky[0] as f64,
                g: sky[1] as f64,
                b: sky[2] as f64,
                a: 1.0,
            },
            terrain_pipeline,
            uniform_bind_group: &self.uniform_bind_group,
            chunks: &self.world.chunks,
            frustum: &self.simulation.frustum,
            cap_pass: &self.cap_pass,
            cap_config: CapConfig {
                enabled: cap_enabled,
                clip_dirs: [
                    if frame.cos_r >= 0.0 { 1.0 } else { -1.0 },
                    1.0,
                    if frame.sin_r >= 0.0 { 1.0 } else { -1.0 },
                ],
                clip_pos: [0.0; 3], // not needed — cap_pass already rebuilt above
            },
            vegetation_pass: &self.vegetation_pass,
            vegetation_pipeline: &self.pipeline_registry.vegetation_pipeline,
            water_pass: &self.water_pass,
            water_pipeline: &self.pipeline_registry.water_pipeline,
            debug_line_pass: &self.debug_line_pass,
            show_debug_lines: self.ui_state.params.debug.show_chunk_boundaries,
        };

        // Build render graph
        let mut graph = RenderGraph::new();
        graph.add_pass(&shadow_node);
        graph.add_pass(&scene_node);
        graph.add_pass(&self.outline_pass);
        graph.add_pass(&self.post_process);
        graph.add_pass(&self.palette_pass);
        graph.add_pass(&self.upscale_pass);

        #[cfg(debug_assertions)]
        graph
            .validate(&[
                ResourceId::SHADOW_DEPTH,
                ResourceId::SCENE,
                ResourceId::NORMAL,
                ResourceId::DEPTH,
                ResourceId::PROCESSED,
                ResourceId::SURFACE,
            ])
            .unwrap();

        // Resource map
        let mut resources = ResourceMap::new();
        resources.insert(ResourceId::SCENE, &self.render_targets.scene_view);
        resources.insert(ResourceId::NORMAL, &self.render_targets.normal_view);
        resources.insert(ResourceId::DEPTH, &self.render_targets.depth_view);
        resources.insert(ResourceId::PROCESSED, &self.render_targets.processed_view);
        resources.insert(ResourceId::SHADOW_DEPTH, &self.shadow_depth_view);
        resources.insert(ResourceId::SURFACE, &surface_view);

        // Execute graph
        let mut encoder = self.gpu.device.create_command_encoder(
            &wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            },
        );
        graph.execute(&mut encoder, &resources);

        // Egui (outside graph - needs &mut self for tessellation)
        self.egui_renderer.draw(
            &mut self.ui_state,
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &surface_view,
            &self.window,
        );

        // Submit + present
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        surface_frame.present();

        // World regeneration
        self.world_regen.tick(
            &mut self.world,
            &mut self.meshing,
            &mut self.vegetation_pass,
            &mut self.water_pass,
            &mut self.static_water,
            &mut self.ui_state,
            &self.gpu.device,
        );
    }
}

fn compute_render_dimensions(
    window_width: u32,
    window_height: u32,
    zoom: f32,
    target_voxel_pixels: f32,
) -> (u32, u32, f32) {
    let wh = window_height.max(1) as f32;
    let ww = window_width.max(1) as f32;
    // Each voxel covers (wh / 2*zoom) screen pixels.
    // We want that to equal target_voxel_pixels, so:
    //   pixel_scale = (wh / (2*zoom)) / target_voxel_pixels
    //
    // Round to the nearest integer so every render texel maps to exactly N
    // window pixels. A noninteger scale causes some texels to cover 1 pixel
    // and others 2 (or N and N+1 at higher scales), and the assignment shifts
    // as the sub-pixel offset changes during camera movement — producing the
    // "2 2 1 2 → 1 2 2 2" pattern-change artifact.  Integer pixel_scale
    // guarantees a uniform upscale grid that is invariant to the sub-pixel
    // offset, so patterns never change between frames.
    let ideal = (wh / (2.0 * zoom) / target_voxel_pixels).max(1.0);
    let pixel_scale = ideal.round().max(1.0);
    let render_w = ((ww / pixel_scale).floor() as u32).max(1);
    let render_h = ((wh / pixel_scale).floor() as u32).max(1);
    (render_w, render_h, pixel_scale)
}

struct App {
    state: Option<AppState>,
}

impl App {
    fn new() -> Self {
        Self { state: None }
    }
}

impl ApplicationHandler for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        let attrs = WindowAttributes::default()
            .with_title("Voxulacrum")
            .with_inner_size(winit::dpi::PhysicalSize::new(1280u32, 720u32));

        let window = Arc::new(
            event_loop.create_window(attrs).expect("Failed to create window"),
        );

        self.state = Some(AppState::new(window));
        log::info!("Window and GPU initialized.");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else { return; };

        // F1 toggle -- always handled
        if let WindowEvent::KeyboardInput { ref event, .. } = event {
            if event.state == ElementState::Pressed {
                if let PhysicalKey::Code(KeyCode::F1) = event.physical_key {
                    state.egui_renderer.toggle_visibility();
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                state.gpu.resize(new_size.width, new_size.height);
                state.simulation.camera.resize(new_size.width, new_size.height);
                let (rw, rh, eff_scale) = compute_render_dimensions(
                    new_size.width, new_size.height,
                    state.simulation.camera.zoom, state.ui_state.params.render_pipeline.world_pixel_density,
                );
                state.render_targets = RenderTargets::new(
                    &state.gpu.device, rw, rh, eff_scale, state.gpu.surface_format,
                );
                state.outline_pass.rebuild_bind_group(
                    &state.gpu.device,
                    &state.render_targets.scene_view,
                    &state.render_targets.depth_view,
                    &state.render_targets.normal_view,
                );
                state.post_process.rebuild_bind_group(
                    &state.gpu.device,
                    &state.render_targets.processed_view,
                    &state.render_targets.depth_view,
                );
                state.palette_pass.rebuild_bind_group(
                    &state.gpu.device,
                    &state.render_targets.scene_view
                );
                state.upscale_pass.rebuild_bind_group(
                    &state.gpu.device,
                    &state.render_targets.processed_view,
                );
            }
            ref e @ WindowEvent::KeyboardInput { ref event, .. } => {
                let consumed = state.egui_renderer.handle_event(&state.window, e);
                if !consumed {
                    state.simulation.camera.process_keyboard(event.physical_key, event.state);
                }
            }
            ref e @ WindowEvent::MouseWheel { ref delta, .. } => {
                let consumed = state.egui_renderer.handle_event(&state.window, e);
                if !consumed {
                    state.simulation.camera.process_scroll(delta, &state.ui_state.params.camera);
                }
            }
            WindowEvent::RedrawRequested => {
                state.render();
            }
            ref other => {
                // Forward cursor moves, etc. to egui
                state.egui_renderer.handle_event(&state.window, other);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn main() {
    env_logger::init();
    log::info!("Voxulacrum engine starting...");

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("Event loop failed");
}