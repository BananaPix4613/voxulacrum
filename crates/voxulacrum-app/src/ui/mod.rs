pub mod panels;
pub mod colormap;
pub mod column_inspector;
pub mod field_probe;
pub mod hierarchy_editor;
pub mod biome_map;
pub mod blueprint_panel;

use egui_wgpu::ScreenDescriptor;

use crate::rendering::render_context::RenderContext;
use colormap::Colormap;
use hierarchy_editor::HierarchyEditor;

/// Cache key for the field-probe slice texture. The texture is rebuilt only
/// when one of these changes (data capture, slice, colormap, or value range).
#[derive(Clone, Copy, PartialEq)]
struct ProbeTexKey {
    data_version: u64,
    y_slice: i32,
    colormap: Colormap,
    range_bits: (u32, u32),
}

/// A cached, uploaded field-probe slice texture plus the key it was built from.
struct ProbeTexture {
    key: ProbeTexKey,
    handle: egui::TextureHandle,
}

pub struct EguiRenderer {
    pub ctx: egui::Context,
    pub winit_state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
    pub visible: bool,
    /// The embedded node-graph editor + the world's graph hierarchy/selector.
    /// Loaded from the world manifest at startup (see `main.rs`); the panel is
    /// toggled with F2 (hidden by default).
    pub graph_editor: HierarchyEditor,
    /// Whether the left-side graph editor panel is shown.
    pub editor_visible: bool,
    /// Cached field-probe slice texture; re-uploaded only on key change.
    probe_texture: Option<ProbeTexture>,
    /// Cached biome-map texture, keyed on the map's version counter.
    map_texture: Option<(u64, egui::TextureHandle)>,
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
            graph_editor: HierarchyEditor::new(),
            editor_visible: false,
            probe_texture: None,
            map_texture: None,
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
    
    pub fn toggle_editor(&mut self) {
        self.editor_visible = !self.editor_visible;
    }

    /// Rebuild + re-upload the field-probe slice texture when its cache key
    /// changes. No-op when the probe has no captured data.
    fn refresh_probe_texture(
        &mut self,
        probe: &field_probe::FieldProbe,
        registry: &voxel_core::MaterialRegistry,
    ) {
        let Some(data) = &probe.data else {
            self.probe_texture = None;
            return;
        };
        let key = ProbeTexKey {
            data_version: probe.data_version,
            y_slice: probe.y_slice,
            colormap: probe.colormap,
            range_bits: (probe.range_min.to_bits(), probe.range_max.to_bits()),
        };
        if self.probe_texture.as_ref().map(|t| t.key) == Some(key) {
            return;
        }
        let y = probe.y_slice.clamp(0, field_probe::SLICE_DIM as i32 - 1) as usize;
        let image = field_probe::build_color_image(
            data,
            y,
            probe.colormap,
            (probe.range_min, probe.range_max),
            registry,
        );
        let handle = self.ctx.load_texture(
            "field_probe_slice",
            image,
            egui::TextureOptions::NEAREST,
        );
        self.probe_texture = Some(ProbeTexture { key, handle });
    }


    /// Rebuild the biome-map texture when its version changes.
    fn refresh_map_texture(&mut self, state: &biome_map::BiomeMapState) {
        let Some(map) = &state.map else {
            self.map_texture = None;
            return;
        };
        if self.map_texture.as_ref().map(|(v, _)| *v) == Some(state.version) {
            return;
        }
        let image = biome_map::build_image(map, state.show_zones);
        let handle =
            self.ctx
                .load_texture("biome_map", image, egui::TextureOptions::NEAREST);
        self.map_texture = Some((state.version, handle));
    }

    /// Run the egui UI and render it onto the given surface view.
    /// The surface view should already contain the post-processed scene.
    pub fn draw(
        &mut self,
        ui_state: &mut panels::UiState,
        probe: &mut field_probe::FieldProbe,
        registry: &voxel_core::MaterialRegistry,
        ctx: &RenderContext,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        window: &winit::window::Window,
        mode: crate::world::mutation::EngineMode,
        material_name: &str,
        material_color: [f32; 3],
    ) {
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        // Refresh the probe's slice texture before tessellation so the upload
        // is flushed in this frame's `textures_delta`.
        if probe.enabled {
            self.refresh_probe_texture(probe, registry);
        }
        if ui_state.biome_map.open {
            self.refresh_map_texture(&ui_state.biome_map);
        }
        
        let raw_input = self.winit_state.take_egui_input(window);
        let full_output = self.ctx.run(raw_input, |ctx| {
            if self.visible {
                if self.editor_visible {
                    panels::draw_graph_editor_panel(ctx, &mut self.graph_editor);
                }
                panels::draw_engine_panel(ctx, ui_state);
            }
            if let Some(label) = ui_state.blueprint_panel.armed_hud_label() {
                panels::draw_armed_hud(ctx, &label);
            }
            if probe.enabled {
                let tex = self.probe_texture.as_ref().map(|t| &t.handle);
                field_probe::draw_field_probe_window(ctx, probe, registry, tex);
            }
            if ui_state.biome_map.open {
                let tex = self.map_texture.as_ref().map(|(_, h)| h);
                biome_map::draw_window(ctx, &mut ui_state.biome_map, tex);
            }
            if mode == crate::world::mutation::EngineMode::Play {
                panels::draw_play_hud(ctx, material_name, material_color);
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