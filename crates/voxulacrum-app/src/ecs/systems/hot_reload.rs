//! Reloads from disk and from panel requests: shader changes (queued for the
//! render world, which rebuilds the pipelines), world graphs, the selected
//! palette, and parameter change detection.

use bevy_ecs::prelude::*;
use crate::diagnostics::{FrameTimings, SUB_PARAMS};
use crate::ecs::resources::LoadedPalette;
use crate::graph_reload::GraphWatcherRes;
use crate::palette;
use crate::params::ParamChangeKind;
use crate::rendering::resources::RenderRequests;
use crate::shader_reload::ShaderWatcher;
use crate::ui::panels::UiState;

/// Queue the shader files that changed on disk, for `shader_reload_system`.
pub fn shader_watch_system(
    shader_watcher: Res<ShaderWatcher>,
    mut requests: ResMut<RenderRequests>,
) {
    requests.shader_changes.extend(shader_watcher.poll_changes());
}

/// Watch graph files and announce what changed.
///
/// It writes an event and stops there. Before the bus it pushed onto a queue in
/// `UiState` that the regeneration coordinator drained - two systems reaching
/// into a third's state to talk to each other. Now the watcher does not know
/// regeneration exists.
pub fn graph_hot_reload_system(
    graph_watcher: Res<GraphWatcherRes>,
    mut events: EventWriter<crate::world::events::WorldEvent>
) {
    let changed = graph_watcher.0.poll_changes();
    if changed.is_empty() {
        return;
    }
    let manifest = crate::world::world_generator::world_manifest_path();
    for path in changed {
        match crate::world::world_generator::slot_for_graph_file(&manifest, &path) {
            Some(slot) => {
                events.write(crate::world::events::WorldEvent::GraphChanged { slot });
                log::info!("graph changed on disk: {} ({slot:?})", path.display());
            }
            // A `.json` in the graphs directory that the manifest does not
            // reference cannot affect generation.
            None => log::debug!("ignoring unreferenced graph file {}", path.display()),
        }
    }
}

pub fn param_change_detection_system(
    mut ui: ResMut<UiState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_PARAMS);
    let kind = ui.change_detector.detect(&ui.params);
    if kind == ParamChangeKind::MeshInvalidating {
        ui.mesh_params_pending = true;
    }
    let params_clone = ui.params.clone();
    ui.change_detector.snapshot(&params_clone);
}

pub fn palette_load_system(
    mut ui: ResMut<UiState>,
    mut loaded: ResMut<LoadedPalette>,
) {
    if !ui.palette_load_requested {
        return;
    }
    ui.palette_load_requested = false;
    let name = &ui.params.palette.selected_palette;
    if !name.is_empty() {
        let path = crate::paths::asset_root().join("palettes").join(format!("{}.json", name));
        match palette::load_palette(&path) {
            Ok(pal) => {
                log::info!("Loaded palette: {}", pal.name);
                ui.loaded_palette_preview = pal.colors_srgb.clone();
                loaded.0 = Some(pal);
            }
            Err(e) => {
                log::warn!("Failed to load palette {}: {}", name, e);
                loaded.0 = None;
                ui.loaded_palette_preview.clear();
            }
        }
    } else {
        loaded.0 = None;
        ui.loaded_palette_preview.clear();
    }
}
