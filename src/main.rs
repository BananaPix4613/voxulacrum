mod camera;
mod cloud_shadow;
mod meshing;
mod params;
mod rendering;
mod shader_reload;
mod simulation;
mod ui;
mod world;

use std::path::PathBuf;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use camera::IsometricCamera;
use cloud_shadow::CloudShadowState;
use params::{EngineParams, ParamChangeKind};
use rendering::gpu_state::GpuState;
use rendering::pipelines::{self, PipelineRegistry, PipelineResources};
use rendering::uniforms::{self, GlobalUniforms, PostProcessUniforms, ShadowUniforms};
use rendering::vegetation_pass::VegetationPass;
use rendering::water_pass::WaterPass;
use rendering::frustum::Frustum;
use rendering::post_process::PostProcessPass;
use shader_reload::ShaderWatcher;
use simulation::water::StaticWater;
use simulation::time_of_day::TimeOfDay;
use simulation::wind::WindState;
use ui::panels::UiState;
use world::generation::{WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z};

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
    vegetation_pass: VegetationPass,
    static_water: StaticWater,
    water_pass: WaterPass,
    post_process: PostProcessPass,
    egui_renderer: ui::EguiRenderer,
    ui_state: UiState,
    frame_counter: FrameCounter,
    shader_watcher: ShaderWatcher,
    shader_dir: PathBuf,
    meshing_pipeline: MeshingPipeline,
    frustum: Frustum,
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

        // Generate world using params
        let gen_start = std::time::Instant::now();
        let mut world = world::World::generate(&initial_params.terrain_gen);
        log::info!("World generated in {:.2?}", gen_start.elapsed());
        world.print_debug_stats();

        let mut meshing_pipeline = MeshingPipeline::new(
            WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z,
        );
        meshing_pipeline.submit_all_dirty(&world);
        log::info!("Submitted {} chunks are async meshing", world.chunks.len());

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
            &gpu.device, gpu.surface_format, &gpu.scene_view, &pp_shader_source,
        );

        // Egui
        let egui_renderer = ui::EguiRenderer::new(&gpu.device, gpu.surface_format, &window);
        let ui_state = UiState::new(initial_params, presets_dir);

        let vp = camera.projection_matrix() * camera.view_matrix();
        let frustum = Frustum::from_view_projection(vp);

        Self {
            window,
            gpu,
            camera,
            pipeline_registry,
            pipeline_resources,
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
            vegetation_pass,
            static_water,
            water_pass,
            post_process,
            egui_renderer,
            ui_state,
            frame_counter: FrameCounter::new(),
            shader_watcher,
            shader_dir,
            meshing_pipeline,
            frustum,
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

        // Clear boundary maps when pipeline finishes
        if self.meshing_pipeline.is_idle() {
            self.meshing_pipeline.clear_boundary_maps();
        }

        // Apply params to systems
        self.camera.apply_params(&self.ui_state.params.camera);
        self.camera.update(dt);
        self.time_of_day.update(dt, &self.ui_state.params.time_control);
        self.wind.update(dt, &self.ui_state.params.wind);
        self.cloud_shadow.update(dt, &self.wind.wind_vector, self.elapsed, &self.ui_state.params.cloud);

        // Update frustum for culling (frozen when freeze_culling is enabled)
        if !self.ui_state.params.debug.freeze_culling {
            let vp = self.camera.projection_matrix() * self.camera.view_matrix();
            self.frustum = Frustum::from_view_projection(vp);
        }

        // Light-space matrix
        let light_space = self.time_of_day.light_space_matrix();

        // Shadow uniforms
        let shadow_uniforms = ShadowUniforms {
            light_space_matrix: light_space.to_cols_array_2d(),
        };
        self.gpu.queue.write_buffer(
            &self.shadow_uniform_buffer, 0, bytemuck::cast_slice(&[shadow_uniforms]),
        );

        // Main uniforms
        let sun_dir = self.time_of_day.sun_direction();
        let sun_color = self.time_of_day.sun_color(&self.ui_state.params.lighting);
        let ambient_color = self.time_of_day.ambient_color(&self.ui_state.params.lighting);

        let uniforms = GlobalUniforms {
            view_proj: self.camera.view_projection(),
            light_space_matrix: light_space.to_cols_array_2d(),
            sun_direction: sun_dir.into(),
            _pad0: 0.0,
            sun_color: sun_color.into(),
            _pad1: 0.0,
            ambient_color: ambient_color.into(),
            _pad2: 0.0,
            wind_vector: self.wind.wind_vector.into(),
            time: self.elapsed,
            _pad_time: 0.0,
            cloud_shadow_offset: self.cloud_shadow.offset.into(),
            cloud_coverage: self.cloud_shadow.coverage,
            _pad3: 0.0,
        };
        self.gpu.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        // Post-process uniforms from params
        let pp = &self.ui_state.params.post_process;
        let (tint_color, tint_strength) = self.time_of_day.warm_tint();
        let pp_uniforms = PostProcessUniforms {
            warm_tint: tint_color.into(),
            warm_tint_strength: tint_strength,
            desaturation: self.cloud_shadow.coverage * pp.overcast_desaturation_factor,
            vignette_strength: pp.vignette_strength,
            exposure: pp.exposure,
            _pad: 0.0,
        };
        self.post_process.update_uniforms(&self.gpu.queue, pp_uniforms);

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
                self.post_process.rebuild_bind_group(&self.gpu.device, &self.gpu.scene_view);
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
        let sky = ambient_color;
        let sky_brightness = sky.length() / 0.8_f32.sqrt();
        let sky_r = (0.5 * sky_brightness).clamp(0.02, 0.5) as f64;
        let sky_g = (0.65 * sky_brightness).clamp(0.02, 0.65) as f64;
        let sky_b = (0.8 * sky_brightness).clamp(0.05, 0.8) as f64;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.gpu.scene_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: sky_r, g: sky_g, b: sky_b, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.gpu.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.pipeline_registry.terrain_pipeline);
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
        }

        // === Post-processing pass ===
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post_process_pass"),
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
            pass.set_pipeline(&self.post_process.pipeline);
            pass.set_bind_group(0, &self.post_process.bind_group, &[]);
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

        // Detect and log parameter changes
        let change = self.ui_state.change_detector.detect(&self.ui_state.params);
        match change {
            ParamChangeKind::RegenerationRequired => {
                log::info!("Terrain params changed -- regeneration needed (Phase 6)");
            }
            ParamChangeKind::MeshInvalidating => {
                log::info!("Mesh-invalidating param changed -- resubmitting all chunks");
                for chunk in &mut self.world.chunks {
                    chunk.mesh_dirty = true;
                }
                self.meshing_pipeline.submit_all_dirty(&self.world);
            }
            _ => {}
        }
        self.ui_state.change_detector.snapshot(&self.ui_state.params);
    }
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
                state.post_process.rebuild_bind_group(&state.gpu.device, &state.gpu.scene_view);
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