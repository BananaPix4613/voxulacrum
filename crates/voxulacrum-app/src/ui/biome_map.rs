//! Biome / zone map preview §4.3).
//!
//! A top-down view of assignment at arbitrary scale, sampled without generating
//! chunks. Multi-biome authoring without it is guesswork: a zone band moved in
//! the World graph changes a region you cannot see, and the only feedback is
//! flying there.

use nodegraph_eval::IdMap;

/// Grid edge, in samples. Fixed: the interesting control is `step`, and a
/// resizable grid would just be two ways to spell the same zoom.
pub const MAP_DIM: usize = 128;

/// Request state, last result, and legend.
#[derive(Default)]
pub struct BiomeMapState {
    pub open: bool,
    pub center_x: i32,
    pub center_z: i32,
    /// World units between samples. The zoom control.
    pub step: i32,
    /// Color by zone rather than biome.
    pub show_zones: bool,
    pub dirty: bool,
    pub map: Option<IdMap>,
    /// Bumped on each new map; keys the texture cache.
    pub version: u64,
    pub error: Option<String>,
    /// `(id, name)` for whichever dimension is displayed, refreshed with the map.
    pub zone_legend: Vec<(u16, String)>,
    pub biome_legend: Vec<(u16, String)>,
    /// Camera position, mirrored each frame so "center on camera" works.
    pub camera_x: i32,
    pub camera_z: i32,
}

impl BiomeMapState {
    /// Sensible defaults on first open: 8 world units per sample covers ~1000
    /// units across, which is several biomes at the shipped band frequencies.
    pub fn ensure_defaults(&mut self) {
        if self.step == 0 {
            self.step = 8;
            self.dirty = true;
        }
    }
}

/// A stable, categorical color per id. Not a gradient: these are labels, and a
/// ramp would imply an ordering that ids do not have.
pub fn id_color(id: u16) -> egui::Color32 {
    const PALETTE: [(u8, u8, u8); 12] = [
        (0x4c, 0x9a, 0xff), (0x6c, 0xc0, 0x6c), (0xe0, 0xc0, 0x4c), (0xe0, 0x4c, 0x4c),
        (0x9c, 0x7c, 0xff), (0x4c, 0xc0, 0xc0), (0xff, 0x9c, 0x4c), (0xc0, 0x6c, 0xa0),
        (0x8c, 0xa0, 0x60), (0xb0, 0xb0, 0xc0), (0x60, 0x80, 0xc0), (0xc8, 0x90, 0x60),
    ];
    let (r, g, b) = PALETTE[id as usize % PALETTE.len()];
    egui::Color32::from_rgb(r, g, b)
}

/// Build the map image. `+Z` runs down the image, matching the top-down view.
pub fn build_image(map: &IdMap, show_zones: bool) -> egui::ColorImage {
    let ids = if show_zones { &map.zone } else { &map.biome };
    let pixels: Vec<egui::Color32> = ids.iter().map(|&id| id_color(id)).collect();
    egui::ColorImage::new([map.dim, map.dim], pixels)
}

/// Draw the window. The texture is uploaded by the caller and passed in.
pub fn draw_window(
    ctx: &egui::Context,
    state: &mut BiomeMapState,
    texture: Option<&egui::TextureHandle>,
) {
    let mut open = state.open;
    egui::Window::new("Biome / Zone Map")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("center x:");
                let cx = ui.add(egui::DragValue::new(&mut state.center_x).speed(4.0)).changed();
                ui.label("z:");
                let cz = ui.add(egui::DragValue::new(&mut state.center_z).speed(4.0)).changed();
                if ui.button("Camera").on_hover_text("Center on the camera").clicked() {
                    state.center_x = state.camera_x;
                    state.center_z = state.camera_z;
                    state.dirty = true;
                }
                if cx || cz {
                    state.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("units / sample:");
                if ui
                    .add(egui::DragValue::new(&mut state.step).speed(1.0).range(1..=256))
                    .changed()
                {
                    state.dirty = true;
                }
                if ui.checkbox(&mut state.show_zones, "zones").changed() {
                    // Coloring only - no resample needed, but the texture must
                    // rebuild, and `version` is what keys it.
                    state.version = state.version.wrapping_add(1);
                }
            });

            if let Some(err) = &state.error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
                return;
            }
            let Some(map) = &state.map else {
                ui.weak("Sampling…");
                return;
            };

            let span = map.dim as i32 * map.step;
            ui.weak(format!(
                "{span} x {span} world units, {} units/sample — no chunks generated",
                map.step
            ));

            if let Some(tex) = texture {
                let size = egui::vec2(384.0, 384.0);
                let response = ui.add(
                    egui::Image::new((tex.id(), size))
                        .texture_options(egui::TextureOptions::NEAREST)
                        .sense(egui::Sense::click()),
                );
                // Click to recentre: the map is also how you navigate it.
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let rel = pos - response.rect.min;
                        let gx = (rel.x / size.x * map.dim as f32) as i32;
                        let gz = (rel.y / size.y * map.dim as f32) as i32;
                        state.center_x = map.origin_x + gx * map.step;
                        state.center_z = map.origin_z + gz * map.step;
                        state.dirty = true;
                    }
                }
            }

            ui.separator();
            let ids = if state.show_zones { &map.zone } else { &map.biome };
            let legend = if state.show_zones { &state.zone_legend } else { &state.biome_legend };
            let mut present: Vec<u16> = ids.to_vec();
            present.sort_unstable();
            present.dedup();
            for id in present {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, id_color(id));
                    let name = legend
                        .iter()
                        .find(|(i, _)| *i == id)
                        .map(|(_, n)| n.as_str())
                        .unwrap_or("unregistered");
                    ui.label(format!("{name} ({id})"));
                });
            }
        });
    state.open = open;
}
