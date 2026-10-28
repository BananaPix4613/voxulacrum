//! Paints the egui frame laid out on the main side (`ui::egui_layout_system`)
//! over the finished scene. The render side owns `egui_wgpu::Renderer` and the
//! textures it holds; the main side owns the egui context and the panels.

use bevy_ecs::prelude::Resource;

/// One frame of laid-out UI, written by the main side and painted here.
#[derive(Resource, Default)]
pub struct EguiFrame {
    /// The tessellated UI, replaced every frame.
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub size_in_pixels: [u32; 2],
    pub pixels_per_point: f32,
    /// Texture changes not yet painted. Layout appends each frame's and the
    /// painter applies and clears them, so a frame that never reaches the
    /// painter (a lost surface, say) drops none: egui sends each change once.
    pub textures: egui::TexturesDelta,
}

#[derive(Resource)]
pub struct EguiPainter {
    renderer: egui_wgpu::Renderer,
}

impl EguiPainter {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: egui_wgpu::Renderer::new(device, surface_format, None, 1, false),
        }
    }

    /// Apply the pending texture changes and paint `frame` onto `output_view`,
    /// which already holds the post-processed scene.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        frame: &mut EguiFrame,
    ) {
        let [width, height] = frame.size_in_pixels;
        if width == 0 || height == 0 {
            return;
        }
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: frame.size_in_pixels,
            pixels_per_point: frame.pixels_per_point,
        };

        let textures = std::mem::take(&mut frame.textures);
        for (id, image_delta) in &textures.set {
            self.renderer.update_texture(device, queue, *id, image_delta);
        }

        self.renderer
            .update_buffers(device, queue, encoder, &frame.primitives, &screen_descriptor);

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

            self.renderer.render(
                &mut render_pass.forget_lifetime(),
                &frame.primitives,
                &screen_descriptor,
            );
        }

        for id in &textures.free {
            self.renderer.free_texture(id);
        }
    }
}
