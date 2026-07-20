use bevy_ecs::schedule::{Schedule, SystemSet, IntoScheduleConfigs};
use crate::ecs::systems;
use crate::input;

/// Frame execution stages, run in order each frame.
///
/// Input -> Simulation -> Meshing -> UniformWrite -> Render -> PostFrame
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum FrameStage {
    /// Frame counter, shader hot-reload, window resize, input forwarding.
    Input,
    /// Camera, time-of-day, wind, frustum, param change detection.
    Simulation,
    /// Poll meshing workers, upload completed meshes.
    Meshing,
    /// Write uniform buffers to GPU, compute stats.
    UniformWrite,
    /// Acquire swapchain, build/execute render graph, egui, present.
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

    // Input
    schedule.add_systems((
        input::process_input_system.in_set(FrameStage::Input),
        input::toggle_ui_system.in_set(FrameStage::Input),
        systems::frame_counter_system.in_set(FrameStage::Input),
        systems::shader_hot_reload_system.in_set(FrameStage::Input),
        systems::graph_hot_reload_system.in_set(FrameStage::Input),
    ));
    // Simulation
    schedule.add_systems((
        systems::simulation_tick_system
            .in_set(FrameStage::Simulation)
            .after(crate::player::systems::player_sim_system)
            .after(systems::room_detection_system),
        systems::param_change_detection_system.in_set(FrameStage::Simulation),
        systems::palette_load_system.in_set(FrameStage::Simulation),
        systems::fluid_tick_system
            .in_set(FrameStage::Simulation)
            .after(systems::simulation_tick_system) // needs FrameState
            .after(crate::interaction::picking_system), // needs the current pick
        crate::interaction::picking_system
            .in_set(FrameStage::Simulation)
            .after(systems::simulation_tick_system),
        crate::interaction::scatter_edit_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::picking_system),
    ));
    // Player sim (Substep 4)
    schedule.add_systems((
        crate::player::systems::player_mode_toggle_system.in_set(FrameStage::Input),
        crate::player::systems::player_input_gather_system.in_set(FrameStage::Simulation),
        crate::player::systems::player_sim_system
            .in_set(FrameStage::Simulation)
            .after(crate::player::systems::player_input_gather_system),
        systems::room_detection_system
            .in_set(FrameStage::Simulation)
            .after(crate::player::systems::player_sim_system),
        crate::interaction::player_targeting_system
            .in_set(FrameStage::Simulation)
            .after(crate::player::systems::player_sim_system),
        crate::interaction::player_tool_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::player_targeting_system),
        crate::interaction::player_action_system
            .in_set(FrameStage::Simulation)
            .after(crate::interaction::player_tool_system),
    ));
    // Streaming + Meshing
    schedule.add_systems((
        systems::streaming_tick_system
            .in_set(FrameStage::Meshing)
            .before(systems::meshing_tick_system),
        systems::seam_smoothing_system
            .in_set(FrameStage::Meshing)
            .after(systems::streaming_tick_system)
            .before(systems::meshing_tick_system),
        systems::meshing_tick_system.in_set(FrameStage::Meshing),
    ));
    // Field probe (debug): poll/dispatch graph-node inspection eval.
    schedule.add_systems(
        crate::ui::field_probe::field_probe_system.in_set(FrameStage::Meshing),
    );
    // UniformWrite
    schedule.add_systems((
        systems::visibility_mask_upload_system
            .in_set(FrameStage::UniformWrite)
            .before(systems::write_uniforms_system),
        systems::write_uniforms_system.in_set(FrameStage::UniformWrite),
        systems::material_color_upload_system.in_set(FrameStage::UniformWrite),
        systems::player_render_prep_system.in_set(FrameStage::UniformWrite),
        systems::compute_stats_system.in_set(FrameStage::UniformWrite),
    ));
    // Render
    schedule.add_systems(
        systems::render_present_system.in_set(FrameStage::Render),
    );
    // PostFrame
    schedule.add_systems((
        systems::world_regen_system.in_set(FrameStage::PostFrame),
        systems::persistence_autosave_system.in_set(FrameStage::PostFrame),
    ));

    schedule
}