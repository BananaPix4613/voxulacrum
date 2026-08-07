//! Column inspector (roadmap §4.3): the generation pipeline for one world
//! column, stage by stage.
//!
//! The question it answers is "why is this column this biome" - which terrain
//! cannot answer once a world has several zones and several biomes, because the
//! result of five stages looks the same as the result of one.
//!
//! This module owns presentation and the request state; the query itself is
//! `WorldGenerator::inspect_column`, which runs the production path.

use crate::world::world_generator::ColumnInspection;

/// Request state plus the last result.
#[derive(Default)]
pub struct ColumnInspectorState {
    /// Whether the section is expanded and querying.
    pub open: bool,
    /// World column to inspect.
    pub world_x: i32,
    /// World column to inspect.
    pub world_z: i32,
    /// Set when an input changes; the query system consumes it.
    ///
    /// A dirty flag rather than a query per frame: the inspection costs a column
    /// pipeline pass and a border scan, which is nothing on demand and wasteful
    /// sixty times a second.
    pub dirty: bool,
    /// Last successful report.
    pub report: Option<ColumnInspection>,
    /// Last error, if the query failed.
    pub error: Option<String>,
}

/// Draw the inspector section. Returns nothing; the query runs in a system.
pub fn draw(ui: &mut egui::Ui, state: &mut ColumnInspectorState) {
    let header = egui::CollapsingHeader::new("Column Inspector").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("x:");
            let cx = ui.add(egui::DragValue::new(&mut state.world_x).speed(1.0)).changed();
            ui.label("z:");
            let cz = ui.add(egui::DragValue::new(&mut state.world_z).speed(1.0)).changed();
            if cx || cz
                || ui
                .button("Refresh")
                .on_hover_text("Runs the full pipeline for this chunk; expect a brief pause")
                .clicked()
            {
                state.dirty = true;
            }
        });

        if let Some(err) = &state.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
            return;
        }
        let Some(inspection) = &state.report else {
            ui.weak("Set a column and press Refresh.");
            return;
        };
        let r = &inspection.report;

        ui.label(format!(
            "column ({}, {})   chunk ({}, {})",
            r.world_x, r.world_z, r.chunk.x, r.chunk.z
        ));
        // Worth stating once, prominently: this runs the generator. Player edits
        // and simulated fluid live on resident chunks and are not here.
        ui.weak("generated state — excludes player edits and simulated fluid");
        ui.separator();

        ui.weak("1. World graph — climate");
        if r.climate.is_empty() {
            ui.label("   (no named outputs)");
        }
        for (name, v) in &r.climate {
            ui.label(format!("   {name}: {v:.4}"));
        }

        ui.weak("2. World graph — zone assignment");
        ui.label(format!("   {} ({})", inspection.zone_name, r.zone_id));

        ui.weak("3. Zone graph — biome assignment");
        ui.label(format!("   {} ({})", inspection.biome_name, r.biome_id));

        ui.weak("4. Border analysis");
        if r.border_neighbor == r.biome_id {
            ui.label(format!(
                "   no differing biome within {:.0} — no blend",
                r.fade_radius
            ));
        } else {
            ui.label(format!(
                "   nearest differing biome {} ({}) at {:.2} (fade radius {:.0})",
                inspection.neighbor_name, r.border_neighbor, r.border_distance, r.fade_radius
            ));
        }

        ui.weak("5. Biome density (own biome, pointwise)");
        if r.density.is_empty() {
            ui.label("   (this biome produces no density)");
        } else {
            egui::ScrollArea::vertical()
                .id_salt("column_density")
                .max_height(120.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (y, v) in r.density.iter().rev() {
                        match v {
                            Some(d) => ui.label(format!("   y {y:>4}   {d:>8.3}")),
                            // Not cosmetic: the pointwise sampler failing here is
                            // the exact condition that breaks chunk-Y seam
                            // material continuation.
                            None => ui.label(format!("   y {y:>4}   (not sampleable)")),
                        };
                    }
                });
        }

        ui.weak(format!("6. Composited voxels (chunk Y {})", r.voxel_chunk_y));
        match r.surface_y {
            None => ui.label("   air column"),
            Some(sy) => ui.label(format!("   surface y {sy}")),
        };
        for (i, (y, mat, shape)) in r.voxels.iter().enumerate() {
            let marker = if i == 0 { "<- surface" } else { "" };
            ui.label(format!("   y {y:>4}   material {:<4} {shape:?} {marker}", mat.0));
        }

        ui.weak("7. Fluid (generation sources)");
        match r.pond_level {
            Some(level) => {
                ui.label(format!("   biome pond surface y {level:.1}"));
            }
            None => {
                ui.label("   no biome pond (no FluidOutput on this biome)");
            }
        }
        // The ocean is applied at the storage boundary from the manifest's sea
        // level, after the evaluator has finished, so it is derived here rather
        // than reported by generation.
        match r.surface_y {
            Some(sy) if sy < inspection.sea_level => ui.label(format!(
                "   ocean fills to y {} (surface is {} below)",
                inspection.sea_level,
                inspection.sea_level - sy
            )),
            Some(_) => ui.label(format!("   above sea level ({})", inspection.sea_level)),
            None => ui.label(format!("   ocean fills to y {}", inspection.sea_level)),
        };
    });
    // Query only while the section is open, so a collapsed inspector costs
    // nothing.
    state.open = header.fully_open();
}
