//! Debug view for tree skeletons (`voxel-tree-rendering-spec.md` §16 step 1).
//!
//! Derives a skeleton at a placed anchor and draws its node graph as
//! world-space lines colored by heavy-path chain, so the pipe model and the
//! trunk selection can be looked at rather than only asserted in a test.
//!
//! **Armed placement, not cursor tracking.** The preview stands where a click
//! put it and stays there. An earlier version latched to the last picked voxel,
//! which survives the pointer moving *onto* the panel but not the trip across
//! the world to get there - so every reach for a slider moved the tree. Same
//! correction as 16b, for the same reason: aiming and clicking are the same
//! mouse.
//!
//! Nothing here touches world content. The skeleton is derived, drawn and
//! dropped whenever an input changes, which is what the generation path will
//! also do - so this view exercises the real derivation rather than a mock.

use glam::IVec3;
use nodegraph_ir::TreeSpecies;

/// Panel state for the skeleton debug view.
pub struct SkeletonPanelState {
    /// Whether the section is expanded. Collapsed clears the overlay and
    /// disarms: a drawing on screen with no visible control explaining it is a
    /// mode you forget you are in.
    pub open: bool,
    /// The next in-world click places the preview.
    pub armed: bool,
    /// Where the preview stands, set by that click.
    pub origin: Option<IVec3>,
    /// Per-tree seed.
    pub seed: u32,
    /// Node budget as a fraction of the cap.
    pub age: f32,
    /// Shape parameters.
    pub species: TreeSpecies,
    /// Draw a cross at each node sized by its radius.
    pub show_radii: bool,
    /// Outline the voxel cells the wood occupies.
    pub show_wood: bool,
    /// Outline each rendered capsule, so branch thickness can be judged.
    pub show_capsules: bool,
    /// The inputs the current overlay was built from, so a frame that changes
    /// nothing rebuilds nothing. Same role as `BlueprintPanelState::preview_for`.
    pub built: Option<(IVec3, u32, f32, TreeSpecies, bool, bool, bool)>,
    /// Node count of the last derivation. Written back by the system.
    pub nodes: usize,
    /// Chain count of the last derivation. Written back by the system.
    pub chains: u32,
    /// Lobe count of the last derivation. Written back by the system.
    pub lobes: usize,
    /// Wood cell count of the last derivation. Written back by the system.
    pub wood: usize,
    /// Name the next save writes to. Becomes a filename, and is validated as one.
    pub name: String,
    /// Set by the Save button; consumed by the system.
    pub save_requested: bool,
    /// Last successful save, and last failure. Never both.
    pub status: Option<String>,
    pub error: Option<String>,
    /// Horizontal reach of the last derivation. Written back by the system.
    pub reach: f32,
    /// Height of the last derivation. Written back by the system.
    pub height: f32,
}

impl Default for SkeletonPanelState {
    fn default() -> Self {
        Self {
            open: false,
            armed: false,
            origin: None,
            seed: 1,
            age: 1.0,
            species: TreeSpecies::default(),
            show_radii: false,
            show_wood: false,
            show_capsules: false,
            built: None,
            nodes: 0,
            chains: 0,
            lobes: 0,
            wood: 0,
            name: "oak".to_string(),
            save_requested: false,
            status: None,
            error: None,
            reach: 0.0,
            height: 0.0,
        }
    }
}

impl SkeletonPanelState {
    /// HUD line while the placement tool is armed, so the mode is legible with
    /// your eyes out in the world rather than only on the panel that set it.
    pub fn armed_hud_label(&self) -> Option<String> {
        self.armed
            .then(|| "Tree skeleton: click to place the preview — Shift repeats".to_string())
    }
}

/// Draw the section. Every control is an input to the derivation; the system
/// rebuilds the overlay when one of them moves.
pub fn draw(ui: &mut egui::Ui, state: &mut SkeletonPanelState) {
    let header = egui::CollapsingHeader::new("Tree skeleton (debug)").show(ui, |ui| {
        ui.horizontal(|ui| {
            let armed = state.armed;
            if ui
                .add(egui::Button::selectable(armed, "Place"))
                .on_hover_text("Then click a surface in the world")
                .clicked()
            {
                state.armed = !armed;
            }
            if ui
                .add_enabled(state.origin.is_some(), egui::Button::new("Clear"))
                .on_hover_text("Remove the preview")
                .clicked()
            {
                state.origin = None;
                state.armed = false;
            }
        });
        match state.origin {
            Some(v) => ui.weak(format!("standing on {v}")),
            None => ui.colored_label(
                egui::Color32::LIGHT_YELLOW,
                "press Place, then click a surface",
            ),
        };

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("seed");
            ui.add(egui::DragValue::new(&mut state.seed).speed(1.0));
            if ui.button("next").clicked() {
                state.seed = state.seed.wrapping_add(1);
            }
        });
        ui.add(egui::Slider::new(&mut state.age, 0.0..=1.0).text("age"));
        ui.checkbox(&mut state.show_radii, "show radii");
        ui.checkbox(&mut state.show_wood, "show wood voxels");
        ui.checkbox(&mut state.show_capsules, "show capsules");

        ui.separator();
        ui.weak("Species");
        let sp = &mut state.species;
        ui.add(egui::Slider::new(&mut sp.trunk_radius, 0.1..=4.0).text("trunk radius"));
        ui.add(egui::Slider::new(&mut sp.tip_radius, 0.01..=1.0).text("tip radius"));
        ui.add(egui::Slider::new(&mut sp.variance, 0.0..=0.8).text("variance"));
        ui.add(egui::Slider::new(&mut sp.internode, 0.2..=4.0).text("internode"));
        ui.add(egui::Slider::new(&mut sp.taper, 0.5..=1.0).text("taper"));
        ui.add(egui::Slider::new(&mut sp.child_scale, 0.2..=1.0).text("child scale"));
        ui.add(egui::Slider::new(&mut sp.axis_steps, 1..=16).text("axis steps"));
        ui.add(egui::Slider::new(&mut sp.max_order, 0..=6).text("max order"));
        ui.add(egui::Slider::new(&mut sp.laterals, 0..=4).text("laterals"));
        ui.add(egui::Slider::new(&mut sp.branch_angle, 0.0..=1.6).text("branch angle"));
        ui.add(egui::Slider::new(&mut sp.gravitropism, -1.0..=1.0).text("gravitropism"));
        ui.add(egui::Slider::new(&mut sp.wobble, 0.0..=0.6).text("wobble"));

        ui.weak("Crown (space colonization)");
        ui.add(egui::Slider::new(&mut sp.crown_attractors, 0..=800).text("attractors"));
        ui.add(egui::Slider::new(&mut sp.crown_center, 0.0..=1.5).text("crown center"));
        ui.add(egui::Slider::new(&mut sp.crown_radius, 0.5..=8.0).text("crown radius"));
        ui.add(egui::Slider::new(&mut sp.crown_height, 0.5..=8.0).text("crown height"));
        ui.add(egui::Slider::new(&mut sp.influence, 0.5..=10.0).text("influence"));
        ui.add(egui::Slider::new(&mut sp.kill_radius, 0.1..=1.5).text("kill radius"));
        ui.add(egui::Slider::new(&mut sp.colonize_step, 0.1..=2.0).text("step"));

        ui.weak("Canopy (lobes)");
        ui.add(egui::Slider::new(&mut sp.max_lobes, 0..=12).text("max lobes"));
        ui.add(egui::Slider::new(&mut sp.lobe_scale, 0.2..=4.0).text("lobe scale"));
        ui.add(egui::Slider::new(&mut sp.lobe_size_jitter, 0.0..=0.6).text("size jitter"));
        ui.add(egui::Slider::new(&mut sp.lobe_anisotropy, 0.0..=0.6).text("anisotropy"));
        ui.add(egui::Slider::new(&mut sp.lobe_separation, 0.0..=1.5).text("separation"));
        ui.add(egui::Slider::new(&mut sp.lobe_min_radius, 0.05..=2.0).text("min radius"));

        if ui.button("Reset species").clicked() {
            *sp = TreeSpecies::default();
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("name");
            ui.text_edit_singleline(&mut state.name);
            if ui.button("Save species").clicked() {
                state.save_requested = true;
            }
        });
        ui.weak("writes assets/species/<name>.species.json — a Place Tree node names it");

        // One or the other, never both: a stale success line under a fresh
        // error reads as though the error were survivable.
        if let Some(err) = &state.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        } else if let Some(status) = &state.status {
            ui.weak(status);
        }

        ui.separator();
        ui.label(format!(
            "{} nodes, {} chains, {} lobes, {} wood cells, reach {:.2}, height {:.2}",
            state.nodes, state.chains, state.lobes, state.wood, state.reach, state.height
        ));
        ui.weak("orange is the heavy path — the trunk decomposition picked");
        ui.weak("green rings are canopy lobes, sized by the limb that carries them");
    });
    state.open = header.fully_open();
    // A tool armed behind a collapsed section is a mode with no indicator.
    if !state.open {
        state.armed = false;
    }
}
