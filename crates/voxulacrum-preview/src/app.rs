//! `PreviewApp`: the eframe `App` implementation.

use std::path::PathBuf;

use nodegraph_editor::EditorState;
use nodegraph_eval::{ScalarField, CHUNK_DIM};

use crate::render_3d::{ChunkRenderState, OrbitCamera, RenderCallback};
use crate::colormap::Colormap;
use crate::runtime::Runtime;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab { Heatmap, Voxels }

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
    bottom_tab: BottomTab,
    camera: OrbitCamera,
    /// Cached `field_version` that drove the last 3D mesh upload
    last_mesh_version: u64,
}

impl PreviewApp {
    /// Construct the app and load the initial graph (also seeding the editor).
    pub fn new(dir: PathBuf, cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let runtime = Runtime::new(dir)?;
        let wgpu_state = cc.wgpu_render_state.as_ref().expect("eframe wgpu backend required");
        let chunk_renderer = ChunkRenderState::new(
            &wgpu_state.device,
            wgpu_state.target_format,
        );
        wgpu_state.renderer.write().callback_resources.insert(chunk_renderer);
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
            bottom_tab: BottomTab::Heatmap,
            camera: OrbitCamera::default(),
            last_mesh_version: u64::MAX, // forces first upload
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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
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

        // 5. Bottom tabbed panel: 2D Heatmap | 3D Voxels.
        egui::TopBottomPanel::bottom("bottom")
            .default_height(360.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.bottom_tab, BottomTab::Heatmap, "2D Heatmap");
                    ui.selectable_value(&mut self.bottom_tab, BottomTab::Voxels, "3D Voxels");
                });
                ui.separator();
                match self.bottom_tab {
                    BottomTab::Heatmap => self.show_heatmap(ui, ctx),
                    BottomTab::Voxels => self.show_voxels(ui, frame),
                }
            });

        // 6. Central panel: editor canvas.
        egui::CentralPanel::default().show(ctx, |ui| {
            self.editor.show(ui);
        });

        // 7. If anything changed this frame, push the new graph to the runtime.
        if self.editor.consume_dirty() {
            let g = self.editor.build_graph();
            self.runtime.replace_graph(g);
        }
    }
}

impl PreviewApp {
    /// 2D heatmap body (left tab in the bottom panel).
    fn show_heatmap(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
    }

    /// 3D voxel body (right tab in the bottom panel). Uploads the chunk mesh
    /// when `field_version` advances; schedules an egui-wgpu paint callback
    /// for the actual draw.
    fn show_voxels(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let Some(wgpu_state) = frame.wgpu_render_state() else {
            ui.centered_and_justified(|ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, "wgpu backend not available");
            });
            return;
        };

        // 1. (Re)upload the mesh when the field has been refreshed.
        if let Some(terrain) = self.runtime.terrain() {
            if self.last_mesh_version != self.runtime.field_version {
                let mut renderer = wgpu_state.renderer.write();
                if let Some(state) = renderer
                    .callback_resources
                    .get_mut::<ChunkRenderState>()
                {
                    state.update_mesh(
                        &wgpu_state.device,
                        terrain,
                        self.runtime.field_version,
                    );
                }
                self.last_mesh_version = self.runtime.field_version;
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Add a Terrain Output node to render 3D voxels.");
            });
            return;
        }

        // 2. Allocate the viewport rect; handle drag-rotate + scroll-zoom.
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        if response.dragged() {
            let d = response.drag_delta();
            self.camera.rotate(d.x, d.y);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
        }

        // 3. Schedule the paint callback for this frame.
        let cb = egui_wgpu::Callback::new_paint_callback(
            rect,
            RenderCallback {
                camera: self.camera,
                size: [rect.width().max(1.0) as u32, rect.height().max(1.0) as u32],
            },
        );
        ui.painter().add(cb);
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
