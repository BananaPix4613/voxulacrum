//! Blueprint authoring panel (roadmap §4.2): capture a region of the world into
//! a blueprint, and stamp a saved one back.
//!
//! **Armed tools, not act-on-hover.** Aiming and clicking are the same mouse, so
//! a button that acts on "wherever the cursor points" can never be pressed while
//! pointing anywhere useful. A button here *arms* a tool; the next click in the
//! world performs it at the picked voxel. Right-click cancels.
//!
//! This module owns presentation and request state only. Reading the world and
//! writing it both happen in `blueprint_authoring_system`.

use glam::IVec3;

/// What the next in-world click will do.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ArmedTool {
    /// Clicks fall through to whatever else wants them.
    #[default]
    None,
    /// Two clicks: the first sets corner A, the second corner B.
    Region,
    /// One click: stamp the selected blueprint one voxel above the target.
    Stamp,
}

/// One cell of the cached preview: anchor-relative, **unrotated**, with its
/// material color already resolved.
///
/// Resolved in the system, which holds the registry, so the panel stays pure
/// presentation. Unrotated so changing the yaw redraws without touching disk.
pub struct PreviewCell {
    /// Offset from the anchor cell.
    pub at: [i32; 3],
    /// Runtime shape, so slabs draw at half height.
    pub shape: voxel_core::ShapeId,
    /// Linear RGB, matching the convention `draw_play_hud` uses for swatches.
    pub color: [f32; 3],
}

/// Request state plus the last result.
pub struct BlueprintPanelState {
    /// Whether the section is expanded. Collapsing disarms.
    pub open: bool,
    /// The armed tool, consumed by the next in-world click.
    pub armed: ArmedTool,
    /// The two corners of the capture region, inclusive, in world voxels.
    pub corner_a: Option<IVec3>,
    pub corner_b: Option<IVec3>,
    /// Name for the next capture. Becomes a filename, and is validated as one.
    pub name: String,
    pub capture_requested: bool,
    /// Re-read the library from disk. True initially so the first expand fills it.
    pub refresh_requested: bool,
    /// Blueprint names found on disk, sorted.
    pub library: Vec<String>,
    pub selected: usize,
    /// Rotation applied to the next stamp. Survives a stamp, so a whole row of
    /// rotated copies is one rotation choice and many clicks.
    pub yaw: voxel_core::Yaw,
    /// Cached cells of the selected blueprint, for the preview.
    pub preview: Vec<PreviewCell>,
    /// Which blueprint `preview` was built from, so it reloads on change only.
    pub preview_for: Option<String>,
    /// The camera's snapped orientation, negated, so the preview shows the
    /// blueprint the way the world will. Written by the system each frame.
    pub camera_yaw: voxel_core::Yaw,
    /// Last successful action, and last failure. Never both.
    pub status: Option<String>,
    pub error: Option<String>,
}

impl Default for BlueprintPanelState {
    fn default() -> Self {
        Self {
            open: false,
            armed: ArmedTool::None,
            corner_a: None,
            corner_b: None,
            name: "untitled".to_string(),
            capture_requested: false,
            refresh_requested: true,
            library: Vec::new(),
            selected: 0,
            yaw: voxel_core::Yaw::Deg0,
            preview: Vec::new(),
            preview_for: None,
            camera_yaw: voxel_core::Yaw::Deg0,
            status: None,
            error: None,
        }
    }
}

impl BlueprintPanelState {
    /// Both corners as `(min, max)`, if a full region is set.
    pub fn region(&self) -> Option<(IVec3, IVec3)> {
        match (self.corner_a, self.corner_b) {
            (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
            _ => None,
        }
    }

    /// Move one bound of one axis by `delta`, never letting min pass max.
    ///
    /// Not a convenience. A corner can only be placed on a voxel the pick ray
    /// actually hits, so without this a region can never be trimmed to a shape
    /// whose faces are not all clickable surfaces - which is most of the time.
    pub fn adjust(&mut self, axis: usize, high: bool, delta: i32) {
        let Some((mut lo, mut hi)) = self.region() else {
            return;
        };
        if high {
            hi[axis] = (hi[axis] + delta).max(lo[axis]);
        } else {
            lo[axis] = (lo[axis] + delta).min(hi[axis]);
        }
        self.corner_a = Some(lo);
        self.corner_b = Some(hi);
    }

    /// The selected blueprint's name, if the library has one.
    pub fn selected_name(&self) -> Option<&str> {
        self.library.get(self.selected).map(String::as_str)
    }

    /// What to show on the screen HUD while a tool is armed, or `None`.
    ///
    /// On the screen rather than only in the panel because an armed tool is a
    /// mode, and the whole point of arming one is to go look at the world.
    pub fn armed_hud_label(&self) -> Option<String> {
        match self.armed {
            ArmedTool::None => None,
            ArmedTool::Region => Some(match (self.corner_a, self.corner_b) {
                (Some(_), None) => "Blueprint: click the opposite corner".to_string(),
                _ => "Blueprint: click the first corner".to_string(),
            }),
            ArmedTool::Stamp => Some(format!(
                "Blueprint: click to stamp {} at {} — R rotates, Shift repeats",
                self.selected_name().unwrap_or("(none)"),
                self.yaw.label()
            )),
        }
    }
}

/// Preview height, in points.
const PREVIEW_H: f32 = 132.0;
/// Face shading, brightest first: top, +X, +Z.
const FACE_SHADE: [f32; 3] = [1.0, 0.72, 0.55];
/// Largest blueprint that gets drawn. Past this the preview reports its size
/// instead: a preview that costs more than the thing it previews is worse than
/// none, and refusing is legible where a stutter is not.
const PREVIEW_MAX_CELLS: usize = 4096;
/// Quads are grown slightly about their center so neighbors overlap. Adjacent
/// coplanar faces each antialias against the background at their shared edge,
/// which shows as a hairline seam; face culling cannot reach that, and an
/// overlap covers it. Small enough not to distort the silhouette.
const QUAD_OVERLAP: f32 = 1.015;

fn shaded(c: [f32; 3], shade: f32) -> egui::Color32 {
    egui::Color32::from_rgb(
        (c[0] * shade * 255.0).clamp(0.0, 255.0) as u8,
        (c[1] * shade * 255.0).clamp(0.0, 255.0) as u8,
        (c[2] * shade * 255.0).clamp(0.0, 255.0) as u8,
    )
}

/// Project to unit screen space: +X right-and-down, +Z left-and-down, +Y up.
/// Screen Y grows downward, which is why the height term is subtracted.
fn project(x: f32, y: f32, z: f32) -> egui::Pos2 {
    egui::pos2((x - z) * 0.866_025_4, (x + z) * 0.5 - y)
}

/// Grow a quad about its center so neighbors overlap, hiding AA seams.
fn inflate(quad: [egui::Pos2; 4]) -> [egui::Pos2; 4] {
    let c = egui::pos2(
        (quad[0].x + quad[1].x + quad[2].x + quad[3].x) * 0.25,
        (quad[0].y + quad[1].y + quad[2].y + quad[3].y) * 0.25,
    );
    quad.map(|p| c + (p - c) * QUAD_OVERLAP)
}

/// The camera-facing quads of one cell that are not buried, each tagged with
/// its depth. `covered` reports whether a full cube sits in a given direction —
/// only a full cube can occlude a whole face, so this is exact rather than
/// conservative, and it leaves slab faces alone.
fn cell_quads(
    at: [i32; 3],
    shape: voxel_core::ShapeId,
    color: [f32; 3],
    covered: &dyn Fn([i32; 3]) -> bool,
    out: &mut Vec<(f32, [egui::Pos2; 4], egui::Color32)>,
) {
    let (x, y, z) = (at[0] as f32, at[1] as f32, at[2] as f32);
    let (lo, hi) = match shape {
        voxel_core::ShapeId::SlabBottom => (0.0, 0.5),
        voxel_core::ShapeId::SlabTop => (0.5, 1.0),
        _ => (0.0, 1.0),
    };
    let (yb, yt) = (y + lo, y + hi);
    // Larger x+y+z is nearer the viewer in this projection.
    let depth = x + y + z;
    let show_top = !covered([at[0], at[1] + 1, at[2]]);
    let show_px = !covered([at[0] + 1, at[1], at[2]]);
    let show_pz = !covered([at[0], at[1], at[2] + 1]);
    if show_top {
        out.push((
            depth,
            [
                project(x, yt, z),
                project(x + 1.0, yt, z),
                project(x + 1.0, yt, z + 1.0),
                project(x, yt, z + 1.0),
            ],
            shaded(color, FACE_SHADE[0]),
        ));
    }
    if show_px {
        out.push((
            depth,
            [
                project(x + 1.0, yt, z),
                project(x + 1.0, yt, z + 1.0),
                project(x + 1.0, yb, z + 1.0),
                project(x + 1.0, yb, z),
            ],
            shaded(color, FACE_SHADE[1]),
        ));
    }
    if show_pz {
        out.push((
            depth,
            [
                project(x, yt, z + 1.0),
                project(x + 1.0, yt, z + 1.0),
                project(x + 1.0, yb, z + 1.0),
                project(x, yb, z + 1.0),
            ],
            shaded(color, FACE_SHADE[2]),
        ));
    }
}

/// Isometric preview of the selected blueprint at the current rotation, with
/// the anchor and facing marked.
///
/// Painted with egui shapes rather than rasterized to a texture: a blueprint is
/// a few hundred quads, so this is less code than a `ColorImage`, is
/// resolution-independent, and needs no upload or cache key.
pub fn draw_preview(ui: &mut egui::Ui, cells: &[PreviewCell], yaw: voxel_core::Yaw) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(240.0), PREVIEW_H),
        egui::Sense::hover(),
    );
    if cells.is_empty() {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no preview",
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
        return;
    }

    if cells.len() > PREVIEW_MAX_CELLS {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{} cells — too large to preview", cells.len()),
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
        return;
    }

    // Occupancy of full cubes only, in rotated space, so buried faces can be
    // dropped. Building it once is what turns a volume back into a surface.
    let solid: std::collections::HashSet<[i32; 3]> = cells
        .iter()
        .filter(|c| c.shape == voxel_core::ShapeId::Cube)
        .map(|c| yaw.apply(c.at))
        .collect();
    let covered = |at: [i32; 3]| solid.contains(&at);

    let mut quads: Vec<(f32, [egui::Pos2; 4], egui::Color32)> = Vec::new();
    for cell in cells {
        cell_quads(yaw.apply(cell.at), cell.shape, cell.color, &covered, &mut quads);
    }

    // The yaw turns about the anchor cell's *center*, which is why the anchor is
    // a fixed point at every rotation and why these markers use +0.5.
    let anchor = project(0.5, 0.0, 0.5);
    let facing = {
        let [ax, _, az] = yaw.apply([2, 0, 0]);
        project(ax as f32 + 0.5, 0.0, az as f32 + 0.5)
    };

    let (mut min, mut max) = (egui::pos2(f32::MAX, f32::MAX), egui::pos2(f32::MIN, f32::MIN));
    for (_, quad, _) in &quads {
        for p in quad {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    let span = egui::vec2((max.x - min.x).max(1e-3), (max.y - min.y).max(1e-3));
    let scale = (rect.width() / span.x).min(rect.height() / span.y) * 0.82;
    let mid = egui::pos2((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
    let to_screen =
        |p: egui::Pos2| rect.center() + egui::vec2((p.x - mid.x) * scale, (p.y - mid.y) * scale);

    // Painter's algorithm: farther first, so nearer cells overwrite them.
    quads.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let painter = ui.painter_at(rect);
    for (_, quad, fill) in &quads {
        painter.add(egui::Shape::convex_polygon(
            inflate(*quad).iter().map(|p| to_screen(*p)).collect(),
            *fill,
            // No stroke: with hidden faces culled, the three shading levels
            // carry the form, and an outline on every quad was the doubled
            // edge artifact rather than definition.
            egui::Stroke::NONE,
        ));
    }

    // Drawn last so they are never buried by a tall blueprint.
    painter.line_segment(
        [to_screen(anchor), to_screen(facing)],
        egui::Stroke::new(1.5, egui::Color32::LIGHT_BLUE),
    );
    painter.circle_filled(to_screen(anchor), 3.0, egui::Color32::LIGHT_BLUE);
}

/// Draw the section. Buttons arm tools; the world click is what acts.
pub fn draw(ui: &mut egui::Ui, state: &mut BlueprintPanelState) {
    let header = egui::CollapsingHeader::new("Blueprints").show(ui, |ui| {
        if state.armed != ArmedTool::None {
            ui.colored_label(
                egui::Color32::LIGHT_YELLOW,
                "armed — click in the world; Esc or right-click cancels",
            );
        }

        ui.weak("Capture");
        ui.horizontal(|ui| {
            let armed = state.armed == ArmedTool::Region;
            if ui
                .selectable_label(armed, "Set region")
                .on_hover_text("Then click two opposite corners in the world")
                .clicked()
            {
                state.armed = if armed { ArmedTool::None } else { ArmedTool::Region };
                state.corner_a = None;
                state.corner_b = None;
            }
            if ui.button("Clear").clicked() {
                state.armed = ArmedTool::None;
                state.corner_a = None;
                state.corner_b = None;
            }
        });
        match (state.region(), state.corner_a) {
            (Some((lo, hi)), _) => {
                let d = hi - lo + IVec3::ONE;
                let cells = d.x as i64 * d.y as i64 * d.z as i64;
                ui.weak(format!("region {}×{}×{} — {cells} cells scanned", d.x, d.y, d.z));
                egui::Grid::new("blueprint_region_axes")
                    .num_columns(4)
                    .spacing([10.0, 2.0])
                    .show(ui, |ui| {
                        for axis in 0..3 {
                            ui.label(["X", "Y", "Z"][axis]);
                            ui.label(format!("{} … {}", lo[axis], hi[axis]));
                            ui.horizontal(|ui| {
                                ui.small("min");
                                if ui.small_button("-").clicked() {
                                    state.adjust(axis, false, -1);
                                }
                                if ui.small_button("+").clicked() {
                                    state.adjust(axis, false, 1);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.small("max");
                                if ui.small_button("-").clicked() {
                                    state.adjust(axis, true, -1);
                                }
                                if ui.small_button("+").clicked() {
                                    state.adjust(axis, true, 1);
                                }
                            });
                            ui.end_row();
                        }
                    });
            }
            (None, Some(a)) => {
                ui.weak(format!("corner A at {}, {}, {}", a.x, a.y, a.z));
            }
            _ => {
                ui.weak("no region");
            }
        }
        ui.horizontal(|ui| {
            ui.label("name:");
            ui.text_edit_singleline(&mut state.name);
            if ui
                .add_enabled(
                    state.corner_a.is_some() && state.corner_b.is_some(),
                    egui::Button::new("Capture"),
                )
                .clicked()
            {
                state.capture_requested = true;
            }
        });

        ui.separator();
        ui.weak("Stamp — lands one voxel above the clicked surface");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("blueprint_library")
                .selected_text(state.selected_name().unwrap_or("(none)"))
                .show_ui(ui, |ui| {
                    for (i, name) in state.library.iter().enumerate() {
                        ui.selectable_value(&mut state.selected, i, name);
                    }
                });
            let armed = state.armed == ArmedTool::Stamp;
            if ui
                .add_enabled(
                    !state.library.is_empty(),
                    egui::Button::selectable(armed, "Stamp"),
                )
                .on_hover_text("Then click a surface in the world")
                .clicked()
            {
                state.armed = if armed { ArmedTool::None } else { ArmedTool::Stamp };
            }
            if ui.button("Reload").clicked() {
                state.refresh_requested = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("rotation:");
            for yaw in voxel_core::Yaw::ALL {
                if ui.selectable_label(state.yaw == yaw, yaw.label()).clicked() {
                    state.yaw = yaw;
                }
            }
        });
        draw_preview(ui, &state.preview, state.yaw.then(state.camera_yaw));
        ui.weak("● anchor and facing — the anchor lands one voxel above the click");

        // One or the other, never both: a stale success line under a fresh error
        // reads as though the error were survivable.
        if let Some(err) = &state.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        } else if let Some(status) = &state.status {
            ui.weak(status);
        }
    });
    state.open = header.fully_open();
    // A tool armed behind a collapsed section is a mode with no indicator.
    if !state.open {
        state.armed = ArmedTool::None;
    }
}
