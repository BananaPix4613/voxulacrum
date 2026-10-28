pub mod panels;
pub mod colormap;
pub mod column_inspector;
pub mod field_probe;
pub mod hierarchy_editor;
pub mod biome_map;
pub mod blueprint_panel;
pub mod skeleton_panel;

use bevy_ecs::prelude::*;

use crate::rendering::egui_painter::EguiFrame;
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

/// The egui context and everything the panels keep between frames. Lays the
/// UI out on the main side; `EguiPainter` paints the result.
pub struct EguiUi {
    pub ctx: egui::Context,
    pub winit_state: egui_winit::State,
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

impl EguiUi {
    /// `max_texture_side` is the device's largest 2D texture, which egui sizes
    /// its font atlas against.
    pub fn new(window: &winit::window::Window, max_texture_side: usize) -> Self {
        let ctx = egui::Context::default();

        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(max_texture_side),
        );

        Self {
            ctx,
            winit_state,
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

    /// Run the egui UI and lay it out into `frame`: this frame's primitives and
    /// screen, with its texture changes appended to the ones not yet painted.
    pub fn run(
        &mut self,
        ui_state: &mut panels::UiState,
        probe: &mut field_probe::FieldProbe,
        registry: &voxel_core::MaterialRegistry,
        window: &winit::window::Window,
        play_hud: Option<(&str, [f32; 3])>,
        frame: &mut EguiFrame,
    ) {
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            frame.primitives.clear();
            return;
        }

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
                panels::draw_engine_panel(ctx, ui_state, registry);
            }
            // At most one tool is armed at a time - `skeleton_debug_system`
            // disarms itself while a blueprint tool holds the click - so these
            // chain rather than stacking two HUD lines in the same place.
            if let Some(label) = ui_state
                .blueprint_panel
                .armed_hud_label()
                .or_else(|| ui_state.skeleton_panel.armed_hud_label())
            {
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
            if let Some((material_name, material_color)) = play_hud {
                panels::draw_play_hud(ctx, material_name, material_color);
            }
        });

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        frame.primitives = self
            .ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        frame.size_in_pixels = [size.width, size.height];
        frame.pixels_per_point = window.scale_factor() as f32;
        frame.textures.append(full_output.textures_delta);
    }
}

/// Lay out this frame's UI into `EguiFrame`, which the Render stage paints.
/// Runs last in UniformWrite, after the uniforms are written, so a panel edit
/// takes effect on the next frame, as it did when layout ran in the Render stage.
pub fn egui_layout_system(
    mut egui: NonSendMut<EguiUi>,
    mut ui_state: ResMut<panels::UiState>,
    mut probe: ResMut<field_probe::FieldProbe>,
    registry: Res<crate::materials::MaterialRegistryRes>,
    window: Res<crate::ecs::resources::WindowHandle>,
    world: Res<crate::ecs::resources::VoxelWorld>,
    tool: Res<crate::interaction::PlayerToolState>,
    play: Option<Res<crate::play::PlaySettings>>,
    mut frame: ResMut<EguiFrame>,
) {
    // The build tool's selected material, when the package shows it in Play.
    let in_play = world.0.mode == crate::world::mutation::EngineMode::Play;
    let play_hud = play
        .as_deref()
        .filter(|play| in_play && play.hud)
        .and_then(|play| play.build.as_ref())
        .and_then(|build| build.materials.get(tool.selected))
        .and_then(|material| material.id())
        .and_then(|id| registry.0.get(id))
        .map(|def| (def.display_name.as_str(), def.color));

    egui.run(&mut *ui_state, &mut *probe, &registry.0, &window.0, play_hud, &mut *frame);
}
