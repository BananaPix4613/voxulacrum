//! Per-`NodeKind` parameter editor widgets.

use egui::Ui;
use nodegraph_ir::{Axis, FractalType, GraphRefTarget, NodeKind};
use voxel_core::MaterialId;

/// One parameter row: a left-aligned label followed by its editor widget.
/// Returns `true` if the widget reported a change this frame.
fn row(ui: &mut Ui, label: &str, widget: impl FnOnce(&mut Ui) -> egui::Response) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        widget(ui)
    })
        .inner
        .changed()
}

/// Two tightly-related parameter fields on one row:
/// `label_a [a]   label_b [b]`. Returns `true` if either widget changed.
/// Use only for short-labeled pairs that fit the body width cap.
fn row2(
    ui: &mut Ui,
    label_a: &str,
    a: impl FnOnce(&mut Ui) -> egui::Response,
    label_b: &str,
    b: impl FnOnce(&mut Ui) -> egui::Response,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label_a);
        let ca = a(ui).changed();
        ui.label(label_b);
        let cb = b(ui).changed();
        ca || cb
    })
        .inner
}

/// Draw the body of a node - its parameters. Returns `true` if anything
/// was edited this frame.
pub fn params_ui(ui: &mut Ui, kind: &mut NodeKind) -> bool {
    match kind {
        NodeKind::Constant(p) => row(ui, "value", |ui| {
            ui.add(egui::DragValue::new(&mut p.value).speed(0.05))
        }),
        NodeKind::Threshold(p) => row(ui, "threshold", |ui| {
            ui.add(egui::DragValue::new(&mut p.threshold).speed(0.05))
        }),
        NodeKind::Clamp(p) => row2(
            ui,
            "min", |ui| ui.add(egui::DragValue::new(&mut p.min).speed(0.05)),
            "max", |ui| ui.add(egui::DragValue::new(&mut p.max).speed(0.05)),
        ),
        NodeKind::Remap(p) => {
            let mut changed = false;
            changed |= row2(
                ui,
                "src lo", |ui| ui.add(egui::DragValue::new(&mut p.src_lo).speed(0.05)),
                "src hi", |ui| ui.add(egui::DragValue::new(&mut p.src_hi).speed(0.05)),
            );
            changed |= row2(
                ui,
                "dst lo", |ui| ui.add(egui::DragValue::new(&mut p.dst_lo).speed(0.05)),
                "dst hi", |ui| ui.add(egui::DragValue::new(&mut p.dst_hi).speed(0.05)),
            );
            changed
        }
        NodeKind::Output(p) => row(ui, "label", |ui| ui.text_edit_singleline(&mut p.label)),
        NodeKind::Perlin2D(p) | NodeKind::Perlin3D(p)
        | NodeKind::Simplex2D(p) | NodeKind::Simplex3D(p) => noise_params_ui(ui, p),
        NodeKind::WorldAxis(p) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("axis");
                egui::ComboBox::from_id_salt("world_axis")
                    .selected_text(format!("{:?}", p.axis))
                    .show_ui(ui, |ui| {
                        for ax in [Axis::X, Axis::Y, Axis::Z] {
                            if ui.selectable_value(&mut p.axis, ax, format!("{:?}", ax)).clicked() {
                                changed = true;
                            }
                        }
                    });
            });
            changed
        }
        NodeKind::DomainWarp(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "frequency", |ui| ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001)));
            changed |= row(ui, "amplitude", |ui| ui.add(egui::DragValue::new(&mut p.amplitude).speed(0.1)));
            changed
        }
        NodeKind::CurveMapper(p) => curve_stops_ui(ui, &mut p.stops),
        NodeKind::ConstantMaterial(p) => material_id_ui(ui, &mut p.material, "material"),
        NodeKind::Layer(p) => layer_bands_ui(ui, p),
        NodeKind::JitteredGrid(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "cell size", |ui| ui.add(egui::DragValue::new(&mut p.cell_size).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "jitter", |ui| ui.add(egui::DragValue::new(&mut p.jitter).speed(0.02).range(0.0..=1.0)));
            changed |= row(ui, "density", |ui| ui.add(egui::DragValue::new(&mut p.density).speed(0.02).range(0.0..=1.0)));
            changed
        }
        NodeKind::PoissonDisk(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "radius", |ui| ui.add(egui::DragValue::new(&mut p.radius).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "k", |ui| ui.add(egui::DragValue::new(&mut p.k).range(1..=64)));
            changed
        }
        NodeKind::FindFlat(p) => {
            let mut changed = row(ui, "max step", |ui| ui.add(egui::DragValue::new(&mut p.max_step).range(0..=16)));
            changed |= material_list_ui(ui, &mut p.on_materials);
            changed
        }
        NodeKind::PlaceTree(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "trunk min", |ui| ui.add(egui::DragValue::new(&mut p.trunk_min).range(1..=32)));
            changed |= row(ui, "trunk max", |ui| ui.add(egui::DragValue::new(&mut p.trunk_max).range(1..=32)));
            changed |= row(ui, "canopy radius", |ui| ui.add(egui::DragValue::new(&mut p.canopy_radius).range(1..=8)));
            changed |= material_id_ui(ui, &mut p.trunk_material, "trunk");
            changed |= material_id_ui(ui, &mut p.leaf_material, "leaf");
            changed
        }
        NodeKind::PlacePrefab(p) => row(ui, "prefab", |ui| ui.text_edit_singleline(&mut p.prefab)),
        NodeKind::LibraryRef(p) => row(ui, "library id", |ui| {
            ui.add(egui::DragValue::new(&mut p.library.0))
        }),
        NodeKind::GraphRef(p) => graph_ref_target_ui(ui, &mut p.target),
        NodeKind::GraphOutput(p) => {
            row(ui, "output name", |ui| ui.text_edit_singleline(&mut p.name))
        }
        NodeKind::SurfaceNoise(p) => noise_params_ui(ui, p),
        NodeKind::WorldOutput(p) => band_list_ui(ui, "zone bands (ascending)", &mut p.zone_bands),
        NodeKind::ZoneOutput(p) => band_list_ui(ui, "biome bands (ascending)", &mut p.biome_bands),
        NodeKind::YBand(p) => row2(
            ui,
            "min y", |ui| ui.add(egui::DragValue::new(&mut p.min).speed(0.5)),
            "max y", |ui| ui.add(egui::DragValue::new(&mut p.max).speed(0.5)),
        ),
        NodeKind::PoissonDistribution(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "radius", |ui| ui.add(egui::DragValue::new(&mut p.radius).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "jitter", |ui| ui.add(egui::DragValue::new(&mut p.jitter).speed(0.02).range(0.0..=1.0)));
            changed
        }
        NodeKind::SurfaceFilter(p) => {
            let mut changed = row(ui, "max slope", |ui| ui.add(egui::DragValue::new(&mut p.max_slope).speed(0.05).range(0.0..=10.0)));
            changed |= row(ui, "min height", |ui| ui.add(egui::DragValue::new(&mut p.min_height).speed(0.5)));
            changed |= row(ui, "max height", |ui| ui.add(egui::DragValue::new(&mut p.max_height).speed(0.5)));
            changed |= material_list_ui(ui, &mut p.materials);
            changed
        }
        NodeKind::BiomeContextMask(p) => row(ui, "biome", |ui| ui.add(egui::DragValue::new(&mut p.biome))),
        NodeKind::SpeciesPicker(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= band_list_ui(ui, "species weights", &mut p.weights);
            changed
        }
        NodeKind::PaintDensity(p) => {
            let mut changed = row(ui, "layer id", |ui| ui.add(egui::DragValue::new(&mut p.layer_id)));
            changed |= row(ui, "species", |ui| ui.add(egui::DragValue::new(&mut p.species)));
            changed |= row(ui, "density", |ui| ui.add(egui::DragValue::new(&mut p.density)));
            changed |= row(ui, "tint", |ui| ui.add(egui::DragValue::new(&mut p.tint)));
            changed
        }
        NodeKind::ScatterPlace(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "type id", |ui| ui.add(egui::DragValue::new(&mut p.type_id)));
            changed |= row(ui, "prefab id", |ui| ui.add(egui::DragValue::new(&mut p.prefab_id)));
            changed
        }
        NodeKind::BiomeParam(p) => {
            let mut changed = row(ui, "param name", |ui| ui.text_edit_singleline(&mut p.name));
            changed |= row(ui, "default", |ui| ui.add(egui::DragValue::new(&mut p.default).speed(0.05)));
            changed
        }
        // Parameterless variants:
        NodeKind::WorldPos(_) | NodeKind::Add(_) | NodeKind::Multiply(_)
        | NodeKind::Subtract(_) | NodeKind::Min(_) | NodeKind::Max(_)
        | NodeKind::Lerp(_) | NodeKind::Union(_) | NodeKind::Intersect(_)
        | NodeKind::DensitySubtract(_) | NodeKind::Mix(_) | NodeKind::Mask(_)
        | NodeKind::Queue(_) | NodeKind::TerrainOutput(_) | NodeKind::BuildTerrain(_)
        | NodeKind::DensityOutput(_) => {
            ui.weak("(no parameters)");
            false
        }
    }
}

/// Editor for a [`GraphRefTarget`]: pick World / Zone / Biome, plus a biome id
/// when Biome. Returns `true` if edited this frame.
fn graph_ref_target_ui(ui: &mut Ui, target: &mut GraphRefTarget) -> bool {
    let mut changed = false;
    let selected = match target {
        GraphRefTarget::World => "World".to_string(),
        GraphRefTarget::Zone => "Zone".to_string(),
        GraphRefTarget::Biome(id) => format!("Biome {id}"),
    };
    ui.horizontal(|ui| {
        ui.label("target");
        egui::ComboBox::from_id_salt("graph_ref_target")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui.selectable_label(matches!(target, GraphRefTarget::World), "World").clicked()
                    && !matches!(target, GraphRefTarget::World)
                {
                    *target = GraphRefTarget::World;
                    changed = true;
                }
                if ui.selectable_label(matches!(target, GraphRefTarget::Zone), "Zone").clicked()
                    && !matches!(target, GraphRefTarget::Zone)
                {
                    *target = GraphRefTarget::Zone;
                    changed = true;
                }
                if ui.selectable_label(matches!(target, GraphRefTarget::Biome(_)), "Biome").clicked()
                    && !matches!(target, GraphRefTarget::Biome(_))
                {
                    *target = GraphRefTarget::Biome(0);
                    changed = true;
                }
            });
    });
    if let GraphRefTarget::Biome(id) = target {
        changed |= row(ui, "biome id", |ui| ui.add(egui::DragValue::new(id)));
    }
    changed
}

/// Editor for an ascending list of band thresholds (zone or biome selection),
/// labeled by `label`. Returns `true` if edited this frame.
fn band_list_ui(ui: &mut Ui, label: &str, bands: &mut Vec<f32>) -> bool {
    let mut changed = false;
    ui.weak(label);
    let mut remove = None;
    for (i, b) in bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.add(egui::DragValue::new(b).speed(0.01)).changed() {
                changed = true;
            }
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        bands.remove(i);
        changed = true;
    }
    if ui.small_button("+ band").clicked() {
        bands.push(0.0);
        changed = true;
    }
    changed
}

fn noise_params_ui(ui: &mut Ui, p: &mut nodegraph_ir::NoiseParams) -> bool {
    let mut changed = false;
    changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
    changed |= row(ui, "frequency", |ui| ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001)));
    changed |= row(ui, "octaves", |ui| ui.add(egui::DragValue::new(&mut p.octaves).range(1..=10)));
    changed |= row(ui, "lacunarity", |ui| ui.add(egui::DragValue::new(&mut p.lacunarity).speed(0.05)));
    changed |= row(ui, "gain", |ui| ui.add(egui::DragValue::new(&mut p.gain).speed(0.05)));
    ui.horizontal(|ui| {
        ui.label("fractal");
        egui::ComboBox::from_id_salt("fractal_type")
            .selected_text(format!("{:?}", p.fractal_type))
            .show_ui(ui, |ui| {
                for ft in [FractalType::FBm, FractalType::Ridged, FractalType::PingPong] {
                    if ui.selectable_value(&mut p.fractal_type, ft, format!("{:?}", ft)).clicked() {
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// Inline drag-value for a `MaterialId` (just its u16). Authors can look up
/// IDs in the preview's material color table.
fn material_id_ui(ui: &mut Ui, id: &mut MaterialId, label: &str) -> bool {
    row(ui, label, |ui| {
        ui.add(egui::DragValue::new(&mut id.0).range(0..=u16::MAX))
    })
}

/// Editor for a `Vec<MaterialId>` (the FindFlat allow-list). Empty = any.
fn material_list_ui(ui: &mut Ui, mats: &mut Vec<MaterialId>) -> bool {
    let mut changed = false;
    ui.label("on materials (empty = any):");
    let mut remove: Option<usize> = None;
    for (i, m) in mats.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("mat");
            changed |= ui.add(egui::DragValue::new(&mut m.0)).changed();
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        mats.remove(i);
        changed = true;
    }
    if ui.button("+ material").clicked() {
        mats.push(MaterialId(6));
        changed = true;
    }
    changed
}

fn layer_bands_ui(ui: &mut Ui, p: &mut nodegraph_ir::LayerParams) -> bool {
    let mut changed = false;
    ui.label("Bands (top → down):");
    let mut remove: Option<usize> = None;
    for (i, (mat, thickness)) in p.bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("mat");
            changed |= ui.add(egui::DragValue::new(&mut mat.0)).changed();
            ui.label("×");
            changed |= ui
                .add(egui::DragValue::new(thickness).range(1u32..=64))
                .changed();
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        p.bands.remove(i);
        changed = true;
    }
    if ui.button("+ band").clicked() {
        p.bands.push((MaterialId(1), 1));
        changed = true;
    }
    ui.separator();
    changed |= material_id_ui(ui, &mut p.fill, "fill");
    changed
}

fn curve_stops_ui(ui: &mut Ui, stops: &mut Vec<(f32, f32)>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    for (i, (x, y)) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("x");
            changed |= ui.add(egui::DragValue::new(x).speed(0.02)).changed();
            ui.label("y");
            changed |= ui.add(egui::DragValue::new(y).speed(0.02)).changed();
            if ui.small_button("✕").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        stops.remove(i);
        changed = true;
    }
    if ui.button("+ stop").clicked() {
        let last = stops.last().copied().unwrap_or((0.0, 0.0));
        stops.push((last.0 + 0.1, last.1));
        changed = true;
    }
    changed
}
