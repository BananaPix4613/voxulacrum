pub mod panels;

use egui_wgpu::ScreenDescriptor;

use crate::rendering::render_context::RenderContext;

pub struct EguiRenderer {
    pub ctx: egui::Context,
    pub winit_state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
    pub visible: bool,
}

impl EguiRenderer {
    pub fn new(
        render_ctx: &RenderContext,
        window: &winit::window::Window,
    ) -> Self {
        let ctx = egui::Context::default();

        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(render_ctx.device.limits().max_texture_dimension_2d as usize),
        );

        let renderer = egui_wgpu::Renderer::new(
            &render_ctx.device,
            render_ctx.surface_format,
            None,
            1,
            false,
        );
        
        Self {
            ctx,
            winit_state,
            renderer,
            visible: true,
        }
    }
    
    /// Feed a winit WindowEvent to egui. Returns true if egui consumed the event.
    pub fn handle_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        self.winit_state.on_window_event(window, event).consumed
    }
    
    pub fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }
    
    /// Run the egui UI and render it onto the given surface view.
    /// The surface view should already contain the post-processed scene.
    pub fn draw(
        &mut self,
        ui_state: &mut panels::UiState,
        ctx: &RenderContext,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        window: &winit::window::Window,
    ) {
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [
                window.inner_size().width,
                window.inner_size().height,
            ],
            pixels_per_point: window.scale_factor() as f32,
        };
        
        let raw_input = self.winit_state.take_egui_input(window);
        let full_output = self.ctx.run(raw_input, |ctx| {
            if self.visible {
                panels::draw_engine_panel(ctx, ui_state);
            }
        });
        
        self.winit_state
            .handle_platform_output(window, full_output.platform_output);
        
        let tris = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(&ctx.device, &ctx.queue, *id, image_delta);
        }

        self.renderer
            .update_buffers(&ctx.device, &ctx.queue, encoder, &tris, &screen_descriptor);

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            
            self.renderer
                .render(&mut render_pass.forget_lifetime(), &tris, &screen_descriptor);
        }
        
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}