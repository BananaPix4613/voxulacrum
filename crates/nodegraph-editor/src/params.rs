//! Per-`NodeKind` parameter editor widgets.

use egui::Ui;
use nodegraph_ir::{FractalType, NodeKind};

/// Draw the body of a node - its parameters. Returns `true` if anything
/// was edited this frame.
pub fn params_ui(ui: &mut Ui, kind: &mut NodeKind) -> bool {
    match kind {
        NodeKind::Constant(p) => ui.add(egui::DragValue::new(&mut p.value).speed(0.05)).changed(),
        NodeKind::Threshold(p) => ui.add(egui::DragValue::new(&mut p.threshold).speed(0.05)).changed(),
        NodeKind::Clamp(p) => {
            let a = ui.add(egui::DragValue::new(&mut p.min).speed(0.05).prefix("min ")).changed();
            let b = ui.add(egui::DragValue::new(&mut p.max).speed(0.05).prefix("max ")).changed();
            a || b
        }
        NodeKind::Remap(p) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut p.src_lo).speed(0.05).prefix("src_lo ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut p.src_hi).speed(0.05).prefix("src_hi ")).changed();
            });
            ui.horizontal(|ui| {
                changed |= ui.add(egui::DragValue::new(&mut p.dst_lo).speed(0.05).prefix("dst_lo ")).changed();
                changed |= ui.add(egui::DragValue::new(&mut p.dst_hi).speed(0.05).prefix("dst_hi ")).changed();
            });
            changed
        }
        NodeKind::Output(p) => ui.text_edit_singleline(&mut p.label).changed(),
        NodeKind::Perlin2D(p) | NodeKind::Perlin3D(p)
        | NodeKind::Simplex2D(p) | NodeKind::Simplex3D(p) => noise_params_ui(ui, p),
        NodeKind::DomainWarp(p) => {
            let mut changed = false;
            changed |= ui.add(egui::DragValue::new(&mut p.seed)).changed();
            changed |= ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001).prefix("freq ")).changed();
            changed |= ui.add(egui::DragValue::new(&mut p.amplitude).speed(0.1).prefix("amp ")).changed();
            changed
        }
        NodeKind::CurveMapper(p) => curve_stops_ui(ui, &mut p.stops),
        // Parameterless variants:
        NodeKind::WorldPos(_) | NodeKind::Add(_) | NodeKind::Multiply(_)
        | NodeKind::Subtract(_) | NodeKind::Min(_) | NodeKind::Max(_)
        | NodeKind::Lerp(_) | NodeKind::Union(_) | NodeKind::Intersect(_)
        | NodeKind::DensitySubtract(_) | NodeKind::Mix(_) | NodeKind::Mask(_) => {
            ui.weak("(no parameters)");
            false
        }
    }
}

fn noise_params_ui(ui: &mut Ui, p: &mut nodegraph_ir::NoiseParams) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui.add(egui::DragValue::new(&mut p.seed).prefix("seed ")).changed();
        changed |= ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001).prefix("freq ")).changed();
    });
    ui.horizontal(|ui| {
        changed |= ui.add(egui::DragValue::new(&mut p.octaves).range(1..=10).prefix("oct ")).changed();
        changed |= ui.add(egui::DragValue::new(&mut p.lacunarity).speed(0.05).prefix("lac ")).changed();
        changed |= ui.add(egui::DragValue::new(&mut p.gain).speed(0.05).prefix("gain ")).changed();
    });
    egui::ComboBox::from_id_salt("fractal_type")
        .selected_text(format!("{:?}", p.fractal_type))
        .show_ui(ui, |ui| {
            for ft in [FractalType::FBm, FractalType::Ridged, FractalType::PingPong] {
                if ui.selectable_value(&mut p.fractal_type, ft, format!("{:?}", ft)).clicked() {
                    changed = true;
                }
            }
        });
    changed
}

fn curve_stops_ui(ui: &mut Ui, stops: &mut Vec<(f32, f32)>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    for (i, (x, y)) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.add(egui::DragValue::new(x).speed(0.02).prefix("x ")).changed();
            changed |= ui.add(egui::DragValue::new(y).speed(0.02).prefix("y ")).changed();
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
