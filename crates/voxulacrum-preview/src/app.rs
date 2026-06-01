//! `PreviewApp`: the eframe `App` implementation.

use std::path::PathBuf;

use nodegraph_editor::{
    snarl_to_graph, DiagnosticIndex, EditorState, GraphViewer,
};
use nodegraph_eval::{ScalarField, CHUNK_DIM};

use crate::colormap::Colormap;
use crate::runtime::Runtime;

/// Cache key for the currently-uploaded texture; re-upload only when it changes.
#[derive(Clone, Copy, PartialEq)]
struct TextureKey {
    field_version: u64,
    y_slice: usize,
    range: (f32, f32),
    colormap: Colormap,
}

/// Pan/zoom state for the heatmap image.
struct PanZoom {
    /// Pixels per voxel.
    zoom: f32,
    /// Pan offset (screen pixels) of the image's top-left from the area's top-left.
    pan: egui::Vec2,
}

impl Default for PanZoom {
    fn default() -> Self {
        Self { zoom: 8.0, pan: egui::Vec2::ZERO }
    }
}

/// The eframe app.
pub struct PreviewApp {
    runtime: Runtime,
    editor: EditorState,
    y_slice: usize,
    range: (f32, f32),
    colormap: Colormap,
    pan_zoom: PanZoom,
    texture: Option<egui::TextureHandle>,
    texture_key: Option<TextureKey>,
    hover_readout: Option<(usize, usize, f32)>,
}

impl PreviewApp {
    /// Construct the app and load the initial graph (also seeding the editor).
    pub fn new(dir: PathBuf) -> Result<Self, String> {
        let runtime = Runtime::new(dir)?;
        let editor = match runtime.current_graph() {
            Some(g) => EditorState::from_graph(g),
            None => EditorState::new(),
        };
        Ok(Self {
            runtime,
            editor,
            y_slice: 0,
            range: (-1.0, 1.5),
            colormap: Colormap::Viridis,
            pan_zoom: PanZoom::default(),
            texture: None,
            texture_key: None,
            hover_readout: None,
        })
    }

    fn ensure_texture(&mut self, ctx: &egui::Context) {
        let Some(field) = self.runtime.field.as_ref() else {
            self.texture = None;
            self.texture_key = None;
            return;
        };
        let want = TextureKey {
            field_version: self.runtime.field_version,
            y_slice: self.y_slice,
            range: self.range,
            colormap: self.colormap,
        };
        if self.texture_key == Some(want) && self.texture.is_some() {
            return;
        }
        let img = bake_color_image(field, want.y_slice, want.range, want.colormap);
        self.texture = Some(ctx.load_texture("preview_heatmap", img, egui::TextureOptions::NEAREST));
        self.texture_key = Some(want);
    }
}

impl eframe::App for PreviewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. Watcher → reload from disk if external edit landed.
        if self.runtime.poll() {
            if let Some(g) = self.runtime.current_graph() {
                self.editor = EditorState::from_graph(g);
            }
            ctx.request_repaint();
        }

        // 2. Top toolbar: New / Save / Undo / Redo / modified marker.
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New").clicked() {
                    if self.editor.is_modified() {
                        self.editor.toast("Click Discard, then New, to clear unsaved changes");
                    } else {
                        self.editor = EditorState::new();
                        self.runtime.replace_graph(self.editor.build_graph());
                    }
                }
                if ui.button("Discard").clicked() {
                    if let Some(g) = self.runtime.current_graph() {
                        self.editor = EditorState::from_graph(g);
                        self.editor.toast("Discarded unsaved changes");
                    }
                }
                if ui.button("Save").clicked() {
                    let g = self.editor.build_graph();
                    match self.runtime.save_to_disk(&g) {
                        Ok(p) => {
                            self.editor.mark_saved();
                            self.editor.toast(format!("Saved {}", p.display()));
                        }
                        Err(e) => self.editor.toast(format!("Save failed: {e}")),
                    }
                }
                let undo_label = self.editor.peek_undo().map(str::to_owned);
                if ui
                    .add_enabled(undo_label.is_some(), egui::Button::new("Undo"))
                    .on_hover_text(undo_label.as_deref().unwrap_or(""))
                    .clicked()
                {
                    if let Some(l) = self.editor.undo() {
                        self.editor.toast(format!("Undid: {l}"));
                    }
                }
                let redo_label = self.editor.peek_redo().map(str::to_owned);
                if ui
                    .add_enabled(redo_label.is_some(), egui::Button::new("Redo"))
                    .on_hover_text(redo_label.as_deref().unwrap_or(""))
                    .clicked()
                {
                    if let Some(l) = self.editor.redo() {
                        self.editor.toast(format!("Redid: {l}"));
                    }
                }
                if self.editor.is_modified() {
                    ui.colored_label(egui::Color32::YELLOW, "● modified");
                }
                ui.label(format!("Last eval: {:.1} ms", self.runtime.last_eval_ms));
                if let Some(err) = &self.runtime.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {err}"));
                }
            });
        });

        // 3. Right side panel: heatmap controls + hover readout.
        egui::SidePanel::right("controls").min_width(220.0).show(ctx, |ui| {
            ui.heading("Preview");
            ui.label(format!("Graph: {}", self.runtime.graph_path().display()));
            ui.separator();

            ui.label("Y slice");
            ui.add(egui::Slider::new(&mut self.y_slice, 0..=(CHUNK_DIM - 1)));

            ui.label("Colormap");
            egui::ComboBox::from_id_salt("colormap")
                .selected_text(self.colormap.label())
                .show_ui(ui, |ui| {
                    for c in Colormap::ALL {
                        ui.selectable_value(&mut self.colormap, c, c.label());
                    }
                });

            ui.label("Value range (lo / hi)");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.range.0).speed(0.05));
                ui.add(egui::DragValue::new(&mut self.range.1).speed(0.05));
            });
            if self.range.1 <= self.range.0 {
                self.range.1 = self.range.0 + 1e-3;
            }

            ui.separator();
            if ui.button("Reset view").clicked() {
                self.pan_zoom = PanZoom::default();
            }
            ui.label(format!("Zoom: {:.1}x", self.pan_zoom.zoom));
            ui.separator();

            ui.label("Hover");
            match self.hover_readout {
                Some((x, z, v)) => ui.monospace(format!("x={x:>3} z={z:>3}  v = {v:+.4}")),
                None => ui.monospace("(off-image)"),
            };

            ui.separator();
            self.editor.show_toast(ui);
        });

        // 4. Refresh the heatmap texture if any input changed.
        self.ensure_texture(ctx);

        // 5. Bottom panel: heatmap with pan/zoom + hover.
        egui::TopBottomPanel::bottom("heatmap")
            .default_height(300.0)
            .resizable(true)
            .show(ctx, |ui| {
                self.hover_readout = None;

                let Some(texture) = self.texture.as_ref() else {
                    ui.centered_and_justified(|ui| { ui.label("no field"); });
                    return;
                };

                let area = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(area, egui::Sense::click_and_drag());

                if response.dragged() {
                    self.pan_zoom.pan += response.drag_delta();
                }
                if response.hovered() {
                    let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        let factor = (scroll * 0.005).exp();
                        self.pan_zoom.zoom = (self.pan_zoom.zoom * factor).clamp(1.0, 64.0);
                    }
                }

                let img_size = egui::vec2(CHUNK_DIM as f32, CHUNK_DIM as f32) * self.pan_zoom.zoom;
                let img_min = area.min + self.pan_zoom.pan;
                let img_rect = egui::Rect::from_min_size(img_min, img_size);

                ui.painter_at(area).image(
                    texture.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );

                if let (Some(pos), Some(field)) =
                    (response.hover_pos(), self.runtime.field.as_ref())
                {
                    let rel = (pos - img_rect.min) / self.pan_zoom.zoom;
                    let x = rel.x.floor() as i32;
                    let z = rel.y.floor() as i32;
                    if (0..CHUNK_DIM as i32).contains(&x) && (0..CHUNK_DIM as i32).contains(&z) {
                        let v = field.get(x as usize, self.y_slice, z as usize);
                        self.hover_readout = Some((x as usize, z as usize, v));
                    }
                }
            });

        // 6. Central panel: editor canvas.
        egui::CentralPanel::default().show(ctx, |ui| {
            // Capture the pre-mutation graph snapshot for undo, plus the
            // diagnostic + id-map pair the viewer needs.
            let (pre_graph, id_map) = snarl_to_graph(&self.editor.snarl);
            let diagnostics = DiagnosticIndex::from_diagnostics(&pre_graph.validate());

            let mut actions: Vec<nodegraph_editor::UndoLabel> = Vec::new();
            let mut dirty_param = false;
            let mut viewer = GraphViewer {
                id_map: &id_map,
                diagnostics: &diagnostics,
                actions: &mut actions,
                dirty_param: &mut dirty_param,
            };
            self.editor.snarl.show(
                &mut viewer,
                &egui_snarl::ui::SnarlStyle::default(),
                egui::Id::new("snarl-editor"),
                ui,
            );

            if !actions.is_empty() {
                // Coalesce all structural actions in this frame into one
                // undo entry against the pre-mutation snapshot.
                let label = actions.join(", ");
                self.editor.push_undo(label, pre_graph);
            } else if dirty_param {
                self.editor.mark_dirty();
            }
        });

        // 7. If anything changed this frame, push the new graph to the runtime.
        if self.editor.consume_dirty() {
            let g = self.editor.build_graph();
            self.runtime.replace_graph(g);
        }
    }
}

/// Bake a Y-slice of `field` into an `egui::ColorImage` via `colormap`.
fn bake_color_image(
    field: &ScalarField,
    y: usize,
    range: (f32, f32),
    colormap: Colormap,
) -> egui::ColorImage {
    let (lo, hi) = range;
    let span = (hi - lo).max(1e-6);
    let mut pixels = Vec::with_capacity(CHUNK_DIM * CHUNK_DIM * 4);
    for z in 0..CHUNK_DIM {
        for x in 0..CHUNK_DIM {
            let t = ((field.get(x, y, z) - lo) / span).clamp(0.0, 1.0);
            pixels.extend_from_slice(&colormap.apply(t));
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([CHUNK_DIM, CHUNK_DIM], &pixels)
}
