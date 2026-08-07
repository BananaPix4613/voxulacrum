use std::path::PathBuf;
use bevy_ecs::prelude::Resource;

use crate::params::*;
use crate::meshing::MeshingStats;
use crate::ui::hierarchy_editor::HierarchyEditor;

pub struct ShaderLogEntry {
    pub message: String,
    pub is_error: bool,
    pub timestamp: std::time::Instant,
}

/// Transient UI state not persisted in EngineParams.
#[derive(Resource)]
pub struct UiState {
    pub params: EngineParams,
    pub change_detector: ParamChangeDetector,
    pub presets_dir: PathBuf,
    pub preset_list: Vec<String>,
    pub selected_preset: usize,
    pub save_name: String,
    pub selected_material: usize,
    pub selected_keyframe: usize,
    pub fps: f32,
    pub frame_time_ms: f32,
    /// Frames whose CPU work exceeded the budget in the last completed second.
    pub over_budget_frames: u32,
    /// Longest wall-clock inter-frame time in the last completed second, in ms.
    pub worst_frame_ms: f32,
    /// Worst CPU work (frame minus present block) last second, in ms.
    pub cpu_max_ms: f32,
    /// Mean time blocked on swapchain acquire + present last second, in ms.
    pub present_mean_ms: f32,
    /// Per-`FrameStage` CPU mean/max last second, in ms.
    pub stage_mean_ms: [f32; crate::diagnostics::STAGE_COUNT],
    pub stage_max_ms: [f32; crate::diagnostics::STAGE_COUNT],
    /// Generation-frontier sample (roadmap §7.3), mirrored from `FrameTimings`.
    /// Session-scoped, not per-second - see the note in `diagnostics.rs`.
    pub frontier_cpu_max_ms: f32,
    pub frontier_cpu_mean_ms: f32,
    pub frontier_frames: u32,
    pub frontier_over_budget: u32,
    pub frontier_worst_stages: [f32; crate::diagnostics::STAGE_COUNT],
    pub frontier_worst_sub: [f32; crate::diagnostics::SUB_COUNT],
    pub frontier_sub_max: [f32; crate::diagnostics::SUB_COUNT],
    pub frontier_sub_mean: [f32; crate::diagnostics::SUB_COUNT],
    /// Per-kind world-event counts, filled by a bus subscriber. This is the
    /// "emitted events" column post-phase-10 recorded as having no referent.
    pub event_counts: [u64; crate::world::events::WorldEvent::COUNT],
    pub events_total: u64,
    /// Column inspector request state and last report (roadmap §4.3).
    pub column_inspector: crate::ui::column_inspector::ColumnInspectorState,
    /// Biome/zone map preview state (roadmap §4.3).
    pub biome_map: crate::ui::biome_map::BiomeMapState,
    /// Blueprint capture / stamp panel state (roadmap §4.2).
    pub blueprint_panel: crate::ui::blueprint_panel::BlueprintPanelState,
    /// Set by the panel's Reset button; consumed at frame end.
    pub reset_frontier_requested: bool,
    /// Scheduler load per job kind (roadmap §4.4 job system inspector).
    pub job_stats: [crate::jobs::JobKindStats; crate::jobs::JobKind::COUNT],
    /// Jobs executing across all kinds, and the global limit.
    pub job_running_total: usize,
    pub job_max_running: usize,
    #[allow(dead_code)] // HUD stat; retained for the vertex-count readout
    pub total_vertices: u64,
    pub total_triangles: u64,
    pub chunks_visible: u32,
    pub chunks_total: u32,
    pub shader_log: Vec<ShaderLogEntry>,
    pub meshing_stats: MeshingStats,
    pub clear_cache_requested: bool,
    pub regenerate_requested: bool,
    pub remesh_requested: bool,
    pub mesh_params_pending: bool,
    pub regenerating: bool,
    pub remeshing: bool,
    pub regen_progress: (u32, u32),
    pub palette_list: Vec<String>,
    pub palette_load_requested: bool,
    pub loaded_palette_preview: Vec<[f32; 3]>,
    /// Streaming load + the §7.3 fill-time metric.
    pub streaming: crate::world::streaming::StreamingStats,
    /// Resident memory by layer (roadmap §4.4 memory/residency panel).
    pub residency: crate::diagnostics::ResidencyStats,
    /// Frames until the next residency sample. Sampling walks every resident
    /// chunk, so it runs on a throttle rather than per frame.
    pub residency_countdown: u32,
    /// Every command through the mutation door (roadmap §4.4).
    pub mutation_log: crate::world::mutation::MutationLog,
    /// Operator pressed "Verify determinism".
    pub determinism_check_requested: bool,
    pub determinism: crate::world::DeterminismReport,
}

impl UiState {
    pub fn new(params: EngineParams, presets_dir: PathBuf) -> Self {
        let change_detector = ParamChangeDetector::new(&params);
        let preset_list = EngineParams::list_presets(&presets_dir);
        Self {
            params,
            change_detector,
            presets_dir,
            preset_list,
            selected_preset: 0,
            save_name: "default".into(),
            selected_material: 1,
            selected_keyframe: 0,
            fps: 0.0,
            frame_time_ms: 0.0,
            over_budget_frames: 0,
            worst_frame_ms: 0.0,
            cpu_max_ms: 0.0,
            present_mean_ms: 0.0,
            stage_mean_ms: [0.0; crate::diagnostics::STAGE_COUNT],
            stage_max_ms: [0.0; crate::diagnostics::STAGE_COUNT],
            frontier_cpu_max_ms: 0.0,
            frontier_cpu_mean_ms: 0.0,
            frontier_frames: 0,
            frontier_over_budget: 0,
            frontier_worst_stages: [0.0; crate::diagnostics::STAGE_COUNT],
            frontier_worst_sub: [0.0; crate::diagnostics::SUB_COUNT],
            frontier_sub_max: [0.0; crate::diagnostics::SUB_COUNT],
            frontier_sub_mean: [0.0; crate::diagnostics::SUB_COUNT],
            event_counts: [0; crate::world::events::WorldEvent::COUNT],
            events_total: 0,
            column_inspector: Default::default(),
            blueprint_panel: Default::default(),
            biome_map: Default::default(),
            reset_frontier_requested: false,
            job_stats: [crate::jobs::JobKindStats::default(); crate::jobs::JobKind::COUNT],
            job_running_total: 0,
            job_max_running: 0,
            total_vertices: 0,
            total_triangles: 0,
            chunks_visible: 0,
            chunks_total: 0,
            shader_log: Vec::new(),
            meshing_stats: MeshingStats::default(),
            clear_cache_requested: false,
            regenerate_requested: false,
            remesh_requested: false,
            mesh_params_pending: false,
            regenerating: false,
            remeshing: false,
            regen_progress: (0, 0),
            palette_list: crate::palette::list_palettes(&crate::paths::asset_root().join("palettes")),
            palette_load_requested: false,
            loaded_palette_preview: Vec::new(),
            streaming: Default::default(),
            residency: Default::default(),
            residency_countdown: 0,
            mutation_log: Default::default(),
            determinism_check_requested: false,
            determinism: Default::default(),
        }
    }

    pub fn refresh_presets(&mut self) {
        self.preset_list = EngineParams::list_presets(&self.presets_dir);
    }
    
    pub fn push_shader_log(&mut self, message: String, is_error: bool) {
        self.shader_log.push(ShaderLogEntry {
            message,
            is_error,
            timestamp: std::time::Instant::now(),
        });
        // Keep last 50 entries
        if self.shader_log.len() > 50 {
            self.shader_log.remove(0);
        }
    }
}

fn change_dot(ui: &mut egui::Ui, color: egui::Color32, tooltip: &str) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
    response.on_hover_text(tooltip);
}

fn dot_green(ui: &mut egui::Ui) {
    change_dot(ui, egui::Color32::from_rgb(80, 200, 80), "Instant (uniform-only)");
}

fn dot_yellow(ui: &mut egui::Ui) {
    change_dot(ui, egui::Color32::from_rgb(220, 200, 60), "Requires remeshing");
}

fn dot_red(ui: &mut egui::Ui) {
    change_dot(ui, egui::Color32::from_rgb(220, 60, 60), "Requires world regeneration");
}

pub fn draw_engine_panel(ctx: &egui::Context, state: &mut UiState) {
    egui::SidePanel::right("engine_params_panel")
        .default_width(320.0)
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Engine Parameters");
                ui.separator();
                draw_time_control(ui, &mut state.params.time_control);
                ui.separator();
                draw_lighting(ui, &mut state.params.lighting, &mut state.selected_keyframe);
                ui.separator();
                draw_materials(ui, &mut state.params.materials, &mut state.selected_material, state.remeshing, state.mesh_params_pending);
                ui.separator();
                draw_wind(ui, &mut state.params.wind);
                ui.separator();
                draw_cloud(ui, &mut state.params.cloud);
                ui.separator();
                draw_post_process(ui, &mut state.params.post_process);
                ui.separator();
                draw_outline(ui, &mut state.params.outline);
                ui.separator();
                draw_palette(ui, &mut state.params.palette, &state.palette_list,
                             &state.loaded_palette_preview, &mut state.palette_load_requested);
                ui.separator();
                draw_render_pipeline(ui, &mut state.params.render_pipeline);
                ui.separator();
                draw_camera(ui, &mut state.params.camera);
                ui.separator();
                draw_cross_section(ui, &mut state.params.cross_section);
                ui.separator();
                draw_water(ui, &mut state.params.water);
                ui.separator();
                draw_meshing_params(ui, &mut state.params.meshing, state.remeshing, state.mesh_params_pending);
                ui.separator();
                draw_terrain_gen(
                    ui,
                    &mut state.params.terrain_gen,
                    state.regenerating,
                    state.regen_progress,
                    &mut state.regenerate_requested,
                );
                ui.separator();
                draw_debug(ui, &mut state.params.debug);
                ui.separator();
                draw_shader_log(ui, &mut state.shader_log);
                ui.separator();
                draw_meshing_section(
                    ui,
                    &state.meshing_stats,
                    &mut state.clear_cache_requested,
                    &mut state.remesh_requested,
                    state.mesh_params_pending,
                    state.remeshing,
                );
                ui.separator();
                crate::ui::column_inspector::draw(ui, &mut state.column_inspector);
                ui.separator();
                crate::ui::blueprint_panel::draw(ui, &mut state.blueprint_panel);
                if ui.button("Biome / Zone Map…").clicked() {
                    state.biome_map.open = true;
                    state.biome_map.dirty = true;
                }
                ui.separator();
                draw_performance(ui, state);
                ui.separator();
                draw_preset_controls(ui, state);
            });
        });
}

/// Left-side node-graph editor panel. Hidden by default; toggled with F2.
/// Renders the embedded `EditorState` canvas with an undo/redo/modified
/// toolbar. Phase 1: edits accumulate here (with undo) but do not yet
/// regenerate the world - that loop is wired in a later step.
pub fn draw_graph_editor_panel(ctx: &egui::Context, hierarchy: &mut HierarchyEditor) {
    let default_w = ctx.screen_rect().width() * 0.45;
    egui::SidePanel::left("graph_editor_panel")
        .default_width(default_w)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Graph Editor");

                let editor = hierarchy.editor_mut();
                let undo_label = editor.peek_undo().map(str::to_owned);
                if ui
                    .add_enabled(undo_label.is_some(), egui::Button::new("Undo"))
                    .on_hover_text(undo_label.as_deref().unwrap_or(""))
                    .clicked()
                {
                    editor.undo();
                }
                let redo_label = editor.peek_redo().map(str::to_owned);
                if ui
                    .add_enabled(redo_label.is_some(), egui::Button::new("Redo"))
                    .on_hover_text(redo_label.as_deref().unwrap_or(""))
                    .clicked()
                {
                    editor.redo();
                }

                ui.separator();

                let modified = hierarchy.is_modified();
                if ui
                    .add_enabled(modified, egui::Button::new("Save"))
                    .on_hover_text("Write this graph to its .json file")
                    .clicked()
                {
                    hierarchy.save_active();
                }
                if modified {
                    ui.colored_label(egui::Color32::YELLOW, "● modified");
                }
            });

            // Create / delete row.
            ui.horizontal(|ui| {
                ui.label("New biome:");
                ui.add(
                    egui::TextEdit::singleline(hierarchy.new_name_mut())
                        .desired_width(120.0)
                        .hint_text("name"),
                );
                if ui.button("+ Create").clicked() {
                    let name = hierarchy.new_name_mut().clone();
                    hierarchy.create_biome(&name);
                    hierarchy.new_name_mut().clear();
                }
                let deletable = hierarchy.active_is_deletable_biome();
                if ui
                    .add_enabled(deletable, egui::Button::new("Delete"))
                    .on_hover_text("Delete the selected biome graph and its manifest entry")
                    .clicked()
                {
                    hierarchy.delete_active();
                }
            });

            ui.horizontal(|ui| {
                ui.label("New zone:");
                ui.add(
                    egui::TextEdit::singleline(hierarchy.new_zone_name_mut())
                        .desired_width(120.0)
                        .hint_text("name"),
                );
                if ui.button("+ Create").clicked() {
                    let name = hierarchy.new_zone_name_mut().clone();
                    hierarchy.create_zone(&name);
                    hierarchy.new_zone_name_mut().clear();
                }
                ui.separator();
                let can_attach = hierarchy.active_biome_lacks_detail();
                if ui
                    .add_enabled(can_attach, egui::Button::new("+ Attach detail"))
                    .on_hover_text("Create and register a DetailGraph for the selected biome")
                    .clicked()
                {
                    hierarchy.attach_detail();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Rename selected:");
                ui.add(
                    egui::TextEdit::singleline(hierarchy.new_rename_mut())
                        .desired_width(120.0)
                        .hint_text("new name"),
                );
                let can_rename = hierarchy.active_is_renameable();
                if ui
                    .add_enabled(can_rename, egui::Button::new("Rename"))
                    .on_hover_text("Rename the selected biome or zone, moving its files and repointing the manifest")
                    .clicked()
                {
                    let name = hierarchy.new_rename_mut().clone();
                    hierarchy.rename_active(&name);
                    hierarchy.new_rename_mut().clear();
                }
            });

            draw_manifest_form(ui, hierarchy);

            if let Some(status) = hierarchy.take_status() {
                ui.colored_label(egui::Color32::LIGHT_BLUE, status);
            }

            ui.separator();
            hierarchy.show(ui);
        });
}

/// World-settings form: the manifest fields that are not graph nodes.
///
/// `sea_level` is here because the reference world needs a below-sea-level
/// biome and that value had no in-engine editor at all - it was the last scalar
/// requiring a text editor to change.
fn draw_manifest_form(ui: &mut egui::Ui, hierarchy: &mut HierarchyEditor) {
    let selected_biome = hierarchy.selected_biome_id();
    let Some((manifest, dirty, new_param)) = hierarchy.manifest_form() else {
        return;
    };
    ui.collapsing("World settings", |ui| {
        ui.horizontal(|ui| {
            ui.label("Seed:");
            if ui
                .add(egui::DragValue::new(&mut manifest.seed))
                .on_hover_text("World seed. Changing it regenerates every chunk.")
                .changed()
            {
                *dirty = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Sea level:");
            if ui
                .add(egui::DragValue::new(&mut manifest.sea_level).speed(1.0))
                .on_hover_text("Global ocean surface (world Y). Empty voxels at or below it fill with water.")
                .changed()
            {
                *dirty = true;
            }
        });

        let Some(bid) = selected_biome else {
            ui.weak("Select a biome to edit its parameters.");
            return;
        };
        let Some(entry) = manifest.biomes.iter_mut().find(|b| b.id == bid) else {
            return;
        };

        ui.separator();
        ui.weak(format!("Biome {bid} parameters:"));

        // Sorted: `params` is a HashMap, and unsorted iteration would reshuffle
        // the rows every frame - under the pointer, mid-drag.
        let mut names: Vec<String> = entry.params.keys().cloned().collect();
        names.sort();

        let mut remove: Option<String> = None;
        for name in &names {
            ui.horizontal(|ui| {
                ui.label(name);
                if let Some(v) = entry.params.get_mut(name) {
                    if ui.add(egui::DragValue::new(v).speed(0.05)).changed() {
                        *dirty = true;
                    }
                }
                if ui.small_button("×").on_hover_text("Remove parameter").clicked() {
                    remove = Some(name.clone());
                }
            });
        }
        if let Some(name) = remove {
            entry.params.remove(&name);
            *dirty = true;
        }

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(new_param)
                    .desired_width(160.0)
                    .hint_text("parameter name"),
            );
            let can_add = !new_param.trim().is_empty();
            if ui.add_enabled(can_add, egui::Button::new("+ Add")).clicked() {
                entry.params.insert(new_param.trim().to_string(), 0.0);
                new_param.clear();
                *dirty = true;
            }
        });
        ui.weak("e.g. traversal_smoothing_distance");
    });
}

fn draw_time_control(ui: &mut egui::Ui, p: &mut TimeControlParams) {
    ui.collapsing("Time Control", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut p.paused, "Paused"); });
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Speed:");
            ui.add(egui::Slider::new(&mut p.speed_multiplier, 0.0..=10.0));
        });
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Day duration (s):");
            ui.add(egui::DragValue::new(&mut p.day_duration_seconds).speed(1.0).range(10.0..=3600.0));
        });
        if p.paused {
            ui.horizontal(|ui| {
                dot_green(ui);
                ui.label("Time of day:");
                ui.add(egui::Slider::new(&mut p.manual_time, 0.0..=1.0));
            });
        }
    });
}

fn draw_lighting(ui: &mut egui::Ui, p: &mut LightingParams, selected: &mut usize) {
    ui.collapsing("Lighting", |ui| {
        let names: Vec<String> = p.keyframes.iter().enumerate()
            .map(|(i, kf)| format!("{}: t={:.2}", i, kf.time))
            .collect();

        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Keyframe:");
            egui::ComboBox::from_id_salt("keyframe_select")
                .selected_text(names.get(*selected).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        ui.selectable_value(selected, i, name);
                    }
                });
        });

        if let Some(kf) = p.keyframes.get_mut(*selected) {
            ui.horizontal(|ui| { dot_green(ui); ui.label("Time:"); ui.add(egui::Slider::new(&mut kf.time, 0.0..=1.0)); });
            ui.horizontal(|ui| { dot_green(ui); ui.label("Sun:"); ui.color_edit_button_rgb(&mut kf.sun_color); });
            ui.horizontal(|ui| { dot_green(ui); ui.label("Ambient:"); ui.color_edit_button_rgb(&mut kf.ambient_color); });
        }
    });
}

fn draw_materials(ui: &mut egui::Ui, p: &mut MaterialParams, selected: &mut usize, remeshing: bool, mesh_params_pending: bool) {
    ui.collapsing("Materials", |ui| {
        let names: Vec<String> = p.entries.iter().map(|e| e.name.clone()).collect();

        egui::ComboBox::from_id_salt("material_select")
            .selected_text(names.get(*selected).cloned().unwrap_or_default())
            .show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(selected, i, name);
                }
            });

        if let Some(mat) = p.entries.get_mut(*selected) {
            ui.add_enabled_ui(!remeshing, |ui| {
                ui.horizontal(|ui| { dot_yellow(ui); ui.label("Color:"); ui.color_edit_button_rgb(&mut mat.color); });
                ui.horizontal(|ui| { dot_yellow(ui); ui.label("Sharpness:"); ui.add(egui::Slider::new(&mut mat.sharpness, 0.0..=1.0)); });
            });
            // Green-dot params stay always-enabled
            ui.horizontal(|ui| { dot_green(ui); ui.label("Hardness:"); 0.0..=1.0 });
            ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut mat.permeable, "Permeable"); });
            ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut mat.supports_flora, "Supports flora"); });
        }
        if mesh_params_pending {
            ui.colored_label(egui::Color32::from_rgb(220, 200, 60), "Remesh required.");
        }
    });
}

fn draw_wind(ui: &mut egui::Ui, p: &mut WindParams) {
    ui.collapsing("Wind", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.label("Base magnitude:"); ui.add(egui::Slider::new(&mut p.base_magnitude, 0.0..=10.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Variation:"); ui.add(egui::Slider::new(&mut p.magnitude_variation, 0.0..=5.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Mag frequency:"); ui.add(egui::Slider::new(&mut p.magnitude_frequency, 0.001..=0.2)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Dir frequency:"); ui.add(egui::Slider::new(&mut p.direction_frequency, 0.001..=0.2)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Gust strength:"); ui.add(egui::Slider::new(&mut p.gust_strength, 0.0..=10.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Gust freq A:"); ui.add(egui::Slider::new(&mut p.gust_frequency_a, 0.1..=5.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Gust freq B:"); ui.add(egui::Slider::new(&mut p.gust_frequency_b, 0.1..=5.0)); });
    });
}

fn draw_cloud(ui: &mut egui::Ui, p: &mut CloudParams) {
    ui.collapsing("Clouds", |ui| {
        for (i, layer) in p.layers.iter_mut().enumerate() {
            ui.collapsing(format!("Layer {i}"), |ui| {
                ui.horizontal(|ui| { dot_green(ui); ui.label("Base coverage:"); ui.add(egui::Slider::new(&mut layer.base_coverage, 0.0..=1.0)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Variation:"); ui.add(egui::Slider::new(&mut layer.coverage_variation, 0.0..=0.5)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Coverage freq:"); ui.add(egui::Slider::new(&mut layer.coverage_frequency, 0.001..=0.2)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Wind dir offset:"); ui.add(egui::Slider::new(&mut layer.wind_dir_offset, -3.14..=3.14)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Wind speed scale:"); ui.add(egui::Slider::new(&mut layer.wind_speed_scale, 0.0..=3.0)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("UV scale:"); ui.add(egui::Slider::new(&mut layer.uv_scale, 0.0003..=0.0015)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Decay rate:"); ui.add(egui::Slider::new(&mut layer.decay_rate, 0.0..=1.0)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Default humidity:"); ui.add(egui::Slider::new(&mut layer.default_humidity, 0.0..=1.0)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Default condensation:"); ui.add(egui::Slider::new(&mut layer.default_condensation, 0.0..=1.0)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Water humidity boost:"); ui.add(egui::Slider::new(&mut layer.water_humidity_boost, 0.0..=0.5)); });
                ui.horizontal(|ui| { dot_green(ui); ui.label("Blob frequency:"); ui.add(egui::Slider::new(&mut layer.blob_frequency, 0.005..=0.05)); });
            });
        }
    });
}

fn draw_post_process(ui: &mut egui::Ui, p: &mut PostProcessParams) {
    ui.collapsing("Post Processing", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.label("Vignette:"); ui.add(egui::Slider::new(&mut p.vignette_strength, 0.0..=1.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Exposure:"); ui.add(egui::Slider::new(&mut p.exposure, 0.1..=3.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Overcast desat:"); ui.add(egui::Slider::new(&mut p.overcast_desaturation_factor, 0.0..=1.0)); });
    });
}

fn draw_outline(ui: &mut egui::Ui, p: &mut OutlineParams) {
    ui.collapsing("Outline", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut p.enabled, "Enabled"); });
        ui.separator();
        ui.label("Edge Detection");
        ui.horizontal(|ui| { dot_green(ui); ui.label("Depth threshold:"); ui.add(egui::Slider::new(&mut p.depth_threshold, 0.0..=0.1)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Depth strength:"); ui.add(egui::Slider::new(&mut p.depth_strength, 0.0..=2.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Normal threshold:"); ui.add(egui::Slider::new(&mut p.normal_threshold, 0.0..=1.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Normal strength:"); ui.add(egui::Slider::new(&mut p.normal_strength, 0.0..=2.0)); });
        ui.separator();
        ui.label("Shading");
        ui.horizontal(|ui| { dot_green(ui); ui.label("Darken strength:"); ui.add(egui::Slider::new(&mut p.darken_strength, 0.0..=1.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Brighten strength:"); ui.add(egui::Slider::new(&mut p.brighten_strength, 0.0..=1.0)); });
    });
}

fn draw_palette(
    ui: &mut egui::Ui,
    p: &mut PaletteParams,
    palette_list: &[String],
    preview_colors: &[[f32; 3]],
    load_requested: &mut bool,
) {
    ui.collapsing("Palette", |ui| {
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.checkbox(&mut p.enabled, "Enabled");
        });

        // Mode selector
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Mode:");
            ui.selectable_value(&mut p.mode, 0, "Palette");
            ui.selectable_value(&mut p.mode, 1, "Stepping");
        });

        if p.mode == 0 {
            // Palette lookup mode
            if !palette_list.is_empty() {
                ui.horizontal(|ui| {
                    dot_green(ui);
                    ui.label("Palette:");
                    let prev = p.selected_palette.clone();
                    egui::ComboBox::from_id_salt("palette_select")
                        .selected_text(if p.selected_palette.is_empty() {
                            "(none)"
                        } else {
                            &p.selected_palette
                        })
                        .show_ui(ui, |ui| {
                            for name in palette_list {
                                ui.selectable_value(
                                    &mut p.selected_palette,
                                    name.clone(),
                                    name,
                                );
                            }
                        });
                    if p.selected_palette != prev {
                        *load_requested = true;
                    }
                });
            } else {
                ui.label("No palettes in palettes/");
            }

            if !preview_colors.is_empty() {
                ui.add_space(4.0);
                ui.label(format!("{} colors:", preview_colors.len()));
                ui.horizontal_wrapped(|ui| {
                    for &color in preview_colors {
                        let c = egui::Color32::from_rgb(
                            (color[0] * 255.0) as u8,
                            (color[1] * 255.0) as u8,
                            (color[2] * 255.0) as u8,
                        );
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(16.0, 16.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(rect, 2.0, c);
                    }
                });
            }
        } else {
            // Color stepping mode
            let mut l = p.l_levels as i32;
            let mut ab = p.ab_levels as i32;
            ui.horizontal(|ui| {
                dot_green(ui);
                ui.label("L levels:");
                ui.add(egui::Slider::new(&mut l, 2..=128));
            });
            ui.horizontal(|ui| {
                dot_green(ui);
                ui.label("ab levels:");
                ui.add(egui::Slider::new(&mut ab, 2..=128));
            });
            p.l_levels = l as u32;
            p.ab_levels = ab as u32;
            ui.horizontal(|ui| {
                dot_green(ui);
                ui.label("L gamma:");
                ui.add(egui::Slider::new(&mut p.l_gamma, 0.1..=1.0));
            });
        }
    });
}

fn draw_render_pipeline(ui: &mut egui::Ui, p: &mut RenderPipelineParams) {
    ui.collapsing("Render Pipeline", |ui| {
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Pixel scale:");
            ui.add(egui::Slider::new(&mut p.world_pixel_density, 1.0..=50.0));
        });
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Camera snap:");
            ui.add(egui::Checkbox::without_text(&mut p.camera_snap_enabled));
        });
        ui.separator();
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Sky color:");
            ui.color_edit_button_rgb(&mut p.sky_color);
        });
    });
}

fn draw_camera(ui: &mut egui::Ui, p: &mut CameraParams) {
    ui.collapsing("Camera", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.label("Pan speed:"); ui.add(egui::Slider::new(&mut p.pan_speed, 1.0..=50.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Scroll speed:"); ui.add(egui::Slider::new(&mut p.scroll_speed, 0.5..=10.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Zoom min:"); ui.add(egui::DragValue::new(&mut p.zoom_min).speed(0.5).range(1.0..=p.zoom_max)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Zoom max:"); ui.add(egui::DragValue::new(&mut p.zoom_max).speed(0.5).range(p.zoom_min..=200.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Smooth speed:"); ui.add(egui::Slider::new(&mut p.smooth_speed, 1.0..=120.0).suffix(" /s")); });
    });
}

fn draw_cross_section(ui: &mut egui::Ui, p: &mut CrossSectionParams) {
    ui.collapsing("Cross-Section", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut p.enabled, "Enabled"); });
        if p.enabled {
            ui.horizontal(|ui| {
                dot_green(ui); ui.label("X clip:");
                ui.add(egui::Slider::new(&mut p.x_offset, 0.0..=250.0));
            });
            ui.horizontal(|ui| {
                dot_green(ui); ui.label("Y clip:");
                ui.add(egui::Slider::new(&mut p.y_offset, 0.0..=250.0));
            });
            ui.horizontal(|ui| {
                dot_green(ui); ui.label("Z clip:");
                ui.add(egui::Slider::new(&mut p.z_offset, 0.0..=250.0));
            });
            ui.separator();
            ui.horizontal(|ui| {
                dot_green(ui); ui.label("Fog density:");
                ui.add(egui::Slider::new(&mut p.fog_density, 0.0..=10.0));
            });
            ui.horizontal(|ui| {
                dot_green(ui); ui.label("Fog color:");
                ui.color_edit_button_rgb(&mut p.fog_color);
            });
            ui.horizontal(|ui| {
                dot_green(ui); ui.checkbox(&mut p.show_edges, "Show clip edges");
            });
        }
    });
}

fn draw_water(ui: &mut egui::Ui, p: &mut WaterVisualParams) {
    ui.collapsing("Water", |ui| {
        ui.horizontal(|ui| { dot_red(ui); ui.label("Water level:"); ui.add(egui::Slider::new(&mut p.water_level, 0.0..=64.0)); });
    });
}

fn draw_meshing_params(ui: &mut egui::Ui, _p: &mut MeshingParams, remeshing: bool, mesh_params_pending: bool) {
    ui.collapsing("Meshing", |ui| {
        ui.add_enabled_ui(!remeshing, |ui| {
            ui.label("(no cube-mesh tuning knobs yet)");
        });
        if mesh_params_pending {
            ui.colored_label(egui::Color32::from_rgb(220, 200, 60), "Remesh required.");
        }
        if remeshing {
            ui.colored_label(
                egui::Color32::from_rgb(220, 200, 60),
                "Remeshing in progress...",
            );
        }
        ui.separator();
        // Green-dot params stay always-enabled
        ui.horizontal(|ui| { dot_green(ui); ui.label("Edge strength:"); 0.0..=0.5 });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Ortho AO:"); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Ortho AO strength:"); 0.0..=0.7 });
    });
}

fn draw_terrain_gen(
    ui: &mut egui::Ui,
    p: &mut TerrainGenParams,
    regenerating: bool,
    regen_progress: (u32, u32),
    regenerate_requested: &mut bool,
) {
    ui.collapsing("Terrain Generation", |ui| {
        // Disable param sliders while regenerating (scoped via add_enabled_ui)
        ui.add_enabled_ui(!regenerating, |ui| {
            ui.horizontal(|ui| { dot_red(ui); ui.label("Base height:"); ui.add(egui::Slider::new(&mut p.base_height, 0.0..=64.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cliff threshold:"); ui.add(egui::Slider::new(&mut p.cliff_threshold, 0.5..=5.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Hill amplitude:"); ui.add(egui::Slider::new(&mut p.hill_amplitude, 0.0..=50.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Hill frequency:"); ui.add(egui::Slider::new(&mut p.hill_frequency, 0.0001..=0.05).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Ridge amplitude:"); ui.add(egui::Slider::new(&mut p.ridge_amplitude, 0.0..=30.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Ridge frequency:"); ui.add(egui::Slider::new(&mut p.ridge_frequency, 0.001..=0.1).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Detail amplitude:"); ui.add(egui::Slider::new(&mut p.detail_amplitude, 0.0..=10.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Detail frequency:"); ui.add(egui::Slider::new(&mut p.detail_frequency, 0.001..=0.2).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.separator();
            ui.label("Cave System");
            ui.horizontal(|ui| { dot_red(ui); ui.label("Caves enabled:"); ui.add(egui::Checkbox::without_text(&mut p.cave_enabled)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Spaghetti freq:"); ui.add(egui::Slider::new(&mut p.cave_spaghetti_freq, 0.005..=0.1).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Spaghetti thickness:"); ui.add(egui::Slider::new(&mut p.cave_spaghetti_thickness, 0.01..=0.3).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Noodle freq:"); ui.add(egui::Slider::new(&mut p.cave_noodle_freq, 0.001..=0.2).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Noodle thickness:"); ui.add(egui::Slider::new(&mut p.cave_noodle_thickness, 0.01..=0.5).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cheese freq:"); ui.add(egui::Slider::new(&mut p.cave_cheese_freq, 0.002..=0.03).logarithmic(true).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cheese threshold:"); ui.add(egui::Slider::new(&mut p.cave_cheese_threshold, 0.1..=0.9).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Warp amplitude:"); ui.add(egui::Slider::new(&mut p.cave_warp_amp, 0.0..=80.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Surface margin:"); ui.add(egui::Slider::new(&mut p.cave_surface_margin, 0.1..=10.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Y squash:"); ui.add(egui::Slider::new(&mut p.cave_y_squash, 0.1..=2.0).clamping(egui::SliderClamping::Never)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Water level (gen):"); ui.add(egui::Slider::new(&mut p.water_level, 0.0..=64.0).clamping(egui::SliderClamping::Never)); });
        });

        // Button/progress area (always enabled, outside the add_enabled_ui scope)
        ui.add_space(4.0);

        if regenerating {
            let (done, total) = regen_progress;
            let fraction = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .text(format!("Regenerating... {}/{}", done, total))
                    .animate(true),
            );
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(220, 60, 60),
                "Changes require world regeneration.",
            );
            if ui.button("Regenerate World").clicked() {
                *regenerate_requested = true;
            }
        }
    });
}

fn draw_debug(ui: &mut egui::Ui, p: &mut DebugParams) {
    ui.collapsing("Debug Overlays", |ui| {
        ui.checkbox(&mut p.show_wireframe, "Wireframe");
        ui.checkbox(&mut p.show_chunk_boundaries, "Chunk boundaries");
        ui.checkbox(&mut p.hide_water, "Hide water");
        ui.checkbox(&mut p.hide_foliage, "Hide foliage");

        ui.separator();
        ui.label("Shader debug (mutually exclusive):");

        if ui.checkbox(&mut p.show_material_ids, "Material IDs").changed() && p.show_material_ids {
            p.show_ao_only = false;
            p.show_normals = false;
        }
        if ui.checkbox(&mut p.show_ao_only, "AO only").changed() && p.show_ao_only {
            p.show_material_ids = false;
            p.show_normals = false;
        }
        if ui.checkbox(&mut p.show_normals, "Normals").changed() && p.show_normals {
            p.show_material_ids = false;
            p.show_ao_only = false;
        }
        if ui.checkbox(&mut p.show_greedy_debug, "Greedy merge").changed() && p.show_greedy_debug {
            p.show_material_ids = false;
            p.show_ao_only = false;
            p.show_normals = false;
        }

        ui.separator();
        ui.checkbox(&mut p.show_water_debug, "Water debug");
        ui.checkbox(&mut p.show_performance, "Show performance");
        ui.checkbox(&mut p.freeze_culling, "Freeze culling");
    });
}

fn draw_shader_log(ui: &mut egui::Ui, log: &mut Vec<ShaderLogEntry>) {
    ui.collapsing("Shader Log", |ui| {
        if log.is_empty() {
            ui.label("No shader reload events.");
            return;
        }
        
        if ui.button("Clear Log").clicked() {
            log.clear();
            return;
        }
        
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in log.iter() {
                    let elapsed = entry.timestamp.elapsed().as_secs();
                    let time_str = if elapsed < 60 {
                        format!("{}s ago", elapsed)
                    } else {
                        format!("{}m ago", elapsed / 60)
                    };
                    
                    let color = if entry.is_error {
                        egui::Color32::from_rgb(255, 100, 100)
                    } else {
                        egui::Color32::from_rgb(100, 255, 100)
                    };
                    
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(150, 150, 150),
                            &time_str,
                        );
                        ui.colored_label(color, &entry.message);
                    });
                }
            });
    });
}

fn draw_meshing_section(
    ui: &mut egui::Ui,
    stats: &MeshingStats,
    clear_cache: &mut bool,
    remesh_requested: &mut bool,
    mesh_params_pending: bool,
    remeshing: bool,
) {
    ui.collapsing("Meshing Pipeline", |ui| {
        ui.label(format!("Workers: {}", stats.worker_count));
        if stats.pending_submissions > 0 {
            ui.label(format!("Pending: {}", stats.pending_submissions));
        }
        ui.label(format!("In progress: {}", stats.in_progress));
        ui.label(format!("Total meshed: {}", stats.total_meshed));
        if stats.last_batch_time_ms > 0.0 {
            ui.label(format!("Last batch: {:.0}ms", stats.last_batch_time_ms));
        }
        let active = stats.in_progress;
        if active > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(80, 200, 80),
                format!("Meshing... ({} active)", active),
            );
        }

        // Remesh controls
        ui.separator();
        if mesh_params_pending {
            ui.colored_label(
                egui::Color32::from_rgb(220, 200, 60),
                "Mesh parameters changed — remesh required.",
            );
        }
        ui.add_enabled_ui(!remeshing, |ui| {
            if ui.button("Remesh").clicked() {
                *remesh_requested = true;
            }
        });

        // Cache stats
        ui.separator();
        ui.label("Disk Cache");
        let total_lookups = stats.cache_hits + stats.cache_misses;
        if total_lookups > 0 {
            let hit_rate = stats.cache_hits as f64 / total_lookups as f64 * 100.0;
            ui.label(format!(
                "Hits: {} | Misses: {} ({:.0}% hit rate)",
                stats.cache_hits, stats.cache_misses, hit_rate,
            ));
        } else {
            ui.label(format!(
                "Hits: {} | Misses: {}",
                stats.cache_hits, stats.cache_misses,
            ));
        }
        if stats.cache_errors > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(255, 100, 100),
                format!("Errors: {}", stats.cache_errors),
            );
        }
        let looked_up = stats.cache_hits + stats.cache_misses;
        if looked_up > 0 {
            ui.label(format!(
                "Hit rate: {:.0}%   misses: {} cold, {} stale",
                stats.cache_hits as f64 / looked_up as f64 * 100.0,
                format_number(stats.cache_misses_cold),
                format_number(stats.cache_misses_stale),
            ));
        }
        ui.label(format!(
            "On disk: {} files ({:.1} MB)",
            stats.cache_files,
            stats.cache_bytes as f64 / (1024.0 * 1024.0),
        ));
        if ui.button("Clear Cache").clicked() {
            *clear_cache = true;
        }
    });
}

fn draw_performance(ui: &mut egui::Ui, state: &mut UiState) {
    if !state.params.debug.show_performance { return; }
    ui.collapsing("Performance", |ui| {
        ui.label(format!("FPS: {:.0}", state.fps));
        ui.label(format!(
            "CPU worst: {:.1}ms / {:.1} budget — {} frames over",
            state.cpu_max_ms,
            crate::diagnostics::FRAME_BUDGET_MS,
            state.over_budget_frames,
        ));
        ui.label(format!(
            "Present mean: {:.1}ms    Wall worst: {:.1}ms",
            state.present_mean_ms, state.worst_frame_ms,
        ));
        ui.separator();
        ui.label("Stage CPU mean / max (ms)");
        for (i, name) in crate::diagnostics::STAGE_NAMES.iter().enumerate() {
            ui.label(format!(
                "  {:<8}{:>6.2} /{:>6.2}",
                name, state.stage_mean_ms[i], state.stage_max_ms[i],
            ));
        }
        ui.separator();
        // Session-scoped, unlike everything above it, and labeled so nobody
        // reads it as a one-second figure.
        ui.horizontal(|ui| {
            ui.label("Generation frontier (session)");
            if ui.button("Reset").clicked() {
                state.reset_frontier_requested = true;
            }
        });
        if state.frontier_frames == 0 {
            ui.label("  no frontier frames sampled — move the camera to open a deficit");
        } else {
            ui.label(format!(
                "  CPU worst: {:.1}ms / {:.1} budget — {} of {} frames over",
                state.frontier_cpu_max_ms,
                crate::diagnostics::FRONTIER_BUDGET_MS,
                state.frontier_over_budget,
                state.frontier_frames,
            ));
            ui.label(format!("  CPU mean:  {:.1}ms", state.frontier_cpu_mean_ms));
            let split: Vec<String> = crate::diagnostics::STAGE_NAMES
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{n} {:.1}", state.frontier_worst_stages[i]))
                .collect();
            ui.label(format!("  worst frame: {}", split.join("  ")));
            // Per-span across ALL frontier frames, not the worst frame's split:
            // spans peak on different frames, so one frame's split names an
            // owner by accident. Rows 0-3 are the stage's systems; rows 4-7
            // decompose row 0.
            ui.label("  span        mean     max    (all frontier frames)");
            for (i, n) in crate::diagnostics::SUB_NAMES.iter().enumerate() {
                // Rows 4-7 decompose row 0; 8-11 are separate stages' systems.
                let indent = if (4..8).contains(&i) { "    " } else { "  " };
                ui.label(format!(
                    "{}{:<10}{:>6.2}{:>8.2}",
                    indent, n, state.frontier_sub_mean[i], state.frontier_sub_max[i],
                ));
            }
        }
        ui.separator();
        ui.label(format!(
            "Jobs — {}/{} threads busy      queued  running  limit  mean/max ms  done",
            state.job_running_total, state.job_max_running,
        ));
        for kind in crate::jobs::JobKind::ALL {
            let s = &state.job_stats[kind.index()];
            ui.label(format!(
                "  {:<9}{:>4}{:>7}/{:<3}{:>6.1}/{:<6.1}{:>8}",
                kind.name(),
                s.queued,
                s.in_flight,
                s.cap,
                s.mean_ms,
                s.max_ms,
                format_number(s.completed),
            ));
        }
        ui.separator();
        ui.label(format!("Triangles: {}", format_number(state.total_triangles)));
        ui.label(format!("Chunks: {}/{}", state.chunks_visible, state.chunks_total));
        if state.chunks_total > 0 {
            let cull_pct = (1.0 - state.chunks_visible as f64 / state.chunks_total as f64) * 100.0;
            ui.label(format!("Culled: {:.0}%", cull_pct));
        }
        ui.separator();
        let log = &state.mutation_log;
        let rejected = log.rejected_wrong_mode + log.rejected_system_origin;
        // Zero is the invariant, not merely the expected value: a rejection means
        // a call site built a command the door refuses, and P5 says a write path
        // that bypasses the door is a bug by definition.
        if rejected == 0 {
            ui.label("Mutations — 0 rejected");
        } else {
            ui.label(format!(
                "Mutations — {} REJECTED ({} wrong-mode, {} system-origin)",
                rejected, log.rejected_wrong_mode, log.rejected_system_origin,
            ));
        }
        if state.events_total == 0 {
            ui.label("Events — none yet");
        } else {
            let kinds: Vec<String> = crate::world::events::WorldEvent::NAMES
                .iter()
                .enumerate()
                .filter(|(i, _)| state.event_counts[*i] > 0)
                .map(|(i, n)| format!("{n} {}", format_number(state.event_counts[i])))
                .collect();
            ui.label(format!("Events — {}", kinds.join("   ")));
        }
        egui::ScrollArea::vertical()
            .id_salt("mutation_log")
            .max_height(150.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, name) in crate::world::mutation::INTENT_NAMES.iter().enumerate() {
                    let row = log.counts[i];
                    if row.iter().all(|c| *c == 0) {
                        continue;
                    }
                    let mut cells = String::new();
                    for origin in [
                        crate::world::mutation::MutationOrigin::Authoring,
                        crate::world::mutation::MutationOrigin::PlayTime,
                        crate::world::mutation::MutationOrigin::System,
                    ] {
                        let n = row[origin.index()];
                        if n > 0 {
                            cells.push_str(&format!("  {} {}", origin.label(), format_number(n)));
                        }
                    }
                    ui.label(format!("  {:<22}{}", name, cells));
                }

                let mut any_recent = false;
                for r in log.recent_newest_first() {
                    if !any_recent {
                        ui.label("Recent actor commands (newest first)");
                        any_recent = true;
                    }
                    ui.label(format!(
                        "  {:<18}{:<6}{:<10}mesh {}  scatter {}  water {}",
                        r.intent,
                        r.origin.label(),
                        format!("{:?}", r.mode).to_lowercase(),
                        r.mesh_invalidated,
                        r.scatter_rebuild,
                        r.water_rebuild,
                    ));
                }
            });
        ui.separator();
        let s = &state.streaming;
        ui.label(format!(
            "Streaming — resident {}   wanted {}   missing {}   in-flight {}",
            s.resident, s.wanted, s.missing, s.in_flight,
        ));
        // The §7.3 budget: visible area fully populated within 2 s of camera
        // rest, at any supported zoom. Flagged rather than merely displayed,
        // because a budget nobody can see the violation of is not a budget.
        const FILL_BUDGET_MS: f32 = 2000.0;
        let fill = if s.filling {
            format!("filling… ({} to go)", s.missing)
        } else if s.fill_chunks == 0 {
            "fill — (no settle measured yet)".to_string()
        } else if s.fill_ms > FILL_BUDGET_MS {
            format!(
                "fill {:.0}ms for {} chunks  OVER BUDGET ({:.0}ms)",
                s.fill_ms, s.fill_chunks, FILL_BUDGET_MS,
            )
        } else {
            format!(
                "fill {:.0}ms for {} chunks / {:.0}ms budget",
                s.fill_ms, s.fill_chunks, FILL_BUDGET_MS,
            )
        };
        ui.label(format!(
            "  radius {} chunks   evicted {}   {}",
            s.load_radius,
            format_number(s.evicted_total),
            fill,
        ));

        ui.separator();
        let r = &state.residency;
        let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
        ui.label(format!(
            "Memory — {:.1} MB CPU + {:.1} MB GPU   ({} chunks, {} uniform)",
            mb(r.cpu_bytes()),
            mb(r.gpu_mesh_bytes),
            r.chunks,
            r.uniform_chunks,
        ));
        ui.label(format!(
            "  voxel {:.1}   detail {:.1}   scatter {:.1}   fluid {:.1}   overrides {:.2}  [MB]",
            mb(r.voxel_bytes),
            mb(r.detail_bytes),
            mb(r.scatter_bytes),
            mb(r.fluid_bytes),
            mb(r.override_bytes),
        ));
        // §7.3: "session growth flat after warm-up". A peak that keeps rising
        // while you revisit the same ground is the leak signal.
        ui.label(format!(
            "  session peak {:.1} MB CPU / {:.1} MB GPU",
            mb(r.peak_cpu_bytes),
            mb(r.peak_gpu_bytes),
        ));

        ui.separator();
        if ui.button("Verify determinism (32 chunks)").clicked() {
            state.determinism_check_requested = true;
        }
        let d = state.determinism;
        if d.ran {
            if d.divergent > 0 {
                ui.label(format!(
                    "DETERMINISM FAILED — {}/{} chunks diverged",
                    d.divergent, d.checked,
                ));
                if let Some(f) = d.first {
                    ui.label(format!(
                        "  first: chunk {:?} local ({}, {}, {})",
                        f.chunk, f.local.x, f.local.y, f.local.z,
                    ));
                    ui.label(format!("  resident  {:?}", f.resident));
                    ui.label(format!("  regen     {:?}", f.regenerated));
                }
            } else if d.checked == 0 {
                // Distinguish "verified" from "had nothing to verify".
                ui.label(format!(
                    "Determinism: nothing comparable ({} skipped — all carry overrides)",
                    d.skipped,
                ));
            } else {
                ui.label(format!(
                    "Determinism OK — {} chunks identical, {} skipped",
                    d.checked, d.skipped,
                ));
            }
        }
    });
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

fn draw_preset_controls(ui: &mut egui::Ui, state: &mut UiState) {
    ui.collapsing("Presets", |ui| {
        if !state.preset_list.is_empty() {
            ui.horizontal(|ui| {
                ui.label("Preset:");
                egui::ComboBox::from_id_salt("preset_select")
                    .selected_text(
                        state.preset_list.get(state.selected_preset)
                            .cloned().unwrap_or_else(|| "(none)".into()),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in state.preset_list.iter().enumerate() {
                            ui.selectable_value(&mut state.selected_preset, i, name);
                        }
                    });
            });

            ui.add_enabled_ui(!state.remeshing, |ui| {
                if ui.button("Load Selected").clicked() {
                    if let Some(name) = state.preset_list.get(state.selected_preset) {
                        let path = state.presets_dir.join(format!("{}.json", name));
                        match EngineParams::load(&path) {
                            Ok(loaded) => {
                                state.params = loaded;
                                log::info!("Loaded preset: {}", name);
                            }
                            Err(e) => log::error!("Failed to load preset {}: {}", name, e),
                        }
                    }
                }
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut state.save_name);
        });

        if ui.button("Save Preset").clicked() {
            let path = state.presets_dir.join(format!("{}.json", state.save_name));
            match state.params.save(&path) {
                Ok(()) => {
                    log::info!("Saved preset: {}", state.save_name);
                    state.refresh_presets();
                }
                Err(e) => log::error!("Failed to save preset: {}", e),
            }
        }

        if ui.button("Refresh List").clicked() {
            state.refresh_presets();
        }

        if ui.button("Reset to Defaults").clicked() {
            state.params = EngineParams::default();
        }
    });
}

/// Screen-anchored indicator shown while a blueprint tool is armed. Not
/// interactable - it is a mode readout, and it must never eat the click it is
/// telling the operator to make.
pub fn draw_armed_hud(ctx: &egui::Context, label: &str) {
    egui::Area::new(egui::Id::new("blueprint_armed_hud"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.colored_label(egui::Color32::LIGHT_YELLOW, label);
                ui.weak("Esc or right-click to cancel");
            });
        });
}

/// Play-mode HUD (Substep 12): the selected build material readout.
pub fn draw_play_hud(ctx: &egui::Context, material_name: &str, material_color: [f32; 3]) {
    egui::Area::new(egui::Id::new("play_hud_material"))
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, -12.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    2.0,
                    egui::Color32::from_rgb(
                        (material_color[0] * 255.0) as u8,
                        (material_color[1] * 255.0) as u8,
                        (material_color[2] * 255.0) as u8,
                    ),
                );
                ui.label(material_name);
            });
        });
}
