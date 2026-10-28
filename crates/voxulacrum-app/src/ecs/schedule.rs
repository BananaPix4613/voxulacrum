use bevy_ecs::schedule::{Schedule, SystemSet, IntoScheduleConfigs};
use crate::ecs::systems;
use crate::input;
use crate::world::tick::{TickPhase, TickSchedule};

/// Frame execution stages, run in order each frame.
///
/// Input -> Simulation -> Meshing -> Extract -> Render -> PostFrame
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrameStage {
    /// Frame counter, shader watching, input forwarding.
    Input,
    /// Simulation ticks, then camera, time-of-day, wind, frustum, param change
    /// detection.
    Simulation,
    /// The local camera's area of interest, the change feed, meshing and the
    /// per-chunk builds.
    Meshing,
    /// Build the main side's render data, compute stats, lay out the UI, then
    /// hand the frame to the render world.
    Extract,
    /// Run the render world's schedule and read back what it reports.
    Render,
    /// World regen polling.
    PostFrame,
}

/// Build the frame schedule with stage ordering configured.
pub fn build_frame_schedule() -> Schedule {
    let mut schedule = Schedule::default();
    schedule.configure_sets((
        FrameStage::Input,
        FrameStage::Simulation.after(FrameStage::Input),
        FrameStage::Meshing.after(FrameStage::Simulation),
        FrameStage::Extract.after(FrameStage::Meshing),
        FrameStage::Render.after(FrameStage::Extract),
        FrameStage::PostFrame.after(FrameStage::Render),
    ));

    // Frame timing boundaries (diagnostics A1). These belong to no set and are
    // ordered against the sets, so each lands exactly between two stages.
    schedule.add_systems((
        systems::diagnostics::frame_timings_begin.before(FrameStage::Input),
        systems::diagnostics::frame_timings_end_input
            .after(FrameStage::Input)
            .before(FrameStage::Simulation),
        systems::diagnostics::frame_timings_end_simulation
            .after(FrameStage::Simulation)
            .before(FrameStage::Meshing),
        systems::diagnostics::frame_timings_end_meshing
            .after(FrameStage::Meshing)
            .before(FrameStage::Extract),
        systems::diagnostics::frame_timings_end_extract
            .after(FrameStage::Extract)
            .before(FrameStage::Render),
        systems::diagnostics::frame_timings_end_render
            .after(FrameStage::Render)
            .before(FrameStage::PostFrame),
        systems::diagnostics::frame_timings_frame_end.after(FrameStage::PostFrame),
    ));

    // Event bus. `update` before anything writes, the pump inside Input so the
    // door's outbox is published early, subscribers in PostFrame so they see
    // everything this frame produced.
    schedule.add_systems((
        systems::world_events::world_event_update_system.before(FrameStage::Input),
        systems::world_events::world_event_pump_system.in_set(FrameStage::Input),
        systems::world_events::world_event_log_system.in_set(FrameStage::PostFrame),
    ));

    // Input
    schedule.add_systems((
        systems::tick::presentation_clock_system.in_set(FrameStage::Input),
        input::process_input_system.in_set(FrameStage::Input),
        input::toggle_ui_system.in_set(FrameStage::Input),
        systems::diagnostics::frame_counter_system.in_set(FrameStage::Input),
        systems::hot_reload::shader_watch_system.in_set(FrameStage::Input),
        systems::hot_reload::graph_hot_reload_system.in_set(FrameStage::Input),
    ));
    // Simulation
    schedule.add_systems((
        systems::simulation::simulation_tick_system
            .in_set(FrameStage::Simulation)
            .after(systems::tick::run_ticks_system)
            .after(systems::visibility::room_detection_system),
        systems::climate::climate_tick_system
            .in_set(FrameStage::Simulation)
            .after(systems::simulation::simulation_tick_system),
        systems::hot_reload::param_change_detection_system.in_set(FrameStage::Simulation),
        systems::editor_tools::column_inspector_system.in_set(FrameStage::Simulation),
        systems::editor_tools::biome_map_system.in_set(FrameStage::Simulation),
        systems::editor_tools::blueprint_selection_gizmo_system
            .in_set(FrameStage::Simulation)
            .after(systems::editor_tools::blueprint_authoring_system),
        systems::editor_tools::blueprint_authoring_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::picking_system), // needs the current pick
        systems::editor_tools::skeleton_debug_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::picking_system), // needs the current pick
        systems::hot_reload::palette_load_system.in_set(FrameStage::Simulation),
        systems::fluids::fluid_pour_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::picking_system), // needs the current pick
        crate::interaction::picking_system
            .in_set(FrameStage::Simulation)
            .after(systems::simulation::simulation_tick_system),
    ));
    // Simulation ticks, with the player systems that feed and read them
    schedule.add_systems((
        crate::player::systems::player_mode_toggle_system.in_set(FrameStage::Input),
        crate::player::systems::player_input_gather_system.in_set(FrameStage::Simulation),
        systems::tick::run_ticks_system
            .in_set(FrameStage::Simulation)
            .after(crate::player::systems::player_input_gather_system),
        systems::visibility::room_detection_system
            .in_set(FrameStage::Simulation)
            .after(systems::tick::run_ticks_system),
        systems::streaming::residency_report_system
            .in_set(FrameStage::Simulation)
            .after(systems::tick::run_ticks_system),
        crate::interaction::player_targeting_system
            .in_set(FrameStage::Simulation)
            .after(systems::simulation::simulation_tick_system),
        crate::interaction::player_tool_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::player_targeting_system),
        crate::interaction::player_action_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::player_tool_system),
    ));
    // Meshing. Residency and seam finalization run inside the tick; this stage
    // sends the camera's area for the next frame's ticks and builds from the
    // change feed.
    schedule.add_systems((
        systems::streaming::local_interest_system.in_set(FrameStage::Meshing),
        systems::jobs::job_pump_system.in_set(FrameStage::Meshing),
        systems::chunk_changes::chunk_change_system
            .in_set(FrameStage::Meshing)
            .before(systems::meshing::meshing_tick_system),
        systems::meshing::meshing_tick_system.in_set(FrameStage::Meshing),
    ));
    // Field probe (debug): poll/dispatch graph-node inspection eval.
    schedule.add_systems(
        crate::ui::field_probe::field_probe_system.in_set(FrameStage::Meshing),
    );
    // Extract. The main side's render data (mask texels, player mesh, chunk
    // boundaries, cap), the stats and the UI layout are built first; then
    // `extract_system` hands the frame to the render world.
    schedule.add_systems((
        systems::visibility::visibility_mask_system
            .in_set(FrameStage::Extract)
            .before(systems::extract::extract_system),
        systems::player::player_render_prep_system
            .in_set(FrameStage::Extract)
            .before(systems::extract::extract_system),
        systems::editor_tools::chunk_boundary_lines_system
            .in_set(FrameStage::Extract)
            .before(systems::extract::extract_system),
        systems::editor_tools::cross_section_cap_system
            .in_set(FrameStage::Extract)
            .before(systems::extract::extract_system),
        systems::diagnostics::compute_stats_system
            .in_set(FrameStage::Extract)
            .before(crate::ui::egui_layout_system),
        // After the stats, so the panels show them; before the hand-over, which
        // takes the laid-out frame and this frame's parameters.
        crate::ui::egui_layout_system
            .in_set(FrameStage::Extract)
            .before(systems::extract::extract_system),
        systems::extract::extract_system.in_set(FrameStage::Extract),
    ));
    // Render
    schedule.add_systems(
        systems::extract::render_world_system.in_set(FrameStage::Render),
    );
    // PostFrame
    schedule.add_systems(
        systems::regen::world_regen_system.in_set(FrameStage::PostFrame),
    );

    schedule
}

/// Build the schedule one simulation tick runs (`world::tick::run_tick`),
/// phase by phase. Tick systems read world state, the tick and queued input
/// only: no frame time and no render or UI resource, so the headless server
/// (R3c) can run this same schedule.
pub fn build_tick_schedule() -> Schedule {
    let mut schedule = Schedule::new(TickSchedule);
    schedule.configure_sets(
        (TickPhase::Players, TickPhase::SubRates, TickPhase::Residency, TickPhase::Autosave)
            .chain()
    );
    schedule.add_systems((
        crate::player::systems::player_tick_system.in_set(TickPhase::Players),
        systems::fluids::fluid_tick_system.in_set(TickPhase::SubRates),
        (
            systems::streaming::residency_system,
            systems::streaming::seam_smoothing_system,
        )
            .chain()
            .in_set(TickPhase::Residency),
        systems::persistence::persistence_autosave_system.in_set(TickPhase::Autosave),
    ));
    schedule.add_systems((
        systems::tick::tick_timings_begin.before(TickPhase::Players),
        systems::tick::tick_timings_end_players
            .after(TickPhase::Players)
            .before(TickPhase::SubRates),
        systems::tick::tick_timings_end_sub_rates
            .after(TickPhase::SubRates)
            .before(TickPhase::Residency),
        systems::tick::tick_timings_end_residency
            .after(TickPhase::Residency)
            .before(TickPhase::Autosave),
        systems::tick::tick_timings_end_autosave.after(TickPhase::Autosave),
    ));
    schedule
}
