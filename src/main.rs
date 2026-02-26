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

use camera::IsometricCamera;
use cloud_shadow::CloudShadowState;
use params::{EngineParams};
use rendering::gpu_state::GpuState;
use rendering::pipelines::{PipelineRegistry, PipelineResources};
use rendering::render_targets::RenderTargets;
use rendering::uniforms::{self, GlobalUniforms, PostProcessUniforms, ShadowUniforms};
use rendering::debug_lines::DebugLinePass;
use rendering::upscale_pass::{UpscalePass, UpscaleUniforms};
use rendering::vegetation_pass::VegetationPass;
use rendering::water_pass::WaterPass;
use rendering::frustum::Frustum;
use rendering::post_process::PostProcessPass;
use rendering::outline_pass::OutlinePass;
use rendering::uniforms::OutlineUniforms;
use rendering::palette_pass::PalettePass;
use rendering::cap_pass::CapPass;
use palette::Palette;
use shader_reload::ShaderWatcher;
use simulation::water::StaticWater;
use simulation::time_of_day::TimeOfDay;
use simulation::wind::WindState;
use ui::panels::UiState;
use world::WorldManager;
use world::chunk::VOXEL_SCALE;

use crate::meshing::MeshingPipeline;

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
    camera: IsometricCamera,
    pipeline_registry: PipelineRegistry,
    pipeline_resources: PipelineResources,
    render_targets: RenderTargets,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    shadow_uniform_buffer: wgpu::Buffer,
    shadow_bind_group: wgpu::BindGroup,
    shadow_depth_view: wgpu::TextureView,
    last_frame: std::time::Instant,
    world: world::World,
    time_of_day: TimeOfDay,
    wind: WindState,
    cloud_shadow: CloudShadowState,
    elapsed: f32,
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
    meshing_pipeline: MeshingPipeline,
    world_manager: WorldManager,
    frustum: Frustum,
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
            size: std::mem::size_of::<GlobalUniforms>() as u64,
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

        let shadow_bind_group_layout = uniforms::create_shadow_bind_group_layout(&gpu.device);
        let shadow_uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_uniform_buffer"),
            size: std::mem::size_of::<ShadowUniforms>() as u64,
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
        let cache_dir = std::path::PathBuf::from("cache/meshes");
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

        let vp = camera.projection_matrix() * camera.view_matrix();
        let frustum = Frustum::from_view_projection(vp);

        let mut meshing_pipeline = MeshingPipeline::new(
            world.chunks_x, world.chunks_y, world.chunks_z,
            &ui_state.params.materials, &ui_state.params.meshing,
        );
        meshing_pipeline.submit_all_dirty(&world);
        log::info!("Submitted {} chunks are async meshing", world.chunks.len());

        Self {
            window,
            gpu,
            camera,
            pipeline_registry,
            pipeline_resources,
            render_targets,
            uniform_buffer,
            uniform_bind_group,
            shadow_uniform_buffer,
            shadow_bind_group,
            shadow_depth_view,
            last_frame: std::time::Instant::now(),
            world,
            time_of_day: TimeOfDay::new(),
            wind: WindState::new(),
            cloud_shadow,
            elapsed: 0.0,
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
            meshing_pipeline,
            world_manager: WorldManager::new(),
            frustum,
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
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.elapsed += dt;

        // Frame stats
        self.frame_counter.tick();
        self.ui_state.fps = self.frame_counter.fps;
        self.ui_state.frame_time_ms = dt * 1000.0;

        // Check for shader hot-reloads before rendering
        self.check_shader_hot_reload();

        // Detect pixel_scale changes requiring render target recreation
        let (rw, rh, eff_scale) = compute_render_dimensions(
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            self.camera.zoom,
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

        // Drain pending snapshot submissions (bounded per frame)
        self.meshing_pipeline.drain_pending_submissions(&self.world);

        // Poll meshing pipeline for completed chunks
        let completed = self.meshing_pipeline.poll();
        for result in completed {
            self.world.upload_mesh_result(
                result.chunk_index,
                &result.vertices,
                &result.indices,
                &self.gpu.device,
            );
        }

        // Update meshing stats for UI
        self.ui_state.meshing_stats = self.meshing_pipeline.stats.clone();
        self.ui_state.remeshing = !self.meshing_pipeline.is_idle();

        // Handle cache clear request from UI
        if self.ui_state.clear_cache_requested {
            self.ui_state.clear_cache_requested = false;
            self.meshing_pipeline.clear_cache();
            log::info!("Cache cleared by user");
        }

        // Handle palette load request from UI
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

        // Clear boundary maps when pipeline finishes
        if self.meshing_pipeline.is_idle() {
            self.meshing_pipeline.clear_boundary_maps();
        }

        // Apply params to systems
        self.camera.apply_params(&self.ui_state.params.camera);
        self.camera.update(dt);

        // Camera snapping
        let snapped = if self.ui_state.params.render_pipeline.camera_snap_enabled {
            self.camera.snap_camera(
                self.render_targets.render_width,
                self.render_targets.render_height,
                self.render_targets.effective_pixel_scale,
            )
        } else {
            camera::SnappedCamera {
                view_proj: self.camera.view_projection(),
                subpixel_offset: [0.0; 2],
            }
        };

        self.time_of_day.update(dt, &self.ui_state.params.time_control);
        self.wind.update(dt, &self.ui_state.params.wind);
        self.cloud_shadow.update(dt, &self.wind.wind_vector, self.elapsed, &self.ui_state.params.cloud);

        // Detect material parameter changes — flag as pending but do NOT auto-remesh
        let change_kind = self.ui_state.change_detector.detect(&self.ui_state.params);
        if change_kind == params::ParamChangeKind::MeshInvalidating {
            self.ui_state.mesh_params_pending = true;
        }
        self.ui_state.change_detector.snapshot(&self.ui_state.params);

        // Handle explicit "Remesh" button press
        if self.ui_state.remesh_requested {
            self.ui_state.remesh_requested = false;
            self.ui_state.mesh_params_pending = false;
            log::info!("Remesh requested by user");
            self.meshing_pipeline.update_material_config(&self.ui_state.params.materials, &self.ui_state.params.meshing);
            self.meshing_pipeline.reset_for_new_world();
            for chunk in &mut self.world.chunks {
                chunk.mesh_dirty = true;
            }
            self.meshing_pipeline.submit_all_dirty(&self.world);
        }

        // Update frustum for culling (frozen when freeze_culling is enabled)
        if !self.ui_state.params.debug.freeze_culling {
            let vp = self.camera.projection_matrix() * self.camera.view_matrix();
            self.frustum = Frustum::from_view_projection(vp);
        }

        // Light-space matrix
        let light_space = self.time_of_day.light_space_matrix();

        // Cross-section clip bounds (camera-direction-aware)
        // Use target_rotation (discrete 90° steps) so clip side flips instantly on Q/E.
        let cos_r = self.camera.target_rotation.cos();
        let sin_r = self.camera.target_rotation.sin();

        let (clip_min, clip_max, clip_enabled) = if self.ui_state.params.cross_section.enabled {
            let cs = &self.ui_state.params.cross_section;
            let cam = self.camera.smooth_target;
            let half = self.ui_state.params.camera.zoom_max;

            // X axis: clip direction follows camera facing
            let (clip_min_x, clip_max_x) = if cs.x_offset > 0.0 {
                if cos_r >= 0.0 {
                    let raw = cam.x + half - cs.x_offset;
                    (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
                } else {
                    let raw = cam.x - half + cs.x_offset;
                    ((raw / VOXEL_SCALE).ceil() * VOXEL_SCALE, 10000.0)
                }
            } else { (-10000.0, 10000.0) };

            // Y axis: always clip from above
            let (clip_min_y, clip_max_y) = if cs.y_offset > 0.0 {
                let raw = cam.y + half - cs.y_offset;
                (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
            } else { (-10000.0, 10000.0) };

            // Z axis: clip direction follows camera facing
            let (clip_min_z, clip_max_z) = if cs.z_offset > 0.0 {
                if sin_r >= 0.0 {
                    let raw = cam.z + half - cs.z_offset;
                    (-10000.0, (raw / VOXEL_SCALE).floor() * VOXEL_SCALE)
                } else {
                    let raw = cam.z - half + cs.z_offset;
                    ((raw / VOXEL_SCALE).ceil() * VOXEL_SCALE, 10000.0)
                }
            } else { (-10000.0, 10000.0) };

            (
                [clip_min_x, clip_min_y, clip_min_z],
                [clip_max_x, clip_max_y, clip_max_z],
                1u32,
            )
        } else {
            ([-10000.0_f32; 3], [10000.0_f32; 3], 0u32)
        };

        // Shadow uniforms
        let shadow_uniforms = ShadowUniforms {
            light_space_matrix: light_space.to_cols_array_2d(),
            clip_min,
            clip_enabled,
            clip_max,
            _pad: 0.0,
        };
        self.gpu.queue.write_buffer(
            &self.shadow_uniform_buffer, 0, bytemuck::cast_slice(&[shadow_uniforms]),
        );

        // Main uniforms
        let sun_dir = self.time_of_day.sun_direction();
        let sun_color = self.time_of_day.sun_color(&self.ui_state.params.lighting);
        let ambient_color = self.time_of_day.ambient_color(&self.ui_state.params.lighting);

        // Compute debug_mode from DebugParams flags
        let debug_mode: u32 = if self.ui_state.params.debug.show_material_ids {
            1 // Material ID view
        } else if self.ui_state.params.debug.show_ao_only {
            2 // AO-only view
        } else if self.ui_state.params.debug.show_normals {
            3 // Normal view
        } else if self.ui_state.params.debug.show_greedy_debug {
            4 // Greedy view
        } else {
            0 // Normal rendering (wireframe is handled by pipeline swap, not shader)
        };

        let uniforms = GlobalUniforms {
            view_proj: snapped.view_proj,
            light_space_matrix: light_space.to_cols_array_2d(),
            sun_direction: sun_dir.into(),
            _pad0: 0.0,
            sun_color: sun_color.into(),
            _pad1: 0.0,
            ambient_color: ambient_color.into(),
            _pad2: 0.0,
            wind_vector: self.wind.wind_vector.into(),
            time: self.elapsed,
            debug_mode,
            cloud_shadow_offset: self.cloud_shadow.offset.into(),
            cloud_coverage: self.cloud_shadow.coverage,
            edge_strength: self.ui_state.params.meshing.edge_strength,
            ortho_ao_strength: if self.ui_state.params.meshing.ortho_ao_enabled {
                self.ui_state.params.meshing.ortho_ao_strength
            } else {
                0.0
            },
            clip_enabled,
            _pad_a: [0.0; 2],
            clip_min,
            _pad3: 0.0,
            clip_max,
            _pad4: 0.0,
        };
        self.gpu.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        // Post-process uniforms from params
        let pp = &self.ui_state.params.post_process;
        let cs = &self.ui_state.params.cross_section;
        let (tint_color, tint_strength) = self.time_of_day.warm_tint();

        // Inverse view-projection for world-position reconstruction in post-process fog
        let vp_mat = glam::Mat4::from_cols_array_2d(&snapped.view_proj);
        let inv_vp = vp_mat.inverse().to_cols_array_2d();

        let clip_fog_enabled: u32 = if cs.enabled {
            if cs.show_edges { 2 } else { 1 }
        } else {
            0
        };

        let pp_uniforms = PostProcessUniforms {
            warm_tint: tint_color.into(),
            warm_tint_strength: tint_strength,
            desaturation: self.cloud_shadow.coverage * pp.overcast_desaturation_factor,
            vignette_strength: pp.vignette_strength,
            exposure: pp.exposure,
            clip_fog_enabled,
            fog_color: cs.fog_color,
            fog_density: cs.fog_density,
            clip_max,
            _pad0: 0.0,
            clip_min,
            _pad1: 0.0,
            inv_view_proj: inv_vp,
            render_resolution: [
                self.render_targets.render_width as f32,
                self.render_targets.render_height as f32,
            ],
            _pad2: [0.0; 2],
        };
        self.post_process.update_uniforms(&self.gpu.queue, pp_uniforms);

        // Outline uniforms
        let op = &self.ui_state.params.outline;
        self.outline_pass.update_uniforms(&self.gpu.queue, OutlineUniforms {
            texel_size: [
                1.0 / self.render_targets.tex_width as f32,
                1.0 / self.render_targets.tex_height as f32,
            ],
            depth_threshold: op.depth_threshold,
            depth_strength: op.depth_strength,
            normal_threshold: op.normal_threshold,
            normal_strength: op.normal_strength,
            darken_strength: op.darken_strength,
            brighten_strength: op.brighten_strength,
            enabled: if op.enabled { 1 } else { 0 },
            _pad: [0; 3],
        });

        // Palette uniforms
        let palette_uniforms = if self.ui_state.params.palette.mode == 0 {
            // Palette lookup mode
            if let Some(ref pal) = self.loaded_palette {
                palette::palette_to_uniforms(pal, &self.ui_state.params.palette)
            } else {
                palette::stepping_uniforms(&self.ui_state.params.palette)
            }
        } else {
            // Color stepping mode (no palette data needed)
            palette::stepping_uniforms(&self.ui_state.params.palette)
        };
        self.palette_pass.update_uniforms(&self.gpu.queue, palette_uniforms);

        // Upscaling uniforms
        let upscale_uniforms = UpscaleUniforms {
            subpixel_offset: snapped.subpixel_offset,
            render_resolution: [
                self.render_targets.render_width as f32,
                self.render_targets.render_height as f32,
            ],
            window_resolution: [
                self.gpu.surface_config.width as f32,
                self.gpu.surface_config.height as f32,
            ],
            tex_resolution: [
                self.render_targets.tex_width as f32,
                self.render_targets.tex_height as f32,
            ],
        };
        self.upscale_pass.update_uniforms(&self.gpu.queue, upscale_uniforms);

        // Performance stats (count only visible chunks)
        let mut total_tris: u64 = 0;
        let mut chunks_visible: u32 = 0;
        let mut chunks_total: u32 = 0;
        for chunk in &self.world.chunks {
            if chunk.mesh.is_some() {
                chunks_total += 1;
                if self.frustum.is_chunk_visible(chunk.position) {
                    chunks_visible += 1;
                    total_tris += chunk.mesh.as_ref().unwrap().index_count as u64 / 3;
                }
            }
        }
        self.ui_state.total_triangles = total_tris;
        self.ui_state.chunks_visible = chunks_visible;
        self.ui_state.chunks_total = chunks_total;

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = self.window.inner_size();
                self.gpu.resize(size.width, size.height);
                let (rw, rh, eff_scale) = compute_render_dimensions(
                    size.width, size.height,
                    self.camera.zoom, self.ui_state.params.render_pipeline.world_pixel_density,
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
                    &self.render_targets.scene_view,
                );
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => { log::warn!("Surface timeout"); return; }
            Err(e) => { log::error!("Surface error: {:?}", e); return; }
        };

        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render_encoder"),
        });

        // === Shadow pass ===
        {
            let mut shadow_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow_pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            shadow_pass.set_pipeline(&self.pipeline_registry.shadow_pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            for chunk in &self.world.chunks {
                if let Some(mesh) = &chunk.mesh {
                    shadow_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    shadow_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    shadow_pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
        }

        // === Main pass ===
        let sky_cfg = self.ui_state.params.render_pipeline.sky_color;
        let sky_r = sky_cfg[0] as f64;
        let sky_g = sky_cfg[1] as f64;
        let sky_b = sky_cfg[2] as f64;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.render_targets.scene_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: sky_r, g: sky_g, b: sky_b, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.render_targets.normal_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.render_targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Select terrain pipeline based on wireframe debug flag
            let terrain_pipeline = if self.ui_state.params.debug.show_wireframe {
                &self.pipeline_registry.terrain_wireframe_pipeline
            } else {
                &self.pipeline_registry.terrain_pipeline
            };

            pass.set_pipeline(terrain_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            for chunk in &self.world.chunks {
                if let Some(mesh) = &chunk.mesh {
                    if self.frustum.is_chunk_visible(chunk.position) {
                        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                }
            }

            // Cross-section cap mesh: solid fill at clip plane boundaries
            if self.ui_state.params.cross_section.enabled && clip_enabled != 0 {
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
                self.cap_pass.maybe_rebuild(
                    &self.gpu.device,
                    &self.world,
                    &self.ui_state.params.cross_section,
                    clip_pos,
                    clip_dirs,
                );
                if self.cap_pass.index_count > 0 {
                    pass.set_pipeline(&self.cap_pass.pipeline);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.cap_pass.vertex_buffer.as_ref().unwrap().slice(..));
                    pass.set_index_buffer(
                        self.cap_pass.index_buffer.as_ref().unwrap().slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..self.cap_pass.index_count, 0, 0..1);
                }
            }

            if self.vegetation_pass.instance_count > 0 {
                pass.set_pipeline(&self.pipeline_registry.vegetation_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.vegetation_pass.grass_vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, self.vegetation_pass.instance_buffer.slice(..));
                pass.set_index_buffer(self.vegetation_pass.grass_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.vegetation_pass.grass_index_count, 0, 0..self.vegetation_pass.instance_count);
            }

            if self.water_pass.index_count > 0 {
                pass.set_pipeline(&self.pipeline_registry.water_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.water_pass.vertex_buffer.slice(..));
                pass.set_index_buffer(self.water_pass.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.water_pass.index_count, 0, 0..1);
            }

            // Debug: chunk boundary lines
            if self.ui_state.params.debug.show_chunk_boundaries {
                let chunk_positions: Vec<IVec3> = self.world.chunks.iter()
                    .map(|c| c.position)
                    .collect();
                self.debug_line_pass.update(
                    &self.gpu.device,
                    &chunk_positions,
                    &self.frustum,
                );

                if let Some(ref vb) = self.debug_line_pass.vertex_buffer {
                    pass.set_pipeline(&self.debug_line_pass.pipeline);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..self.debug_line_pass.vertex_count, 0..1);
                }
            }
        }

        // === Outline pass ===
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_targets.processed_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.outline_pass.pipeline);
            pass.set_bind_group(0, &self.outline_pass.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // === Post-processing pass ===
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post_process_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_targets.scene_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.post_process.pipeline);
            pass.set_bind_group(0, &self.post_process.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // === Palette quantization pass ===
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("palette_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_targets.processed_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.palette_pass.pipeline);
            pass.set_bind_group(0, &self.palette_pass.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // === Upscale blit pass ===
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upscale_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.upscale_pass.pipeline);
            pass.set_bind_group(0, &self.upscale_pass.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // === Egui pass ===
        self.egui_renderer.draw(
            &mut self.ui_state,
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &surface_view,
            &self.window,
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // ================================================================
        // World regeneration management
        // ================================================================

        // Feed regeneration status to UI
        self.ui_state.regenerating = self.world_manager.is_regenerating();
        self.ui_state.regen_progress = self.world_manager.progress();

        // Poll for completed background regeneration
        if let Some((new_chunks, regen_params)) = self.world_manager.poll_regeneration() {
            // Swap new voxel data into the world
            self.world.chunks = new_chunks;
            self.world.generator =
                world::generation::TerrainGenerator::new(&regen_params);

            // Reset meshing pipeline (drains stale in-flight results)
            self.meshing_pipeline.reset_for_new_world();

            // Mark all new chunks dirty and submit for meshing
            for chunk in &mut self.world.chunks {
                chunk.mesh_dirty = true;
            }
            self.meshing_pipeline.submit_all_dirty(&self.world);

            // Rebuild vegetation pass (scans flora voxels in new world)
            self.vegetation_pass = VegetationPass::new(
                &self.gpu.device,
                &self.world,
                &self.ui_state.params.vegetation,
            );
            log::info!("Vegetation pass rebuilt after regeneration");

            // Rebuild water passes (resamples terrain heights)
            self.static_water = StaticWater::new(
                &self.world,
                &self.ui_state.params.water,
                &regen_params,
            );
            self.water_pass = WaterPass::new(&self.gpu.device, &self.static_water);
            log::info!("Water passes rebuilt after regeneration");

            // Clear stale caches and save new world
            let cache_dir = std::path::PathBuf::from("cache/meshes");
            self.meshing_pipeline.clear_cache();
            let _ = meshing::cache::clear_world_cache(&cache_dir);

            let world_key =
                meshing::cache::compute_world_cache_key(&regen_params);
            let world_cache_path =
                meshing::cache::world_cache_path(&cache_dir, world_key);
            if let Err(e) = meshing::cache::save_world_cache(
                &world_cache_path,
                world_key,
                &self.world,
            ) {
                log::warn!("Failed to save regenerated world cache: {}", e);
            } else {
                log::info!("Regenerated world saved to cache");
            }

            // Update UI state
            self.ui_state.regenerating = false;

            // If the user changed terrain params *during* regeneration, the world
            // we just built is already stale. Immediately start another regen.
            if regen_params != self.ui_state.params.terrain_gen {
                log::info!("Terrain params changed during regeneration, restarting");
                self.world_manager
                    .start_regeneration(&self.ui_state.params.terrain_gen);
            }
        }

        // Handle explicit "Regenerate World" button press
        if self.ui_state.regenerate_requested {
            self.ui_state.regenerate_requested = false;
            if !self.world_manager.is_regenerating() {
                self.world_manager
                    .start_regeneration(&self.ui_state.params.terrain_gen);
            }
        }
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
                state.camera.resize(new_size.width, new_size.height);
                let (rw, rh, eff_scale) = compute_render_dimensions(
                    new_size.width, new_size.height,
                    state.camera.zoom, state.ui_state.params.render_pipeline.world_pixel_density,
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
                    &state.render_targets.scene_view,
                );
            }
            ref e @ WindowEvent::KeyboardInput { ref event, .. } => {
                let consumed = state.egui_renderer.handle_event(&state.window, e);
                if !consumed {
                    state.camera.process_keyboard(event.physical_key, event.state);
                }
            }
            ref e @ WindowEvent::MouseWheel { ref delta, .. } => {
                let consumed = state.egui_renderer.handle_event(&state.window, e);
                if !consumed {
                    state.camera.process_scroll(delta, &state.ui_state.params.camera);
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