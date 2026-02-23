use crate::params::*;
use crate::meshing::MeshingStats;
use std::path::PathBuf;

pub struct ShaderLogEntry {
    pub message: String,
    pub is_error: bool,
    pub timestamp: std::time::Instant,
}

/// Transient UI state not persisted in EngineParams.
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
    pub total_vertices: u64,
    pub total_triangles: u64,
    pub chunks_visible: u32,
    pub chunks_total: u32,
    pub shader_log: Vec<ShaderLogEntry>,
    pub meshing_stats: MeshingStats,
    pub clear_cache_requested: bool,
    pub regenerate_requested: bool,
    pub regenerating: bool,
    pub regen_progress: (u32, u32),
    pub palette_list: Vec<String>,
    pub palette_load_requested: bool,
    pub loaded_palette_preview: Vec<[f32; 3]>,
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
            total_vertices: 0,
            total_triangles: 0,
            chunks_visible: 0,
            chunks_total: 0,
            shader_log: Vec::new(),
            meshing_stats: MeshingStats::default(),
            clear_cache_requested: false,
            regenerate_requested: false,
            regenerating: false,
            regen_progress: (0, 0),
            palette_list: crate::palette::list_palettes(&std::path::PathBuf::from("palettes")),
            palette_load_requested: false,
            loaded_palette_preview: Vec::new(),
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
                draw_materials(ui, &mut state.params.materials, &mut state.selected_material);
                ui.separator();
                draw_wind(ui, &mut state.params.wind);
                ui.separator();
                draw_cloud(ui, &mut state.params.cloud);
                ui.separator();
                draw_post_process(ui, &mut state.params.post_process);
                ui.separator();
                draw_palette(ui, &mut state.params.palette, &state.palette_list,
                             &state.loaded_palette_preview, &mut state.palette_load_requested);
                ui.separator();
                draw_render_pipeline(ui, &mut state.params.render_pipeline);
                ui.separator();
                draw_camera(ui, &mut state.params.camera);
                ui.separator();
                draw_water(ui, &mut state.params.water);
                ui.separator();
                draw_vegetation(ui, &mut state.params.vegetation);
                ui.separator();
                draw_meshing_params(ui, &mut state.params.meshing);
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
                draw_meshing_section(ui, &state.meshing_stats, &mut state.clear_cache_requested);
                ui.separator();
                draw_performance(ui, state);
                ui.separator();
                draw_preset_controls(ui, state);
            });
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

fn draw_materials(ui: &mut egui::Ui, p: &mut MaterialParams, selected: &mut usize) {
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
            ui.horizontal(|ui| { dot_yellow(ui); ui.label("Color:"); ui.color_edit_button_rgb(&mut mat.color); });
            ui.horizontal(|ui| { dot_yellow(ui); ui.label("Sharpness:"); ui.add(egui::Slider::new(&mut mat.sharpness, 0.0..=1.0)); });
            ui.horizontal(|ui| { dot_green(ui); ui.label("Hardness:"); ui.add(egui::Slider::new(&mut mat.hardness, 0.0..=1.0)); });
            ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut mat.permeable, "Permeable"); });
            ui.horizontal(|ui| { dot_green(ui); ui.checkbox(&mut mat.supports_flora, "Supports flora"); });
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
        ui.horizontal(|ui| { dot_green(ui); ui.label("Base coverage:"); ui.add(egui::Slider::new(&mut p.base_coverage, 0.0..=1.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Variation:"); ui.add(egui::Slider::new(&mut p.coverage_variation, 0.0..=0.5)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Coverage freq:"); ui.add(egui::Slider::new(&mut p.coverage_frequency, 0.001..=0.2)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Scroll speed:"); ui.add(egui::Slider::new(&mut p.scroll_speed, 0.0..=0.05)); });
    });
}

fn draw_post_process(ui: &mut egui::Ui, p: &mut PostProcessParams) {
    ui.collapsing("Post Processing", |ui| {
        ui.horizontal(|ui| { dot_green(ui); ui.label("Vignette:"); ui.add(egui::Slider::new(&mut p.vignette_strength, 0.0..=1.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Exposure:"); ui.add(egui::Slider::new(&mut p.exposure, 0.1..=3.0)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Overcast desat:"); ui.add(egui::Slider::new(&mut p.overcast_desaturation_factor, 0.0..=1.0)); });
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
        }
    });
}

fn draw_render_pipeline(ui: &mut egui::Ui, p: &mut RenderPipelineParams) {
    ui.collapsing("Render Pipeline", |ui| {
        ui.horizontal(|ui| {
            dot_green(ui);
            ui.label("Pixel scale:");
            ui.add(egui::Slider::new(&mut p.world_pixel_density, 1.0..=20.0));
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
    });
}

fn draw_water(ui: &mut egui::Ui, p: &mut WaterVisualParams) {
    ui.collapsing("Water", |ui| {
        ui.horizontal(|ui| { dot_red(ui); ui.label("Water level:"); ui.add(egui::Slider::new(&mut p.water_level, 0.0..=64.0)); });
    });
}

fn draw_vegetation(ui: &mut egui::Ui, p: &mut VegetationParams) {
    ui.collapsing("Vegetation", |ui| {
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Blade half-width:"); ui.add(egui::Slider::new(&mut p.blade_half_width, 0.01..=0.3)); });
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Blade height:"); ui.add(egui::Slider::new(&mut p.blade_height, 0.1..=2.0)); });
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Blades/voxel:"); ui.add(egui::DragValue::new(&mut p.blades_per_voxel).range(1..=10)); });
    });
}

fn draw_meshing_params(ui: &mut egui::Ui, p: &mut MeshingParams) {
    ui.collapsing("Meshing", |ui| {
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Greedy merge:"); ui.add(egui::Checkbox::without_text(&mut p.greedy_merge_enabled)); });
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Flat error threshold:"); ui.add(egui::Slider::new(&mut p.flat_threshold_error, 0.001..=0.1)); });
        ui.horizontal(|ui| { dot_yellow(ui); ui.label("Flat normal threshold:"); ui.add(egui::Slider::new(&mut p.flat_normal_threshold, 0.8..=1.0)); });
        ui.separator();
        ui.horizontal(|ui| { dot_green(ui); ui.label("Edge strength:"); ui.add(egui::Slider::new(&mut p.edge_strength, 0.0..=0.5)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Ortho AO:"); ui.add(egui::Checkbox::without_text(&mut p.ortho_ao_enabled)); });
        ui.horizontal(|ui| { dot_green(ui); ui.label("Ortho AO strength:"); ui.add(egui::Slider::new(&mut p.ortho_ao_strength, 0.0..=0.7)); });
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
            ui.horizontal(|ui| { dot_red(ui); ui.label("Seed:"); ui.add(egui::DragValue::new(&mut p.seed)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Base height:"); ui.add(egui::Slider::new(&mut p.base_height, 0.0..=64.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cliff threshold:"); ui.add(egui::Slider::new(&mut p.cliff_threshold, 0.5..=5.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Hill amplitude:"); ui.add(egui::Slider::new(&mut p.hill_amplitude, 0.0..=50.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Hill frequency:"); ui.add(egui::Slider::new(&mut p.hill_frequency, 0.0001..=0.05).logarithmic(true)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Ridge amplitude:"); ui.add(egui::Slider::new(&mut p.ridge_amplitude, 0.0..=30.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Ridge frequency:"); ui.add(egui::Slider::new(&mut p.ridge_frequency, 0.001..=0.1).logarithmic(true)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Detail amplitude:"); ui.add(egui::Slider::new(&mut p.detail_amplitude, 0.0..=10.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Detail frequency:"); ui.add(egui::Slider::new(&mut p.detail_frequency, 0.001..=0.2).logarithmic(true)); });
            ui.separator();
            ui.label("Cave System");
            ui.horizontal(|ui| { dot_red(ui); ui.label("Caves enabled:"); ui.add(egui::Checkbox::without_text(&mut p.cave_enabled)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Spaghetti freq:"); ui.add(egui::Slider::new(&mut p.cave_spaghetti_freq, 0.005..=0.1).logarithmic(true)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Spaghetti thickness:"); ui.add(egui::Slider::new(&mut p.cave_spaghetti_thickness, 0.01..=0.3)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Noodle freq:"); ui.add(egui::Slider::new(&mut p.cave_noodle_freq, 0.001..=0.2).logarithmic(true)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Noodle thickness:"); ui.add(egui::Slider::new(&mut p.cave_noodle_thickness, 0.01..=0.5)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cheese freq:"); ui.add(egui::Slider::new(&mut p.cave_cheese_freq, 0.002..=0.03).logarithmic(true)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Cheese threshold:"); ui.add(egui::Slider::new(&mut p.cave_cheese_threshold, 0.1..=0.9)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Warp amplitude:"); ui.add(egui::Slider::new(&mut p.cave_warp_amp, 0.0..=80.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Surface margin:"); ui.add(egui::Slider::new(&mut p.cave_surface_margin, 0.1..=10.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Y squash:"); ui.add(egui::Slider::new(&mut p.cave_y_squash, 0.1..=2.0)); });
            ui.horizontal(|ui| { dot_red(ui); ui.label("Water level (gen):"); ui.add(egui::Slider::new(&mut p.water_level, 0.0..=64.0)); });
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

fn draw_meshing_section(ui: &mut egui::Ui, stats: &MeshingStats, clear_cache: &mut bool) {
    ui.collapsing("Meshing Pipeline", |ui| {
        ui.label(format!("Workers: {}", stats.worker_count));
        if stats.pending_submissions > 0 {
            ui.label(format!("Pending: {}", stats.pending_submissions));
        }
        ui.label(format!("Phase 1 active: {}", stats.phase1_in_progress));
        ui.label(format!("Phase 1 waiting: {}", stats.phase1_complete));
        ui.label(format!("Phase 2 active: {}", stats.phase2_in_progress));
        ui.label(format!("Total meshed: {}", stats.total_meshed));
        if stats.last_batch_time_ms > 0.0 {
            ui.label(format!("Last batch: {:.0}ms", stats.last_batch_time_ms));
        }
        let active =
            stats.phase1_in_progress + stats.phase1_complete + stats.phase2_in_progress;
        if active > 0 {
            ui.colored_label(
                egui::Color32::from_rgb(80, 200, 80),
                format!("Meshing... ({} active)", active),
            );
        }

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

fn draw_performance(ui: &mut egui::Ui, state: &UiState) {
    if !state.params.debug.show_performance { return; }
    ui.collapsing("Performance", |ui| {
        ui.label(format!("FPS: {:.0}", state.fps));
        ui.label(format!("Frame: {:.1}ms", state.frame_time_ms));
        ui.label(format!("Triangles: {}", format_number(state.total_triangles)));
        ui.label(format!("Chunks: {}/{}", state.chunks_visible, state.chunks_total));
        if state.chunks_total > 0 {
            let cull_pct = (1.0 - state.chunks_visible as f64 / state.chunks_total as f64) * 100.0;
            ui.label(format!("Culled: {:.0}%", cull_pct));
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