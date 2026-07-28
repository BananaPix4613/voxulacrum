//! `SnarlViewer` implementation: how each node draws itself and validates
//! its connections.

use std::collections::HashMap;

use egui::Ui;
use egui_snarl::ui::{PinInfo, SnarlPin, SnarlViewer};
use egui_snarl::{InPin, NodeId, OutPin, Snarl};
use nodegraph_ir::{Diagnostic, NodeCategory, NodeKind, PinType, Severity};

use crate::colors::{category_fill, header_text_color, pin_color, pin_shape};
use crate::params::params_ui;
use crate::state::UndoLabel;

/// Maximum width of a node's body (parameter area), in egui points. Caps
/// node width so long labels/widgets wrap instead of stretching the node.
/// Points scale with DPI, so this stays correct across displays.
const BODY_MAX_WIDTH: f32 = 260.0;

/// Clearance, in egui points, inserted between an output label and its pin
/// marker. Output rows lay out right-to-left, so snarl reserves only a tight
/// gap before the marker; without this the marker crowds the last glyph
/// (e.g. "out" renders as "ou●"). Inputs read away from their marker and
/// don't need it.
const OUTPUT_PIN_GAP: f32 = 6.0;

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
        (NodeCategory::Source, "World Axis",   || NodeKind::WorldAxis(WorldAxisParams::default())),
        (NodeCategory::Source, "Surface Noise", || NodeKind::SurfaceNoise(NoiseParams::default())),
        (NodeCategory::Source, "Y Band",       || NodeKind::YBand(YBandParams::default())),
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
        (NodeCategory::Material, "Constant Material", || NodeKind::ConstantMaterial(ConstantMaterialParams::default())),
        (NodeCategory::Material, "Layer",             || NodeKind::Layer(LayerParams::default())),
        (NodeCategory::Material, "Queue",             || NodeKind::Queue(QueueParams::default())),
        (NodeCategory::Material, "Build Terrain", || NodeKind::BuildTerrain(BuildTerrainParams::default())),
        (NodeCategory::Positions, "Jittered Grid", || NodeKind::JitteredGrid(JitteredGridParams::default())),
        (NodeCategory::Positions, "Poisson Disk", || NodeKind::PoissonDisk(PoissonDiskParams::default())),
        (NodeCategory::Scanners, "Find Flat",  || NodeKind::FindFlat(FindFlatParams::default())),
        (NodeCategory::Props, "Place Tree",    || NodeKind::PlaceTree(PlaceTreeParams::default())),
        (NodeCategory::Props, "Place Prefab",  || NodeKind::PlacePrefab(PlacePrefabParams::default())),
        (NodeCategory::Output, "Output",       || NodeKind::Output(OutputParams::default())),
        (NodeCategory::Output, "Terrain Output", || NodeKind::TerrainOutput(TerrainOutputParams::default())),
        (NodeCategory::Output, "World Output", || NodeKind::WorldOutput(WorldOutputParams::default())),
        (NodeCategory::Output, "Zone Output",  || NodeKind::ZoneOutput(ZoneOutputParams::default())),
        (NodeCategory::Foliage, "Poisson Distribution", || NodeKind::PoissonDistribution(PoissonDistributionParams::default())),
        (NodeCategory::Foliage, "Surface Filter", || NodeKind::SurfaceFilter(SurfaceFilterParams::default())),
        (NodeCategory::Foliage, "Biome Context Mask", || NodeKind::BiomeContextMask(BiomeContextMaskParams::default())),
        (NodeCategory::Foliage, "Species Picker", || NodeKind::SpeciesPicker(SpeciesPickerParams::default())),
        (NodeCategory::Foliage, "Paint Density", || NodeKind::PaintDensity(PaintDensityParams::default())),
        (NodeCategory::Foliage, "Scatter Place", || NodeKind::ScatterPlace(ScatterPlaceParams::default())),
    ]
}

/// Severity → marker color for the node's diagnostic badge, shown at the
/// top of the node body (not the header).
pub fn severity_color(s: Severity) -> egui::Color32 {
    match s {
        Severity::Error   => egui::Color32::from_rgb(220, 60, 60),
        Severity::Warning => egui::Color32::from_rgb(220, 200, 60),
        Severity::Info    => egui::Color32::from_rgb(100, 180, 230),
    }
}

/// Short label for a wire action between two nodes (by display name).
fn wire_label(verb: &str, from: &NodeKind, to: &NodeKind) -> String {
    format!(
        "{verb} {} -> {}",
        from.descriptor().display_name,
        to.descriptor().display_name,
    )
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
    /// Display name of the node whose parameter changed this frame, if any.
    /// Used to label the coalesced param-edit undo entry.
    pub param_label: &'a mut Option<String>,
    /// (id, display name) of every biome in the world, for ZoneOutput's biome
    /// picker. Empty for graphs edited outside a world context.
    pub biomes: &'a [(u16, String)],
    /// The node currently selected for inspection, used for highlighting.
    pub selected: Option<NodeId>,
    /// Out-param: the node whose header was left-clicked this frame, if any.
    pub clicked: &'a mut Option<NodeId>,
}

impl SnarlViewer<NodeKind> for GraphViewer<'_> {
    fn title(&mut self, node: &NodeKind) -> String {
        node.descriptor().display_name.to_string()
    }

    fn node_frame(
        &mut self,
        frame: egui::Frame,
        _node: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        _snarl: &Snarl<NodeKind>,
    ) -> egui::Frame {
        // A touch more breathing room inside the node body so the header,
        // pins and params don't crowd the border.
        frame.inner_margin(egui::Margin::same(8))
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
        let frame = frame
            .fill(category_fill(cat))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .corner_radius(egui::CornerRadius::same(4));
        if Some(node_id) == self.selected {
            frame.stroke(egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 210, 90)))
        } else {
            frame
        }
    }

    fn show_header(
        &mut self,
        node_id: NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) {
        let desc = snarl[node_id].descriptor();
        let text = egui::RichText::new(desc.display_name)
            .color(header_text_color(desc.category))
            .strong();
        // Title glyphs plus a transparent strip across the rest of the header
        // make the whole colored band a target. `selectable(false)` stops the
        // label from eating clicks as text selection; `Sense::click` leaves the
        // drag-to-move gesture with snarl. Selecting on *press* (not just a
        // clean click) means a click that nudges into a drag still selects.
        let title = ui.add(
            egui::Label::new(text)
                .selectable(false)
                .sense(egui::Sense::click()));
        let strip_w = ui.available_width();
        let strip_h = title.rect.height();
        let (_r, strip) =
            ui.allocate_exact_size(egui::vec2(strip_w, strip_h), egui::Sense::click());
        if title.is_pointer_button_down_on()
            || strip.is_pointer_button_down_on()
            || title.clicked()
            || strip.clicked()
        {
            *self.clicked = Some(node_id);
        }
    }

    fn inputs(&mut self, node: &NodeKind) -> usize { node.effective_inputs().len() }

    fn show_input(
        &mut self,
        pin: &InPin,
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) -> impl SnarlPin + 'static {
        let pins = snarl[pin.id.node].effective_inputs();
        let spec = &pins[pin.id.input];
        ui.label(spec.name);
        PinInfo::default()
            .with_shape(pin_shape(spec.ty))
            .with_fill(pin_color(spec.ty))
    }

    fn outputs(&mut self, node: &NodeKind) -> usize { node.effective_outputs().len() }

    fn show_output(
        &mut self,
        pin: &OutPin,
        ui: &mut Ui,
        snarl: &mut Snarl<NodeKind>,
    ) -> impl SnarlPin + 'static {
        let pins = snarl[pin.id.node].effective_outputs();
        let spec = &pins[pin.id.output];
        // Output rows are right-to-left: this space lands to the *right* of
        // the label, opening clearance before the pin marker so it stops
        // crowding the last glyph.
        ui.add_space(OUTPUT_PIN_GAP);
        ui.label(spec.name);
        PinInfo::default()
            .with_shape(pin_shape(spec.ty))
            .with_fill(pin_color(spec.ty))
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
        // Snarl builds the body `ui` with a left-to-right layout, which would
        // string every param row out horizontally until they overlap. Open a
        // vertical layout so the diagnostic badge and each param row stack
        // top-to-bottom, one per line. The width cap keeps a long widget or
        // label from stretching the whole node.
        ui.vertical(|ui| {
            ui.set_max_width(BODY_MAX_WIDTH);

            if let Some(&ir_id) = self.id_map.get(&node_id) {
                if let Some(&sev) = self.diagnostics.by_node.get(&ir_id) {
                    let color = severity_color(sev);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 5.0, color);
                        ui.label(format!("{:?}", sev));
                    });
                    ui.add_space(2.0);
                }
            }
            let name = snarl[node_id].descriptor().display_name;
            let node = &mut snarl[node_id];
            if params_ui(ui, node, self.biomes) {
                *self.dirty_param = true;
                *self.param_label = Some(name.to_string());
            }
        });
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
            NodeCategory::Domain, NodeCategory::Density, NodeCategory::Material,
            NodeCategory::Positions, NodeCategory::Scanners, NodeCategory::Props,
            NodeCategory::Biome, NodeCategory::Foliage, NodeCategory::Output,
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

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<NodeKind>) {
        let label = {
            let from_node = &snarl[from.id.node];
            let to_node = &snarl[to.id.node];
            let from_pins = from_node.effective_outputs();
            let to_pins = to_node.effective_inputs();
            let from_spec = &from_pins[from.id.output];
            let to_spec = &to_pins[to.id.input];
            if !PinType::is_compatible(from_spec.ty, to_spec.ty) {
                return;
            }
            format!(
                "Connect {}.{} → {}.{}",
                from_node.descriptor().display_name, from_spec.name,
                to_node.descriptor().display_name, to_spec.name,
            )
        };
        // Single-input rule: drop any existing wire feeding this input first.
        for existing in to.remotes.clone() {
            snarl.disconnect(existing, to.id);
        }
        snarl.connect(from.id, to.id);
        self.actions.push(label);
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<NodeKind>) {
        let label = wire_label("Disconnect", &snarl[from.id.node], &snarl[to.id.node]);
        snarl.disconnect(from.id, to.id);
        self.actions.push(label);
    }
    
    fn drop_outputs(&mut self, pin: &OutPin, snarl: &mut Snarl<NodeKind>) {
        if pin.remotes.is_empty() {
            return; // nothing removed -> no undo entry
        }
        let label = format!(
            "Disconnect {} outputs",
            snarl[pin.id.node].descriptor().display_name
        );
        snarl.drop_outputs(pin.id);
        self.actions.push(label);
    }
    
    fn drop_inputs(&mut self, pin: &InPin, snarl: &mut Snarl<NodeKind>) {
        if pin.remotes.is_empty() {
            return;
        }
        let label = format!(
            "Disconnect {} input",
            snarl[pin.id.node].descriptor().display_name
        );
        snarl.drop_inputs(pin.id);
        self.actions.push(label);
    }
}
