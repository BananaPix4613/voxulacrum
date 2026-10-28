use bevy_ecs::schedule::{Schedule, SystemSet, IntoScheduleConfigs};
use crate::ecs::systems;
use crate::input;
use crate::world::tick::{TickPhase, TickSchedule};

/// Frame execution stages, run in order each frame.
///
/// Input -> Simulation -> Meshing -> UniformWrite -> Render -> PostFrame
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrameStage {
    /// Frame counter, shader hot-reload, window resize, input forwarding.
    Input,
    /// Simulation ticks, then camera, time-of-day, wind, frustum, param change
    /// detection.
    Simulation,
    /// Poll meshing workers, upload completed meshes.
    Meshing,
    /// Build the main side's render data and upload what moved, write uniform
    /// buffers to GPU, compute stats, lay out the UI.
    UniformWrite,
    /// Acquire swapchain, build/execute render graph, paint egui, present.
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
        FrameStage::UniformWrite.after(FrameStage::Meshing),
        FrameStage::Render.after(FrameStage::UniformWrite),
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
            .before(FrameStage::UniformWrite),
        systems::diagnostics::frame_timings_end_uniform
            .after(FrameStage::UniformWrite)
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
        systems::hot_reload::shader_hot_reload_system.in_set(FrameStage::Input),
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
    // Streaming + Meshing
    schedule.add_systems((
        systems::streaming::streaming_tick_system
            .in_set(FrameStage::Meshing)
            .before(systems::meshing::meshing_tick_system),
        systems::streaming::seam_smoothing_system
            .in_set(FrameStage::Meshing)
            .after(systems::streaming::streaming_tick_system)
            .before(systems::meshing::meshing_tick_system),
        systems::jobs::job_pump_system
            .in_set(FrameStage::Meshing)
            .after(systems::streaming::streaming_tick_system),
        systems::chunk_changes::chunk_change_system
            .in_set(FrameStage::Meshing)
            .after(systems::streaming::seam_smoothing_system)
            .before(systems::meshing::meshing_tick_system),
        systems::meshing::meshing_tick_system.in_set(FrameStage::Meshing),
    ));
    // Field probe (debug): poll/dispatch graph-node inspection eval.
    schedule.add_systems(
        crate::ui::field_probe::field_probe_system.in_set(FrameStage::Meshing),
    );
    // UniformWrite. The main side's render data (mask texels, player mesh,
    // chunk boundaries, cap) is built first; the upload system then copies
    // whatever moved into the passes.
    schedule.add_systems((
        systems::visibility::visibility_mask_system
            .in_set(FrameStage::UniformWrite)
            .before(systems::uniforms::write_uniforms_system)
            .before(systems::uniforms::render_upload_system),
        systems::player::player_render_prep_system
            .in_set(FrameStage::UniformWrite)
            .before(systems::uniforms::render_upload_system),
        systems::editor_tools::chunk_boundary_lines_system
            .in_set(FrameStage::UniformWrite)
            .before(systems::uniforms::render_upload_system),
        systems::editor_tools::cross_section_cap_system
            .in_set(FrameStage::UniformWrite)
            .before(systems::uniforms::render_upload_system),
        systems::uniforms::render_upload_system.in_set(FrameStage::UniformWrite),
        systems::uniforms::write_uniforms_system.in_set(FrameStage::UniformWrite),
        systems::diagnostics::compute_stats_system.in_set(FrameStage::UniformWrite),
        // Last, so panel edits reach the uniforms next frame and the panels show
        // this frame's stats.
        crate::ui::egui_layout_system
            .in_set(FrameStage::UniformWrite)
            .after(systems::uniforms::render_upload_system)
            .after(systems::uniforms::write_uniforms_system)
            .after(systems::diagnostics::compute_stats_system),
    ));
    // Render
    schedule.add_systems(
        systems::render::render_present_system.in_set(FrameStage::Render),
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
        (TickPhase::Players, TickPhase::SubRates, TickPhase::Autosave).chain()
    );
    schedule.add_systems((
        crate::player::systems::player_tick_system.in_set(TickPhase::Players),
        systems::fluids::fluid_tick_system.in_set(TickPhase::SubRates),
        systems::persistence::persistence_autosave_system.in_set(TickPhase::Autosave),
    ));
    schedule.add_systems((
        systems::tick::tick_timings_begin.before(TickPhase::Players),
        systems::tick::tick_timings_end_players
            .after(TickPhase::Players)
            .before(TickPhase::SubRates),
        systems::tick::tick_timings_end_sub_rates
            .after(TickPhase::SubRates)
            .before(TickPhase::Autosave),
        systems::tick::tick_timings_end_autosave.after(TickPhase::Autosave),
    ));
    schedule
}
