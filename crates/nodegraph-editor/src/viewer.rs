//! `SnarlViewer` implementation: how each node draws itself and validates
//! its connections.

use std::collections::HashMap;

use egui::Ui;
use egui_snarl::ui::{PinInfo, SnarlPin, SnarlViewer};
use egui_snarl::{InPin, NodeId, OutPin, Snarl};
use nodegraph_ir::{Diagnostic, NodeCategory, NodeKind, PinType, Severity};

use crate::colors::{category_fill, pin_color};
use crate::params::params_ui;
use crate::state::UndoLabel;

/// Static catalog of every kind that can be inserted from the canvas menu.
pub fn catalog() -> &'static [(NodeCategory, &'static str, fn() -> NodeKind)] {
    use nodegraph_ir::*;
    &[
        (NodeCategory::Source, "Perlin 2D",    || NodeKind::Perlin2D(NoiseParams::default())),
        (NodeCategory::Source, "Perlin 3D",    || NodeKind::Perlin3D(NoiseParams::default())),
        (NodeCategory::Source, "Simplex 2D",   || NodeKind::Simplex2D(NoiseParams::default())),
        (NodeCategory::Source, "Simplex 3D",   || NodeKind::Simplex3D(NoiseParams::default())),
        (NodeCategory::Source, "Constant",     || NodeKind::Constant(ConstantParams::default())),
        (NodeCategory::Source, "World Position", || NodeKind::WorldPos(WorldPosParams::default())),
        (NodeCategory::Math, "Add",            || NodeKind::Add(AddParams::default())),
        (NodeCategory::Math, "Multiply",       || NodeKind::Multiply(MultiplyParams::default())),
        (NodeCategory::Math, "Subtract",       || NodeKind::Subtract(SubtractParams::default())),
        (NodeCategory::Math, "Min",            || NodeKind::Min(MinParams::default())),
        (NodeCategory::Math, "Max",            || NodeKind::Max(MaxParams::default())),
        (NodeCategory::Math, "Clamp",          || NodeKind::Clamp(ClampParams::default())),
        (NodeCategory::Math, "Lerp",           || NodeKind::Lerp(LerpParams::default())),
        (NodeCategory::Math, "Remap",          || NodeKind::Remap(RemapParams::default())),
        (NodeCategory::Curves, "Threshold",    || NodeKind::Threshold(ThresholdParams::default())),
        (NodeCategory::Curves, "Curve Mapper", || NodeKind::CurveMapper(CurveMapperParams::default())),
        (NodeCategory::Domain, "Domain Warp",  || NodeKind::DomainWarp(DomainWarpParams::default())),
        (NodeCategory::Density, "Union",       || NodeKind::Union(UnionParams::default())),
        (NodeCategory::Density, "Intersect",   || NodeKind::Intersect(IntersectParams::default())),
        (NodeCategory::Density, "Density Subtract", || NodeKind::DensitySubtract(DensitySubtractParams::default())),
        (NodeCategory::Density, "Mix",         || NodeKind::Mix(MixParams::default())),
        (NodeCategory::Density, "Mask",        || NodeKind::Mask(MaskParams::default())),
        (NodeCategory::Output, "Output",       || NodeKind::Output(OutputParams::default())),
    ]
}

/// Severity → marker color for the node-header badge.
pub fn severity_color(s: Severity) -> egui::Color32 {
    match s {
        Severity::Error   => egui::Color32::from_rgb(230, 80, 80),
        Severity::Warning => egui::Color32::from_rgb(230, 180, 60),
        Severity::Info    => egui::Color32::from_rgb(100, 180, 230),
    }
}

/// One frame's diagnostics, grouped by node for fast lookup in the viewer.
pub struct DiagnosticIndex {
    /// Strongest severity per IR node id.
    pub by_node: HashMap<nodegraph_ir::NodeId, Severity>,
}

impl DiagnosticIndex {
    /// Build from validation output; keeps the strongest severity per node.
    pub fn from_diagnostics(diagnostics: &[Diagnostic]) -> Self {
        let mut by_node = HashMap::new();
        for d in diagnostics {
            if let Some(n) = d.node {
                let cur = by_node.entry(n).or_insert(d.severity);
                if d.severity == Severity::Error {
                    *cur = Severity::Error;
                } else if d.severity == Severity::Warning && *cur != Severity::Error {
                    *cur = Severity::Warning;
                }
            }
        }
        Self { by_node }
    }
}

/// Per-frame viewer carrying refs to diagnostics + the snarl→ir id map.
pub struct GraphViewer<'a> {
    /// `egui_snarl::NodeId` → `nodegraph_ir::NodeId` map (paired with the
    /// `Graph` whose diagnostics this viewer renders).
    pub id_map: &'a HashMap<NodeId, nodegraph_ir::NodeId>,
    /// This frame's validation findings, indexed by IR node id.
    pub diagnostics: &'a DiagnosticIndex,
    /// User-facing structural actions performed this frame. The host
    /// converts these into undo entries (one per frame, labels joined).
    pub actions: &'a mut Vec<UndoLabel>,
    /// Set whenever a parameter changed this frame (no undo entry, but
    /// triggers a re-eval).
    pub dirty_param: &'a mut bool,
}

impl SnarlViewer<NodeKind> for GraphViewer<'_> {
    fn title(&mut self, node: &NodeKind) -> String {
        node.descriptor().display_name.to_string()
    }

    fn inputs(&mut self, node: &NodeKind) -> usize { node.descriptor().inputs.len() }
    fn outputs(&mut self, node: &NodeKind) -> usize { node.descriptor().outputs.len() }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) -> impl SnarlPin + 'static {
        let spec = snarl[pin.id.node].descriptor().inputs[pin.id.input];
        ui.label(spec.name);
        PinInfo::circle().with_fill(pin_color(spec.ty))
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) -> impl SnarlPin + 'static {
        let spec = snarl[pin.id.node].descriptor().outputs[pin.id.output];
        ui.label(spec.name);
        PinInfo::circle().with_fill(pin_color(spec.ty))
    }

    fn has_body(&mut self, _node: &NodeKind) -> bool { true }

    fn show_body(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) {
        if let Some(&ir_id) = self.id_map.get(&node_id) {
            if let Some(&sev) = self.diagnostics.by_node.get(&ir_id) {
                let color = severity_color(sev);
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, color);
                    ui.label(format!("{:?}", sev));
                });
            }
        }
        let node = &mut snarl[node_id];
        if params_ui(ui, node) {
            *self.dirty_param = true;
        }
    }

    fn header_frame(
        &mut self,
        frame: egui::Frame,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        snarl: &Snarl<NodeKind>,
    ) -> egui::Frame {
        let cat = snarl[node_id].descriptor().category;
        frame.fill(category_fill(cat))
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<NodeKind>) {
        let from_desc = snarl[from.id.node].descriptor();
        let to_desc = snarl[to.id.node].descriptor();
        let from_spec = from_desc.outputs[from.id.output];
        let to_spec = to_desc.inputs[to.id.input];
        if !PinType::is_compatible(from_spec.ty, to_spec.ty) {
            return;
        }
        let label = format!(
            "Connect {}.{} → {}.{}",
            from_desc.display_name, from_spec.name,
            to_desc.display_name, to_spec.name,
        );
        // Single-input rule: drop any existing wire feeding this input first.
        for existing in to.remotes.clone() {
            snarl.disconnect(existing, to.id);
        }
        snarl.connect(from.id, to.id);
        self.actions.push(label);
    }

    fn has_graph_menu(&mut self, _pos: egui::Pos2, _snarl: &mut Snarl<NodeKind>) -> bool { true }

    fn show_graph_menu(
        &mut self,
        pos: egui::Pos2,
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) {
        ui.label("Add node");
        ui.separator();
        for cat in [
            NodeCategory::Source, NodeCategory::Math, NodeCategory::Curves,
            NodeCategory::Domain, NodeCategory::Density, NodeCategory::Output,
        ] {
            ui.menu_button(format!("{cat:?}"), |ui| {
                for (entry_cat, label, factory) in catalog() {
                    if *entry_cat == cat && ui.button(*label).clicked() {
                        snarl.insert_node(pos, factory());
                        self.actions.push(format!("Add {}", label));
                        ui.close();
                    }
                }
            });
        }
    }

    fn has_node_menu(&mut self, _node: &NodeKind) -> bool { true }

    fn show_node_menu(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) {
        if ui.button("Delete").clicked() {
            let label = format!("Delete {}", snarl[node_id].descriptor().display_name);
            snarl.remove_node(node_id);
            self.actions.push(label);
            ui.close();
        }
    }
}
