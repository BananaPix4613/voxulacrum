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
use rendering::gpu_state::GpuState;
use rendering::pipelines;
use rendering::terrain_pass::DebugCube;
use rendering::uniforms::{self, GlobalUniforms};

struct AppState {
    window: Arc<Window>,
    gpu: GpuState,
    camera: IsometricCamera,
    terrain_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    debug_cube: DebugCube,
    last_frame: std::time::Instant,
    world: world::World,
}

impl AppState {
    fn new(window: Arc<Window>) -> Self {
        let gpu = GpuState::new(window.clone());

        let mut camera = IsometricCamera::new();
        camera.resize(gpu.surface_config.width, gpu.surface_config.height);

        let bind_group_layout = uniforms::create_bind_group_layout(&gpu.device);

        let uniform_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("global_uniform_buffer"),
            size: std::mem::size_of::<GlobalUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group =
            uniforms::create_bind_group(&gpu.device, &bind_group_layout, &uniform_buffer);

        let terrain_pipeline = pipelines::create_terrain_pipeline(
            &gpu.device,
            gpu.surface_format,
            &bind_group_layout,
        );

        let debug_cube = DebugCube::new(&gpu.device);

        // Generate world
        let gen_start = std::time::Instant::now();
        let world = world::World::generate(12345);
        let gen_elapsed = gen_start.elapsed();
        log::info!("World generated in {:.2?}", gen_elapsed);
        world.print_debug_stats();
        
        Self {
            window,
            gpu,
            camera,
            terrain_pipeline,
            uniform_buffer,
            uniform_bind_group,
            debug_cube,
            last_frame: std::time::Instant::now(),
            world,
        }
    }

    fn render(&mut self) {
        let now = std::time::Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.camera.update(dt);

        // Update uniforms
        let mut uniforms = GlobalUniforms::default();
        uniforms.view_proj = self.camera.view_projection();
        let len = (0.5_f32 * 0.5 + 0.8 * 0.8 + 0.3 * 0.3).sqrt();
        uniforms.sun_direction = [0.5 / len, 0.8 / len, 0.3 / len];

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

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.5,
                            g: 0.65,
                            b: 0.8,
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
            pass.set_vertex_buffer(0, self.debug_cube.vertex_buffer.slice(..));
            pass.set_index_buffer(
                self.debug_cube.index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..self.debug_cube.index_count, 0, 0..1);
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