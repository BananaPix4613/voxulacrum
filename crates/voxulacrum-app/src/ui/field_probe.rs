//! Field probe: headless data plane for the graph-authoring debug panel.
//!
//! The probe evaluates the editor's *live* graph and captures the cached
//! output of whichever node the author has selected on the canvas - the graph
//! itself is the selector (Houdini / Blender model). Evaluation is dispatched
//! onto the shared chunk-generation pool so it never runs on the schedule
//! thread; results return over a channel and are polled each frame.
//!
//! This module owns only the data: the resource, the dispatch/poll system, and
//! the evaluation helper. The panel that renders the captured field as a
//! heatmap is added in a later step.

use std::sync::{mpsc, Arc, Mutex};

use bevy_ecs::prelude::{NonSendMut, Res, ResMut, Resource};
use glam::IVec3;
use nodegraph_eval::{CachedOutput, EvalContext, Evaluator, ScalarField};
use nodegraph_ir::{Graph, NodeId};
use voxel_core::{ChunkBuffer, MaterialRegistry, Voxel};

use crate::ui::colormap::Colormap;
use crate::ui::panels::UiState;
use crate::ui::EguiRenderer;

/// A captured node output, ready for the panel to render. The variant follows
/// whatever the evaluator cached for the selected node.
pub enum ProbeData {
    /// A continuous scalar field (raw noise, a remap result, ...). Rendered as a
    /// value heatmap.
    Scalar(Arc<ScalarField>),
    /// A voxel field (`TerrainOutput` / `BuildTerrain` result). Rendered by
    /// material id.
    Terrain(Arc<ChunkBuffer<Voxel, 32>>),
}

/// Result of one background evaluation, returned over the channel.
struct ProbeResult {
    data: Result<ProbeData, String>,
    eval_ms: f32,
}

/// Debug resource that inspects the output of the selected graph node.
#[derive(Resource)]
pub struct FieldProbe {
    /// Whether the probe is active. Toggled from the UI in a later step.
    pub enabled: bool,
    /// Chunk coordinate to evaluate.
    pub chunk: IVec3,

    /// Most recently captured output, if any.
    pub data: Option<ProbeData>,
    /// Bumped on every new capture; keys the panel's texture cache.
    pub data_version: u64,
    /// Last evaluation error (e.g. nothing selected), if any.
    pub error: Option<String>,

    // --- panel presentation state ---
    /// Colormap applied to scalar fields.
    pub colormap: Colormap,
    /// Height (Y layer) of the slice drawn in the heatmap, in
    /// `0..ScalarField::DIM`.
    pub y_slice: i32,
    /// When true, the scalar value range auto-fits to the field's min/max
    /// (computed over the whole volume, so changing the slice never shifts
    /// the mapping).
    pub auto_range: bool,
    /// Scalar value mapped to the colormap's low end (0.0).
    pub range_min: f32,
    /// Scalar value mapped to the colormap's high end (1.0).
    pub range_max: f32,
    /// Heatmap pan offset, in screen pixels.
    pub pan: [f32; 2],
    /// Heatmap zoom, in screen pixels per texel.
    pub zoom: f32,
    /// Wall-clock duration of the most recent background evaluation.
    pub last_eval_ms: f32,

    // --- dispatch / poll plumbing ---
    pool: Arc<rayon::ThreadPool>,
    result_tx: mpsc::Sender<ProbeResult>,
    result_rx: Mutex<mpsc::Receiver<ProbeResult>>,
    in_flight: bool,
    needs_eval: bool,
    last_revision: u64,
    last_chunk: IVec3,
}

impl FieldProbe {
    pub fn new(pool: Arc<rayon::ThreadPool>) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            enabled: false,
            chunk: IVec3::ZERO,
            data: None,
            data_version: 0,
            error: None,
            colormap: Colormap::Viridis,
            y_slice: 0,
            auto_range: true,
            range_min: 0.0,
            range_max: 1.0,
            pan: [0.0, 0.0],
            zoom: 8.0,
            last_eval_ms: 0.0,
            pool,
            result_tx,
            result_rx: Mutex::new(result_rx),
            in_flight: false,
            needs_eval: false,
            last_revision: 0,
            last_chunk: IVec3::ZERO,
        }
    }

    /// Refit `range_min`/`range_max` to the current scalar field's finite
    /// min/max over the whole volume. No-op for terrain (which colors by
    /// material id, not value) or when no field is loaded.
    pub fn recompute_auto_range(&mut self) {
        if let Some(ProbeData::Scalar(f)) = &self.data {
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for z in 0..ScalarField::DIM {
                for y in 0..ScalarField::DIM {
                    for x in 0..ScalarField::DIM {
                        let v = f.get(x, y, z);
                        if v.is_finite() {
                            lo = lo.min(v);
                            hi = hi.max(v);
                        }
                    }
                }
            }
            if lo <= hi {
                self.range_min = lo;
                // Guard against a zero-width range collapsing the colormap.
                self.range_max = if (hi - lo).abs() < 1e-6 { lo + 1.0 } else { hi };
            }
        }
    }
}

/// Poll completed evaluations and dispatch a new one when the selection, the
/// graph, or the probed chunk changes. Runs in `FrameStage::Meshing`; the
/// editor selection it reads was written during the previous frame's render,
/// so the panel trails live edits by one frame (imperceptible).
pub fn field_probe_system(
    mut probe: ResMut<FieldProbe>,
    egui: NonSendMut<EguiRenderer>,
    ui_state: Res<UiState>,
) {
    // 1. Drain a completed evaluation, if one finished.
    let received = probe.result_rx.get_mut().unwrap().try_recv().ok();
    if let Some(result) = received {
        probe.in_flight = false;
        probe.last_eval_ms = result.eval_ms;
        log::debug!("field probe eval: {:.2} ms", result.eval_ms);
        match result.data {
            Ok(data) => {
                probe.data = Some(data);
                probe.data_version = probe.data_version.wrapping_add(1);
                probe.error = None;
                if probe.auto_range {
                    probe.recompute_auto_range();
                }
            }
            Err(e) => probe.error = Some(e),
        }
    }

    if !probe.enabled {
        return;
    }

    // 2. Re-evaluate when the editor's inspectable state or the chunk changes,
    //    or when we have nothing to show yet.
    let revision = egui.graph_editor.revision();
    if revision != probe.last_revision {
        probe.last_revision = revision;
        probe.needs_eval = true;
    }
    if probe.chunk != probe.last_chunk {
        probe.last_chunk = probe.chunk;
        probe.needs_eval = true;
    }
    if probe.data.is_none() && probe.error.is_none() {
        probe.needs_eval = true;
    }

    // 3. Dispatch onto the shared pool (never the schedule thread).
    if probe.needs_eval && !probe.in_flight {
        let (graph, selected) = egui.graph_editor.build_graph_with_selection();
        probe.needs_eval = false;
        let Some(selected) = selected else {
            probe.data = None;
            probe.error = Some("select a node in the editor to inspect".to_string());
            return;
        };
        let seed = ui_state.params.terrain_gen.seed as u64;
        let chunk = probe.chunk;
        let tx = probe.result_tx.clone();
        probe.in_flight = true;
        probe.pool.spawn(move || {
            let started = std::time::Instant::now();
            let data = evaluate_selected(&graph, selected, seed, chunk);
            let eval_ms = started.elapsed().as_secs_f32() * 1000.0;
            let _ = tx.send(ProbeResult { data, eval_ms });
        });
    }
}

/// Evaluate `graph` for `chunk` and return the selected node's cached output.
fn evaluate_selected(
    graph: &Graph,
    node: NodeId,
    seed: u64,
    chunk: IVec3,
) -> Result<ProbeData, String> {
    let mut eval = Evaluator::new(graph, EvalContext::new(seed, chunk));
    eval.evaluate().map_err(|e| e.to_string())?;
    match eval.cache().get(node) {
        Some(CachedOutput::Scalar(f)) => Ok(ProbeData::Scalar(f.clone())),
        Some(CachedOutput::Terrain(t)) => Ok(ProbeData::Terrain(t.clone())),
        Some(_) => Err("selected node has no displayable field output".to_string()),
        None => Err("selected node was not evaluated".to_string()),
    }
}

/// Render one horizontal (XZ) slice of the captured field to an RGBA image.
///
/// The image is `DIM x DIM`; pixel `(x, z)` maps to the field sample at
/// `(x, y_slice, z)`. Scalar fields are colored by `colormap` over the
/// `[range.0, range.1]` window; terrain is colored by material registry
/// color, with non-solid voxels left transparent.
pub fn build_color_image(
    data: &ProbeData,
    y_slice: usize,
    colormap: Colormap,
    range: (f32, f32),
    registry: &MaterialRegistry,
) -> egui::ColorImage {
    const N: usize = ScalarField::DIM;
    let mut pixels = vec![egui::Color32::TRANSPARENT; N * N];
    let y = y_slice.min(N - 1);

    match data {
        ProbeData::Scalar(f) => {
            let (lo, hi) = range;
            let span = (hi - lo).abs().max(1e-6);
            for z in 0..N {
                for x in 0..N {
                    let t = ((f.get(x, y, z) - lo) / span).clamp(0.0, 1.0);
                    let [r, g, b, _] = colormap.apply(t);
                    pixels[z * N + x] = egui::Color32::from_rgb(r, g, b);
                }
            }
        }
        ProbeData::Terrain(buf) => {
            for z in 0..N {
                for x in 0..N {
                    let vox = buf.get(x, y, z);
                    if !vox.is_solid() {
                        continue; // air / empty stays transparent
                    }
                    let col = registry
                        .get(vox.material)
                        .map(|d| d.color)
                        .unwrap_or([1.0, 0.0, 1.0]); // magenta = missing material
                    let to8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                    pixels[z * N + x] =
                        egui::Color32::from_rgb(to8(col[0]), to8(col[1]), to8(col[2]));
                }
            }
        }
    }

    egui::ColorImage::new([N, N], pixels)
}

/// Edge length (and slice count) of a probed field. The heatmap is one
/// `SLICE_DIM x SLICE_DIM` XZ layer; `y_slice` selects the layer.
pub const SLICE_DIM: usize = ScalarField::DIM;

/// Floating debug window: slice/colormap/range controls plus a pan/zoom
/// heatmap of the selected node's captured field. `texture` is the prebuilt
/// slice image (uploaded by the renderer); `None` means nothing to show yet.
pub fn draw_field_probe_window(
    ctx: &egui::Context,
    probe: &mut FieldProbe,
    registry: &MaterialRegistry,
    texture: Option<&egui::TextureHandle>,
) {
    let mut open = probe.enabled;
    egui::Window::new("Field Probe")
        .open(&mut open)
        .default_width(340.0)
        .resizable(true)
        .show(ctx, |ui| {
            probe_controls(ui, probe);
            ui.separator();
            probe_heatmap(ui, probe, registry, texture);
            ui.separator();
            probe_status(ui, probe);
        });
    // Closing via the window's [x] disables the probe (kept in sync with F3).
    probe.enabled = open;
}

fn probe_controls(ui: &mut egui::Ui, probe: &mut FieldProbe) {
    ui.horizontal(|ui| {
        ui.label("Chunk:");
        ui.add(egui::DragValue::new(&mut probe.chunk.x).speed(1).prefix("x "));
        ui.add(egui::DragValue::new(&mut probe.chunk.y).speed(1).prefix("y "));
        ui.add(egui::DragValue::new(&mut probe.chunk.z).speed(1).prefix("z "));
    });
    ui.horizontal(|ui| {
        ui.label("Y slice:");
        ui.add(egui::Slider::new(&mut probe.y_slice, 0..=(SLICE_DIM as i32 - 1)));
    });
    ui.horizontal(|ui| {
        ui.label("Colormap:");
        egui::ComboBox::from_id_salt("field_probe_colormap")
            .selected_text(probe.colormap.label())
            .show_ui(ui, |ui| {
                for cm in Colormap::ALL {
                    ui.selectable_value(&mut probe.colormap, cm, cm.label());
                }
            });
    });
    ui.horizontal(|ui| {
        if ui.checkbox(&mut probe.auto_range, "Auto range").changed() && probe.auto_range {
            probe.recompute_auto_range();
        }
    });
    ui.add_enabled_ui(!probe.auto_range, |ui| {
        ui.horizontal(|ui| {
            ui.label("min:");
            ui.add(egui::DragValue::new(&mut probe.range_min).speed(0.01));
            ui.label("max:");
            ui.add(egui::DragValue::new(&mut probe.range_max).speed(0.01));
        });
    });
}

fn probe_heatmap(
    ui: &mut egui::Ui,
    probe: &mut FieldProbe,
    registry: &MaterialRegistry,
    texture: Option<&egui::TextureHandle>,
) {
    const N: i32 = SLICE_DIM as i32;

    let viewport = egui::vec2(ui.available_width().clamp(160.0, 420.0), 256.0);
    let (rect, resp) = ui.allocate_exact_size(viewport, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(20));

    let Some(handle) = texture else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "select a node to inspect",
            egui::FontId::proportional(13.0),
            egui::Color32::GRAY,
        );
        return;
    };

    // Pan by dragging.
    if resp.dragged() {
        let d = resp.drag_delta();
        probe.pan[0] += d.x;
        probe.pan[1] += d.y;
    }
    // Zoom toward the cursor on scroll.
    let scroll = ui.input(|i| i.raw_scroll_delta.y);
    if resp.hovered() && scroll.abs() > 0.0 {
        let pivot = resp.hover_pos().unwrap_or(rect.center());
        let img_min = rect.min + egui::vec2(probe.pan[0], probe.pan[1]);
        let texel = (pivot - img_min) / probe.zoom;
        let new_zoom = (probe.zoom * (1.0 + scroll * 0.0015)).clamp(2.0, 48.0);
        let new_min = pivot - texel * new_zoom;
        probe.pan = [new_min.x - rect.min.x, new_min.y - rect.min.y];
        probe.zoom = new_zoom;
    }

    // Blit the slice (NEAREST sampling set on the texture at upload time).
    let img_min = rect.min + egui::vec2(probe.pan[0], probe.pan[1]);
    let img_size = SLICE_DIM as f32 * probe.zoom;
    let img_rect = egui::Rect::from_min_size(img_min, egui::vec2(img_size, img_size));
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(handle.id(), img_rect, uv, egui::Color32::WHITE);

    // Hover read-out: map the cursor to a texel and print its value.
    if let Some(pos) = resp.hover_pos() {
        let rel = (pos - img_min) / probe.zoom;
        let (tx, tz) = (rel.x.floor() as i32, rel.y.floor() as i32);
        if (0..N).contains(&tx) && (0..N).contains(&tz) {
            let y = probe.y_slice.clamp(0, N - 1) as usize;
            let (x, z) = (tx as usize, tz as usize);
            let text = match &probe.data {
                Some(ProbeData::Scalar(f)) => {
                    format!("({}, {}, {}) = {:.4}", x, y, z, f.get(x, y, z))
                }
                Some(ProbeData::Terrain(buf)) => {
                    let vox = buf.get(x, y, z);
                    let name = registry
                        .get(vox.material)
                        .map(|d| d.display_name.as_str())
                        .unwrap_or("?");
                    format!("({}, {}, {}) = {} [{}]", x, y, z, name, vox.material.raw())
                }
                None => String::new(),
            };
            painter.text(
                rect.left_bottom() + egui::vec2(4.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                text,
                egui::FontId::monospace(12.0),
                egui::Color32::WHITE,
            );
        }
    }
}

fn probe_status(ui: &mut egui::Ui, probe: &FieldProbe) {
    if let Some(err) = &probe.error {
        ui.colored_label(egui::Color32::from_rgb(220, 140, 60), err);
    }
    let kind = match &probe.data {
        Some(ProbeData::Scalar(_)) => "scalar",
        Some(ProbeData::Terrain(_)) => "terrain",
        None => "none",
    };
    ui.label(format!("output: {} · {:.2} ms", kind, probe.last_eval_ms));
}
