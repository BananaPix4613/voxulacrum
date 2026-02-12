mod camera;
mod cloud_shadow;
mod meshing;
mod rendering;
mod simulation;
mod world;

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use camera::IsometricCamera;
use cloud_shadow::CloudShadowState;
use rendering::gpu_state::GpuState;
use rendering::pipelines;
use rendering::uniforms::{self, GlobalUniforms, ShadowUniforms};
use rendering::vegetation_pass::VegetationPass;
use rendering::water_pass::WaterPass;
use simulation::water::StaticWater;
use simulation::time_of_day::TimeOfDay;
use simulation::wind::WindState;

const SHADOW_MAP_SIZE: u32 = 2048;

struct AppState {
    window: Arc<Window>,
    gpu: GpuState,
    camera: IsometricCamera,
    terrain_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
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
    vegetation_pipeline: wgpu::RenderPipeline,
    vegetation_pass: VegetationPass,
    water_pipeline: wgpu::RenderPipeline,
    static_water: StaticWater,
    water_pass: WaterPass,
}

impl AppState {
    fn new(window: Arc<Window>) -> Self {
        let gpu = GpuState::new(window.clone());

        let mut camera = IsometricCamera::new();
        camera.resize(gpu.surface_config.width, gpu.surface_config.height);

        // Create shadow map depth texture
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_depth_view =
            shadow_depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Shadow comparison sampler
        let shadow_sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow_comparison_sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = uniforms::create_bind_group_layout(&gpu.device);

        let uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("global_uniform_buffer"),
            size: std::mem::size_of::<GlobalUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cloud_shadow = CloudShadowState::new(&gpu.device, &gpu.queue);

        let uniform_bind_group = uniforms::create_bind_group(
            &gpu.device,
            &bind_group_layout,
            &uniform_buffer,
            &cloud_shadow.texture_view,
            &cloud_shadow.sampler,
            &shadow_depth_view,
            &shadow_sampler,
        );

        let terrain_pipeline = pipelines::create_terrain_pipeline(
            &gpu.device,
            gpu.surface_format,
            &bind_group_layout,
        );

        // Shadow pass resources
        let shadow_bind_group_layout =
            uniforms::create_shadow_bind_group_layout(&gpu.device);
        let shadow_uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_uniform_buffer"),
            size: std::mem::size_of::<ShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_bind_group = uniforms::create_shadow_bind_group(
            &gpu.device,
            &shadow_bind_group_layout,
            &shadow_uniform_buffer,
        );
        let shadow_pipeline =
            pipelines::create_shadow_pipeline(&gpu.device, &shadow_bind_group_layout);

        // Generate world
        let gen_start = std::time::Instant::now();
        let mut world = world::World::generate(54321);
        let gen_elapsed = gen_start.elapsed();
        log::info!("World generated in {:.2?}", gen_elapsed);
        world.print_debug_stats();

        // Mesh all chunks
        let mesh_start = std::time::Instant::now();
        world.mesh_all_chunks(&gpu.device);
        let mesh_elapsed = mesh_start.elapsed();
        log::info!("World meshed in {:.2?}", mesh_elapsed);

        // Vegetation
        let vegetation_pipeline = pipelines::create_vegetation_pipeline(
            &gpu.device,
            gpu.surface_format,
            &bind_group_layout,
        );
        let veg_start = std::time::Instant::now();
        let vegetation_pass = VegetationPass::new(&gpu.device, &world);
        log::info!("Vegetation pass in {:.2?}", veg_start.elapsed());

        // Water
        let water_pipeline = pipelines::create_water_pipeline(
            &gpu.device,
            gpu.surface_format,
            &bind_group_layout,
        );
        let static_water = StaticWater::new(&world);
        let water_pass = WaterPass::new(&gpu.device, &static_water);
        log::info!("Water simulation initialized");

        Self {
            window,
            gpu,
            camera,
            terrain_pipeline,
            shadow_pipeline,
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
            vegetation_pipeline,
            vegetation_pass,
            water_pipeline,
            static_water,
            water_pass,
        }
    }

    fn render(&mut self) {
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.elapsed += dt;

        self.camera.update(dt);
        self.time_of_day.update(dt);
        self.wind.update(dt);
        self.cloud_shadow
            .update(dt, &self.wind.wind_vector, self.elapsed);

        // Compute light-space matrix
        let light_space = self.time_of_day.light_space_matrix();

        // Update shadow uniforms
        let shadow_uniforms = ShadowUniforms {
            light_space_matrix: light_space.to_cols_array_2d(),
        };
        self.gpu.queue.write_buffer(
            &self.shadow_uniform_buffer,
            0,
            bytemuck::cast_slice(&[shadow_uniforms]),
        );

        // Update main uniforms
        let sun_dir = self.time_of_day.sun_direction();
        let sun_color = self.time_of_day.sun_color();
        let ambient_color = self.time_of_day.ambient_color();

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

        self.gpu
            .queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let frame = match self.gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                let size = self.window.inner_size();
                self.gpu.resize(size.width, size.height);
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

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
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

            shadow_pass.set_pipeline(&self.shadow_pipeline);
            shadow_pass.set_bind_group(0, &self.shadow_bind_group, &[]);

            for chunk in &self.world.chunks {
                if let Some(mesh) = &chunk.mesh {
                    shadow_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    shadow_pass.set_index_buffer(
                        mesh.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    shadow_pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
        }

        // === Main pass ===
        let sky = self.time_of_day.ambient_color();
        let sky_brightness = sky.length() / 0.8_f32.sqrt();
        let sky_r = (0.5 * sky_brightness).clamp(0.02, 0.5) as f64;
        let sky_g = (0.65 * sky_brightness).clamp(0.02, 0.65) as f64;
        let sky_b = (0.8 * sky_brightness).clamp(0.05, 0.8) as f64;

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: sky_r,
                            g: sky_g,
                            b: sky_b,
                            a: 1.0,
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

            pass.set_pipeline(&self.terrain_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);

            for chunk in &self.world.chunks {
                if let Some(mesh) = &chunk.mesh {
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        mesh.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }

            if self.vegetation_pass.instance_count > 0 {
                pass.set_pipeline(&self.vegetation_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.vegetation_pass.grass_vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, self.vegetation_pass.instance_buffer.slice(..));
                pass.set_index_buffer(
                    self.vegetation_pass.grass_index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(
                    0..self.vegetation_pass.grass_index_count,
                    0,
                    0..self.vegetation_pass.instance_count,
                );
            }

            if self.water_pass.index_count > 0 {
                pass.set_pipeline(&self.water_pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.water_pass.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    self.water_pass.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..self.water_pass.index_count, 0, 0..1);
            }
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
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
        if self.state.is_some() {
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

        self.state = Some(AppState::new(window));
        log::info!("Window and GPU initialized.");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                state.gpu.resize(new_size.width, new_size.height);
                state.camera.resize(new_size.width, new_size.height);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                state
                    .camera
                    .process_keyboard(event.physical_key, event.state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                state.camera.process_scroll(&delta);
            }
            WindowEvent::RedrawRequested => {
                state.render();
            }
            _ => {}
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