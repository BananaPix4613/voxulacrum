use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::ecs::resources::*;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::ParamChangeKind;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::debug_lines::{voxel_box_lines, DebugLinePass};
use crate::rendering::render_context::RenderContext;
use crate::rendering::surface_state::SurfaceState;
use crate::rendering::main_scene_pass::{CapConfig, MainScenePassNode};
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::pipelines::{PipelineRegistry, PipelineResources};
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_graph::{RenderGraph, ResourceId, ResourceMap};
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::shadow_pass::ShadowPassNode;
use crate::rendering::upscale_pass::UpscalePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::water_pass::WaterPass;
use crate::rendering::reflection_pass::ReflectionPassNode;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::frustum::Frustum;
use crate::shader_reload::ShaderWatcher;
use crate::graph_reload::GraphWatcherRes;
use crate::input::{InputState, PointerState};
use crate::interaction::PickState;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui;
use crate::ui::panels::UiState;
use crate::world::regen::WorldRegenCoordinator;
use crate::{compute_render_dimensions, palette, FrameCounter};
use crate::diagnostics::{
    FrameTimings, SUB_CLIMATE, SUB_DRAIN, SUB_FLUID, SUB_PARAMS, SUB_PUMP, SUB_SAVE, SUB_SCAN,
    SUB_SEAM, SUB_STREAM, SUB_SUBMIT, SUB_UNLOAD, SUB_UPLOAD,
};
use crate::jobs::JobSystem;
use crate::world::chunk::{LoadedChunk, CHUNK_WORLD_SIZE};
use crate::world::streaming::{CameraView, ChunkStreamingManager};
use crate::world::persistence::WorldPersistence;
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};
use crate::materials::MaterialRegistryRes;
use crate::player::sim::WorldView;

// ==========================================================================
// Input stage
// ==========================================================================

pub fn frame_counter_system(
    mut counter: ResMut<FrameCounter>,
    timings: Res<FrameTimings>,
    mut ui: ResMut<UiState>
) {
    counter.tick();
    ui.fps = counter.fps;
    // Mirror the once-per-second published snapshot; the panel draws from
    // `UiState`, and these values only change on the second boundary, so the
    // copy is not a per-frame cost worth avoiding.
    ui.over_budget_frames = timings.over_budget;
    ui.worst_frame_ms = timings.inter_max_ms;
    ui.cpu_max_ms = timings.cpu_max_ms;
    ui.present_mean_ms = timings.present_mean_ms;
    ui.stage_mean_ms = timings.stage_mean_ms;
    ui.stage_max_ms = timings.stage_max_ms;
    // Session-scoped, so unlike the rows above these change continuously rather
    // than on the second boundary. Mirrored here anyway because the panel draws
    // only from `UiState`; being one frame stale is immaterial for a maximum.
    ui.frontier_cpu_max_ms = timings.frontier_cpu_max_ms;
    ui.frontier_cpu_mean_ms = timings.frontier_cpu_mean_ms();
    ui.frontier_frames = timings.frontier_frames;
    ui.frontier_over_budget = timings.frontier_over_budget;
    ui.frontier_worst_stages = timings.frontier_worst_stages;
    ui.frontier_worst_sub = timings.frontier_worst_sub;
    ui.frontier_sub_max = timings.frontier_sub_max;
    for i in 0..crate::diagnostics::SUB_COUNT {
        ui.frontier_sub_mean[i] = timings.frontier_sub_mean_ms(i);
    }
}

// ==========================================================================
// Frame timing boundaries (diagnostics A1)
// ==========================================================================
// Seven systems that belong to no `FrameStage` set and are ordered against the
// sets themselves, so each runs exactly between two stages. Written as separate
// named functions rather than a closure factory so the schedule reads as an
// explicit list of boundaries and the stage indices are visible at each site.

pub fn frame_timings_begin(mut t: ResMut<FrameTimings>) {
    t.frame_begin();
}

pub fn frame_timings_end_input(mut t: ResMut<FrameTimings>) {
    t.end_stage(0);
}

pub fn frame_timings_end_simulation(mut t: ResMut<FrameTimings>) {
    t.end_stage(1);
}

pub fn frame_timings_end_meshing(mut t: ResMut<FrameTimings>) {
    t.end_stage(2);
}

pub fn frame_timings_end_uniform(mut t: ResMut<FrameTimings>) {
    t.end_stage(3);
}

pub fn frame_timings_end_render(mut t: ResMut<FrameTimings>) {
    t.end_stage(4);
}

pub fn frame_timings_frame_end(
    mut t: ResMut<FrameTimings>,
    mut ui: ResMut<UiState>,
    streaming: Res<ChunkStreamingManager>,
) {
    // Before accumulating, so the frame the button was clicked on starts the
    // fresh sample rather than being discarded by it.
    if std::mem::take(&mut ui.reset_frontier_requested) {
        t.reset_frontier();
    }
    // "At an active generation frontier" (roadmap §7.3) = the streamed set is
    // not yet satisfied: chunks wanted but not resident, or generation jobs
    // still outstanding. `missing` alone would miss the tail of a fill where
    // the last results are landing; `in_flight` alone would miss frames where a
    // deficit is open but backpressure has not submitted yet.
    let s = streaming.stats();
    t.frame_end(s.missing > 0 || s.in_flight > 0);
}

// ==========================================================================
// World event bus (design §6.7)
// ==========================================================================

/// Swap `Events<WorldEvent>`'s double buffer once per frame.
///
/// A full Bevy `App` registers this automatically via `add_event`; this engine
/// owns its own schedule, so it is explicit. Without it events are never aged
/// out and every `EventReader` re-reads the whole history.
pub fn world_event_update_system(mut events: ResMut<Events<crate::world::events::WorldEvent>>) {
    events.update();
}

/// Publish the mutation door's outbox onto the box.
///
/// The door is a method on `World`, not a system, so it cannot hold an
/// `EventWriter`. This runs unconditionally every frame, which is what makes the
/// outbox bounded.
pub fn world_event_pump_system(
    mut world: ResMut<VoxelWorld>,
    mut writer: EventWriter<crate::world::events::WorldEvent>,
) {
    for event in world.0.drain_events() {
        writer.write(event);
    }
}

/// **Bus subscriber #1**: the mutation-and-event log's event counts.
///
/// A genuine subscriber - it reads the stream and knows nothing about who
/// emitted, which is the property the bus exists to provide.
pub fn world_event_log_system(
    mut events: EventReader<crate::world::events::WorldEvent>,
    mut ui: ResMut<UiState>,
) {
    for event in events.read() {
        ui.event_counts[event.index()] += 1;
        ui.events_total += 1;
    }
}

pub fn shader_hot_reload_system(
    shader_watcher: Res<ShaderWatcher>,
    ctx: Res<RenderContext>,
    mut pipeline_registry: ResMut<PipelineRegistry>,
    pipeline_resources: Res<PipelineResources>,
    mut post_process: ResMut<PostProcessPass>,
    mut outline_pass: ResMut<OutlinePass>,
    mut palette_pass: ResMut<PalettePass>,
    mut cap_pass: ResMut<CapPass>,
    mut ui: ResMut<UiState>,
) {
    let changed = shader_watcher.poll_changes();
    for path in changed {
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };

        if filename == "post_process.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match post_process.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded post_process.wgsl");
                            ui.push_shader_log("post_process.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("post_process.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("post_process.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read post_process.wgsl: {}", e);
                    ui.push_shader_log(format!("post_process.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "outline.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match outline_pass.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded outline.wgsl");
                            ui.push_shader_log("outline.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("outline.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("outline.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read outline.wgsl: {}", e);
                    ui.push_shader_log(format!("outline.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "palette.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match palette_pass.try_reload_shader(&ctx, &source) {
                        Ok(()) => {
                            log::info!("Reloaded palette.wgsl");
                            ui.push_shader_log("palette.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("palette.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("palette.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read palette.wgsl: {}", e);
                    ui.push_shader_log(format!("palette.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        if filename == "cap.wgsl" {
            match std::fs::read_to_string(&path) {
                Ok(source) => {
                    match cap_pass.try_reload_shader(
                        &ctx,
                        &pipeline_resources.global_bind_group_layout,
                        &source,
                    ) {
                        Ok(()) => {
                            log::info!("Reloaded cap.wgsl");
                            ui.push_shader_log("cap.wgsl reloaded".into(), false);
                        }
                        Err(e) => {
                            log::warn!("cap.wgsl compile error: {}", e);
                            ui.push_shader_log(format!("cap.wgsl: {}", e), true);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read cap.wgsl: {}", e);
                    ui.push_shader_log(format!("cap.wgsl read error: {}", e), true);
                }
            }
            continue;
        }

        // All other shaders go through the pipeline registry
        if let Some(result) = pipeline_registry.try_reload(
            &ctx,
            &pipeline_resources,
            &path,
        ) {
            if result.success {
                log::info!("Reloaded {}", result.filename);
            } else {
                log::warn!("{}: {}", result.filename, result.message);
            }
            ui.push_shader_log(
                format!("{}: {}", result.filename, result.message),
                !result.success,
            );
        }
    }
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

// ==========================================================================
// Simulation stage
// ==========================================================================

/// Vertical offset from the player's feet to the camera look-at point, framing
/// the player's middle (AABB is 1.8 tall).
const CAMERA_FOLLOW_OFFSET: glam::Vec3 = glam::Vec3::new(0.0, 1.0, 0.0);

pub fn simulation_tick_system(
    mut sim: ResMut<SimulationManager>,
    ui: Res<UiState>,
    render_targets: Res<RenderTargets>,
    input: Res<InputState>,
    world: Res<VoxelWorld>,
    clock: Res<crate::player::systems::PlayerClock>,
    players: Query<&crate::player::systems::Player>,
    mut commands: Commands,
) {
    // Interpolated player feet position (Play mode), for both camera follow and cutaway.
    let player_pos = if world.0.mode == EngineMode::Play {
        players
            .iter()
            .next()
            .map(|p| p.prev.position.lerp(p.curr.position, clock.alpha()))
    } else {
        None
    };
    let follow = player_pos.map(|p| p + CAMERA_FOLLOW_OFFSET);

    let frame = sim.tick(&ui.params, &render_targets, &input, follow);

    commands.insert_resource(frame);
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

/// Re-mesh water for `chunks` plus each one's 4 horizontal neighbors. The water
/// mesher welds surface heights across chunk seams (design §7 rendering), so a
/// chunk whose fluid changed shifts its neighbors' boundary columns too. Only
/// X/Z neighbors matter: the mesher samples horizontally within one chunk-Y band.
/// Neighbors that aren't resident are a no-op (`add_chunk_water` early-outs), and
/// the set dedups overlap so each chunk rebuilds at most once per call.
fn rebuild_water_with_neighbors(
    water_pass: &mut WaterPass,
    world: &crate::world::World,
    device: &wgpu::Device,
    chunks: impl IntoIterator<Item = IVec3>,
) {
    let mut set: std::collections::HashSet<IVec3> = std::collections::HashSet::new();
    for pos in chunks {
        set.insert(pos);
        for d in [
            IVec3::new(1, 0, 0), IVec3::new(-1, 0, 0),
            IVec3::new(0, 0, 1), IVec3::new(0, 0, -1),
        ] {
            set.insert(pos + d);
        }
    }
    for pos in set {
        water_pass.add_chunk_water(pos, world, device);
    }
}

pub fn fluid_tick_system(
    mut clock: ResMut<FluidClock>,
    frame: Res<FrameState>,
    input: Res<InputState>,
    pick: Res<PickState>,
    mut world: ResMut<VoxelWorld>,
    mut water_pass: ResMut<WaterPass>,
    ctx: Res<RenderContext>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_FLUID);
    let mut changed: std::collections::HashSet<glam::IVec3> = std::collections::HashSet::new();

    // Debug disturbance (Authoring dev tool): press G with the cursor over terrain
    // to pour a short water column. Inert in Play mode (fluid interaction there is
    // a later substep) rather than a mode-rejected panic.
    if world.0.mode == EngineMode::Authoring
        && input.just_pressed(crate::input::GameAction::PourWater)
    {
        if let Some(anchor) = pick.anchor {
            let out = world
                .0
                .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn { anchor }))
                .expect("authoring pour in authoring mode");
            changed.extend(out.water_rebuild);
        }
    }

    let active_total: usize =
        world.0.chunks.values().map(|c| c.data.fluids.active.len()).sum();
    if active_total > 100_000 {
        log::warn!("fluid: {active_total} active cells - possible mass-conservation runaway");
    }
    let mut ticks = clock.ticks_for(frame.dt);
    // Throttle: during a very large disturbance, run fewer ticks per frame so a
    // big flow slows down instead of stalling the framerate. Each tick is still a
    // full, deterministic global two-phase; we just do fewer per frame.
    if active_total > 20_000 {
        ticks = ticks.min(1);
    }
    for _ in 0..ticks {
        // Phase 1: plan every active chunk from the shared pre-tick snapshot,
        // reading neighbor chunks read-only (nothing mutates yet).
        let active: Vec<voxel_core::ChunkCoord> = world
            .0
            .chunks
            .iter()
            .filter(|(_, c)| !c.data.fluids.active.is_empty())
            .map(|(coord, _)| *coord)
            .collect();
        if active.is_empty() {
            break;
        }
        let mut plans: Vec<(voxel_core::ChunkCoord, crate::world::fluid_sim::ChunkPlan)> =
            Vec::new();
        {
            let w = &world.0;
            for &coord in &active {
                let Some(this) = w.chunks.get(&coord) else { continue };
                let faces = fluid_neighbor_faces(w, coord);
                let sample = crate::world::fluid_sim::NeighborSample::new(
                    &this.data.fluids,
                    this.data.voxels.as_ref(),
                    faces,
                );
                let plan = crate::world::fluid_sim::plan_chunk(&this.data.fluids, &sample);
                plans.push((coord, plan));
            }
        }
        // Phase 2 and 3 (commit + wake) are the tick's write half and go
        // through the mutation door, which also does the persistence marking
        // that used to be a separate `MarkFluidDirty` call. The sim runs in any
        // mode, so this is a System mutation, not an actor edit.
        let out = world
            .0
            .execute(MutationCommand::system(WorldMutation::CommitFluidPlans { plans }))
            .expect("system fluid commit accepted in any mode");
        changed.extend(out.water_rebuild);
    }

    rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, changed);
}

/// The primary biome tag and a cheap "has notable water" signal for
/// whichever loaded chunk column covers `world_pos` (`None` if that column
/// isn't loaded yet). The water check reuses the chunk's already-resident
/// `FluidLayer` - no new persisted data, no extra generation cost, and it's
/// a one-way read (never mutates fluid state), so it doesn't reintroduce the
/// fluid/weather coupling the climate design deliberately avoids.
fn regional_weather_data_at(world: &crate::world::World, world_pos: glam::Vec2) -> Option<(u16, bool)> {
    let chunk_x = (world_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let chunk_z = (world_pos.y / CHUNK_WORLD_SIZE).floor() as i32;
    for chunk_y in world.min_chunk_y..world.max_chunk_y {
        let coord = voxel_core::ChunkCoord::from(IVec3::new(chunk_x, chunk_y, chunk_z));
        if let Some(chunk) = world.chunks.get(&coord) {
            let biome = chunk.data.tags.biomes.first().map(|b| b.0).unwrap_or(0);
            let has_water = !chunk.data.fluids.cells.is_empty();
            return Some((biome, has_water));
        }
    }
    None
}

/// Advance the simulated cloud layers, rasterize them into the shared cloud
/// texture, and refresh the offsets/coverages `simulation_tick_system` reads
/// into this frame's `FrameState`. Runs before `simulation_tick_system` and
/// reads last frame's camera position - a one-frame lag, imperceptible for a
/// slowly-panning macro-scale field (the same trade-off already accepted for
/// the shadow frustum's smoothed camera).
pub fn climate_tick_system(
    mut sim: ResMut<SimulationManager>,
    ui: Res<UiState>,
    world: Res<VoxelWorld>,
    ctx: Res<RenderContext>,
    mut clock: ResMut<ClimateClock>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_CLIMATE);
    let now = std::time::Instant::now();
    let dt = (now - clock.last_tick).as_secs_f32().min(0.25);
    clock.last_tick = now;
    clock.elapsed += dt;

    let camera_xz = glam::Vec2::new(sim.camera.smooth_target.x, sim.camera.smooth_target.z);
    let base_wind = sim.wind.wind_vector;
    let elapsed = clock.elapsed;
    let world_eval = world.0.generator.world_eval();

    sim.cloud_shadow.tick(
        &ctx,
        &ui.params.cloud,
        camera_xz,
        base_wind,
        dt,
        elapsed,
        world_eval,
        |world_pos| {
            match world.0.generator.biome_id_at_world(world_pos.x, world_pos.y) {
                // Biome from the generator (residency-independent); water as a
                // best-effort refinement only where the chunk is resident.
                Some(biome) => {
                    let has_water = regional_weather_data_at(&world.0, world_pos)
                        .is_some_and(|(_, w)| w);
                    Some((biome, has_water))
                }
                // Single-biome world (no Zone graph): fall back to the old
                // loaded-chunk lookup wholesale.
                None => regional_weather_data_at(&world.0, world_pos),
            }
        },
        |world_pos| regional_weather_data_at(&world.0, world_pos).map(|(_, w)| w),
    );
}

/// Read-only `(fluids, storage)` of the six face-neighbors of `coord`, in the
/// order `NeighborSample` expects: `[-x, +x, -y, +y, -z, +z]`.
fn fluid_neighbor_faces<'a>(
    world: &'a crate::world::World,
    coord: voxel_core::ChunkCoord,
) -> [Option<(&'a crate::world::layers::FluidLayer, &'a crate::world::storage::ChunkStorage)>; 6] {
    let offs = [(-1i32, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)];
    let mut faces: [Option<(
        &crate::world::layers::FluidLayer,
        &crate::world::storage::ChunkStorage,
    )>; 6] = [None; 6];
    for (i, &(dx, dy, dz)) in offs.iter().enumerate() {
        let nc = voxel_core::ChunkCoord::new(coord.x + dx, coord.y + dy, coord.z + dz);
        if let Some(chunk) = world.chunks.get(&nc) {
            faces[i] = Some((&chunk.data.fluids, chunk.data.voxels.as_ref()));
        }
    }
    faces
}

/// Rebuild the player avatar mesh each frame at the interpolated render position
/// (same interpolation the camera follows), with a drop shadow on the first solid
/// ground below. Hidden outside Play mode / when no player exists.
pub fn player_render_prep_system(
    ctx: Res<RenderContext>,
    world: Res<VoxelWorld>,
    clock: Res<crate::player::systems::PlayerClock>,
    sim: Res<SimulationManager>,
    render_targets: Res<RenderTargets>,
    players: Query<&crate::player::systems::Player>,
    mut player_pass: ResMut<PlayerPass>,
) {
    let player = players.iter().next();
    if world.0.mode != EngineMode::Play || player.is_none() {
        player_pass.visible = false;
        return;
    }
    let p = player.unwrap();
    let feet = p.prev.position.lerp(p.curr.position, clock.alpha());

    // Snap the RENDERED position to the world texel lattice (sim untouched).
    // The lattice anchor is the world origin - snap_camera pins integer world
    // offsets from the origin to texel centers - so quantizing the player's
    // view-space offset RELATIVE to the origin's puts it at the same sub-texel
    // phase every frame, still or moving. (Quantizing absolute view coords is
    // a no-op under a player-pinned camera and scrambles with the camera frac.)
    let view = sim.camera.view_matrix();
    let k = render_targets.k as f32;
    let o = view.transform_point3(glam::Vec3::ZERO);
    let mut v = view.transform_point3(feet);
    v.x = o.x + ((v.x - o.x) * k).round() / k;
    v.y = o.y + ((v.y - o.y) * k).round() / k;
    let feet = view.inverse().transform_point3(v);

    let ground_y = player_ground_below(&world.0, feet);
    let (verts, indices, body) = crate::rendering::player_pass::build_player_mesh(feet, p.curr.facing, ground_y);
    player_pass.update(&ctx.queue, &verts, &indices, body);
    player_pass.visible = true;
}

/// Top surface Y of the first solid voxel at or below the player's feet column
/// (scans down a bounded distance; finds the floor of a hole the player is over).
fn player_ground_below(world: &crate::world::World, feet: glam::Vec3) -> Option<f32> {
    use crate::player::sim::WorldView;
    let vx = feet.x.floor() as i32;
    let vz = feet.z.floor() as i32;
    let start = feet.y.floor() as i32;
    for y in (start - 64..=start).rev() {
        if let Some((_, iy1)) = world.solid_interval(glam::IVec3::new(vx, y, vz)) {
            return Some(y as f32 + iy1);
        }
    }
    None
}

/// Visibility flood each time the player crosses into a new head cell. Produces
/// undergroundness `u` and the `visible` air set the mask marks; the shader's
/// air-side face rule does the rest (note v2 §3 — no cut set, view-independent, so
/// no rotation recompute). A cheap overhead pre-filter skips the flood outdoors.
pub fn room_detection_system(
    world: Res<VoxelWorld>,
    players: Query<&crate::player::systems::Player>,
    mut room: ResMut<crate::world::room::RoomState>,
) {
    use crate::player::sim::WorldView;
    use crate::world::room::{flood_visibility, overhead_coverage};

    const BUDGET: u32 = 22;             // through-air flood budget (note §4.1)
    const OPEN_SKY_COVERAGE: f32 = 0.3; // below this overhead, skip the flood
    const U_THRESHOLD: f32 = 0.6;       // u above this = enclosed (mask on)
    const CLARITY_RADIUS: u32 = 24;     // MUST match CLARITY_RADIUS in terrain.wgsl

    let player = players.iter().next();
    if world.0.mode != EngineMode::Play || player.is_none() {
        if room.enclosed {
            log::info!("player left enclosed space (play/mode ended)");
        }
        room.enclosed = false;
        room.undergroundness = 0.0;
        room.last_origin = None;
        return;
    }

    let feet = player.unwrap().curr.position;
    let head = glam::IVec3::new(
        feet.x.floor() as i32,
        (feet.y + 1.6).floor() as i32,
        feet.z.floor() as i32,
    );

    // Throttle: only recompute when the player enters a new head cell.
    if room.last_origin == Some(head) {
        return;
    }
    room.last_origin = Some(head);

    let is_solid = |c: glam::IVec3| world.0.solid_interval(c).is_some();

    // Cheap pre-filter: little overhead => outdoors => no flood.
    let coverage = overhead_coverage(head, 1, 16, &is_solid);
    if coverage < OPEN_SKY_COVERAGE {
        if room.enclosed {
            log::info!("player left enclosed space (open sky)");
        }
        room.enclosed = false;
        room.undergroundness = 0.0;
        return;
    }

    let top = world.0.max_chunk_y * crate::world::chunk::CHUNK_SIZE as i32;
    let is_air = |c: glam::IVec3| world.0.solid_interval(c) != Some((0.0, 1.0));
    let sky_lit = |c: glam::IVec3| {
        let mut y = c.y + 1;
        while y < top {
            if is_solid(glam::IVec3::new(c.x, y, c.z)) {
                return false;
            }
            y += 1;
        }
        true
    };

    let vol = flood_visibility(head, BUDGET, is_air, sky_lit);
    let u = vol.undergroundness;
    let cell_count = vol.visible.len();

    // Size the volume circle from the *lit* region only (cost <= CLARITY_RADIUS), so it
    // matches what actually renders in color - not the full BUDGET-22 flood.
    let mut vr = 0.0f32;
    for (&c, &cost) in vol.visible.iter() {
        if cost <= CLARITY_RADIUS {
            vr = vr.max((c - head).as_vec3().length());
        }
    }
    for &c in vol.visible.keys() {
        vr = vr.max((c - head).as_vec3().length());
    }

    room.enclosed = u >= U_THRESHOLD;
    room.undergroundness = u;
    room.volume_radius = vr;
    room.visible = vol.visible;
    room.dirty = true;

    log::info!(
        "visibility: u {:.2}, {} cells, overhead {:.2}{}",
        u,
        cell_count,
        coverage,
        if room.enclosed { ", enclosed" } else { "" }
    );
}

/// Run a requested column inspection (roadmap §4.3).
///
/// Runs on the worker pool and blocks: the full pipeline is tens of
/// milliseconds, and it runs only on an explicit Refresh.
pub fn column_inspector_system(
    mut ui: ResMut<UiState>,
    world: Res<VoxelWorld>,
    jobs: Res<JobSystem>,
) {
    let inspector = &mut ui.column_inspector;
    if !inspector.open || !inspector.dirty {
        return;
    }
    inspector.dirty = false;
    // On the worker pool, blocking. The full pipeline runs `Evaluator::evaluate`,
    // which `debug_assert`s it is not on the schedule thread - a guard that is
    // right, and which `install` satisfies by handing the closure to a pool
    // worker. This is the determinism checker's precedent: an explicit operator
    // action that blocks until it finishes, rather than machinery to make a
    // once-in-a-while query asynchronous.
    let (wx, wz) = (inspector.world_x, inspector.world_z);
    let y_range = world.0.min_chunk_y..world.0.max_chunk_y;
    let generator = std::sync::Arc::clone(&world.0.generator);
    let result = jobs.pool().install(move || generator.inspect_column(wx, wz, y_range));
    match result {
        Ok(report) => {
            inspector.report = Some(report);
            inspector.error = None;
        }
        Err(e) => {
            inspector.report = None;
            inspector.error = Some(e);
        }
    }
}

/// Service the blueprint panel (roadmap §4.2).
///
/// The panel arms a tool; this consumes the next in-world click and performs it
/// at the picked voxel. `pick.anchor` is already `None` whenever the cursor is
/// over egui, so the click that armed a tool can never also trigger it.
pub fn blueprint_authoring_system(
    mut ui: ResMut<UiState>,
    mut world: ResMut<VoxelWorld>,
    pick: Res<PickState>,
    pointer: Res<PointerState>,
    input: Res<InputState>,
    sim: Res<SimulationManager>,
    registry: Res<MaterialRegistryRes>,
) {
    use crate::input::GameAction;
    use crate::ui::blueprint_panel::ArmedTool;

    let state = &mut ui.blueprint_panel;
    if !state.open {
        return;
    }
    let dir = crate::world::blueprint::blueprint_dir();

    if std::mem::take(&mut state.refresh_requested) {
        match voxel_core::Blueprint::list(&dir) {
            Ok(names) => {
                state.selected = state.selected.min(names.len().saturating_sub(1));
                state.library = names;
            }
            Err(e) => state.error = Some(e),
        }
    }

    // Match the preview to the camera, so a blueprint pointing to the upper
    // right in the preview points there in the world too. The camera orbits, so
    // a fixed object appears to turn the *opposite* way - hence the negation.
    // `target_rotation` rather than `rotation`: the preview should snap with the
    // key, not spin through the camera's ease.
    let base = std::f32::consts::FRAC_PI_4;
    let camera_steps =
        ((sim.camera.target_rotation - base) / std::f32::consts::FRAC_PI_2).round() as i32;
    state.camera_yaw = voxel_core::Yaw::from_steps(-camera_steps);
    
    // Keep the preview in step with the selection. Loading and resolving is
    // filesystem work, so it happens when the selection changes rather than
    // every frame - and the cached cells stay unrotated, so changing the yaw
    // redraws without touching disk.
    let want = state.selected_name().map(str::to_string);
    if state.preview_for != want {
        state.preview_for = want.clone();
        state.preview.clear();
        if let Some(name) = want {
            match voxel_core::Blueprint::load(&dir, &name).and_then(|bp| bp.resolve(&registry.0)) {
                Ok(bp) => {
                    let anchor = IVec3::from_array(bp.anchor);
                    state.preview = bp
                        .cells
                        .iter()
                        .map(|(at, voxel)| crate::ui::blueprint_panel::PreviewCell {
                            at: (IVec3::from_array(*at) - anchor).to_array(),
                            shape: voxel.shape,
                            // Unreachable by construction: the id came from
                            // `resolve` against this same registry. Magenta so a
                            // future path that breaks that is obvious on sight.
                            color: registry
                                .0
                                .get(voxel.material)
                                .map(|d| d.color)
                                .unwrap_or([1.0, 0.0, 1.0]),
                        })
                        .collect();
                }
                // A blueprint that will not load is worth knowing before you arm
                // a stamp with it, rather than at the click.
                Err(e) => state.error = Some(e),
            }
        }
    }
    
    // Esc or right-click cancels whatever is armed, wherever the cursor is.
    // The selection is deliberately kept: Clear is what discards it, so Esc is
    // never a key that quietly throws work away.
    if (pointer.right.just_pressed || input.just_pressed(GameAction::CancelTool))
        && state.armed != ArmedTool::None
    {
        state.armed = ArmedTool::None;
        state.status = Some("cancelled".to_string());
        state.error = None;
    }

    // R rotates while a tool is armed - the point of a hotkey here is that your
    // cursor is out in the world, not on the panel.
    if input.just_pressed(GameAction::RotateTool) && state.armed == ArmedTool::Stamp {
        state.yaw = state.yaw.next();
    }
    
    if pointer.left.just_pressed && state.armed != ArmedTool::None {
        if let Some(anchor) = pick.anchor {
            state.error = None;
            match state.armed {
                ArmedTool::None => {}
                ArmedTool::Region => {
                    // A completed region starts a new one, so the tool stays
                    // armed for repeated selections without re-pressing.
                    if state.corner_a.is_none() || state.corner_b.is_some() {
                        state.corner_a = Some(anchor);
                        state.corner_b = None;
                        state.status = Some("corner A set — click the opposite corner".to_string());
                    } else {
                        state.corner_b = Some(anchor);
                        state.armed = ArmedTool::None;
                        state.status = Some("region set".to_string());
                    }
                }
                ArmedTool::Stamp => {
                    // Held Shift keeps the tool armed so the same blueprint can
                    // be placed repeatedly without re-arming between clicks.
                    if !input.pressed(GameAction::RepeatTool) {
                        state.armed = ArmedTool::None;
                    }
                    let Some(name) = state.selected_name().map(str::to_string) else {
                        state.error = Some("no blueprint selected".to_string());
                        return;
                    };
                    match voxel_core::Blueprint::load(&dir, &name)
                        .and_then(|bp| bp.resolve(&registry.0))
                    {
                        Ok(blueprint) => {
                            let cells = blueprint.cells.len();
                            // One above the clicked surface voxel, matching the
                            // rule `place_blueprints` uses on the generation side.
                            let origin = anchor + IVec3::Y;
                            // The outcome is discarded on purpose: a stamp fills
                            // only `mesh_invalidated`, and `meshing_tick_system`
                            // acts on the chunk's `mesh_dirty` flag every frame.
                            let yaw = state.yaw;
                            match world.0.execute(MutationCommand::authoring(
                                WorldMutation::StampBlueprint { origin, blueprint, yaw },
                            )) {
                                Ok(_) => {
                                    state.status = Some(format!(
                                        "stamped {name} ({cells} cells) at {origin}, {}",
                                        yaw.label()
                                    ))
                                }
                                Err(e) => state.error = Some(e.to_string()),
                            }
                        }
                        Err(e) => state.error = Some(e),
                    }
                }
            }
        }
    }

    if std::mem::take(&mut state.capture_requested) {
        state.status = None;
        state.error = None;
        match (state.corner_a, state.corner_b) {
            (Some(a), Some(b)) => {
                let result = crate::world::blueprint::capture(
                    &world.0,
                    &state.name,
                    a.min(b),
                    a.max(b),
                    &registry.0,
                )
                    .and_then(|bp| bp.save(&dir).map(|()| bp.cells.len()));
                match result {
                    Ok(cells) => {
                        state.status = Some(format!(
                            "captured {cells} solid cells to {}.blueprint.json",
                            state.name
                        ));
                        state.refresh_requested = true;
                    }
                    Err(e) => state.error = Some(e),
                }
            }
            _ => state.error = Some("set both corners first".to_string()),
        }
    }
}

/// Cyan, to stay distinct from the pick highlight's yellow.
const SELECTION_COLOR: [f32; 3] = [0.2, 0.9, 1.0];

/// Mirror the blueprint panel's capture region into the debug-line pass.
/// 
/// A separate system from `blueprint_authoring_system` because that one has
/// early returns in its click handling, and a tail appended after them would be
/// skipped on exactly the paths that change the selection.
pub fn blueprint_selection_gizmo_system(
    ui: Res<UiState>,
    ctx: Res<RenderContext>,
    mut debug_lines: ResMut<DebugLinePass>,
) {
    let state = &ui.blueprint_panel;
    // A collapsed section shows no gizmo: the selection is still held, but
    // there is nothing on screen claiming to explain the box.
    let corners = if state.open {
        match (state.corner_a, state.corner_b) {
            (Some(a), Some(b)) => Some((a, b)),
            // Mid-gesture: outline the single voxel already committed, so the
            // first click has visible feedback rather than only a panel line.
            (Some(a), None) => Some((a, a)),
            _ => None,
        }
    } else {
        None
    };
    debug_lines.set_selection(&ctx, corners, SELECTION_COLOR);
}

/// The heavy path, in the color the decomposition is supposed to find.
const SKELETON_TRUNK_COLOR: [f32; 3] = [1.0, 0.55, 0.15];

/// Canopy lobes, distinct from every branch color.
const SKELETON_LOBE_COLOR: [f32; 3] = [0.35, 0.85, 0.40];

/// Voxel cells the wood occupies - what collides and what can be mined.
const SKELETON_WOOD_COLOR: [f32; 3] = [0.75, 0.55, 0.30];

/// Outlines of the capsules the impostor pass actually draws.
const SKELETON_CAPSULE_COLOR: [f32; 3] = [0.95, 0.85, 0.45];

/// A per-chain color for everything that is not the trunk. Hashed rather than
/// cycled through a table, so adjacent chain ids do not land on adjacent hues.
fn skeleton_chain_color(chain: u32) -> [f32; 3] {
    if chain == 0 {
        return SKELETON_TRUNK_COLOR;
    }
    let h = (chain as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    [
        0.30 + ((h >> 8) & 0xFF) as f32 / 365.0,
        0.30 + ((h >> 24) & 0xFF) as f32 / 365.0,
        0.30 + ((h >> 40) & 0xFF) as f32 / 365.0,
    ]
}

/// Derive and draw the debug tree skeleton at its placed anchor.
///
/// **Armed placement.** A latch on the last picked voxel is not enough: it
/// survives the pointer moving onto the panel, but the trip across the world to
/// get there re-anchors it, so every reach for a slider moved the tree. A click
/// places it and it stays. Same correction as 16b.
pub fn skeleton_debug_system(
    pick: Res<PickState>,
    pointer: Res<PointerState>,
    input: Res<InputState>,
    ctx: Res<RenderContext>,
    mut ui: ResMut<UiState>,
    mut debug_lines: ResMut<DebugLinePass>,
) {
    use crate::input::GameAction;
    use crate::rendering::debug_lines::DebugLineVertex;
    use crate::ui::blueprint_panel::ArmedTool;
    use crate::world::chunk::VOXEL_SCALE;
    use glam::Vec3;
    use nodegraph_eval::skeleton::Skeleton;

    // A blueprint tool owns the click while one is armed. Both systems read the
    // same left press, and two tools acting on one click reads as a random
    // extra action rather than as a mode error. Disarming rather than merely
    // deferring keeps the HUD honest about what the next click will do.
    let blueprint_armed = ui.blueprint_panel.armed != ArmedTool::None;

    let state = &mut ui.skeleton_panel;
    if blueprint_armed {
        state.armed = false;
    }

    // Esc or right-click cancels the arming. The origin is deliberately kept -
    // Clear is what discards it, so Esc is never a key that quietly throws
    // placement away. Decision H3, applied to the second armed tool.
    if state.armed && (pointer.right.just_pressed || input.just_pressed(GameAction::CancelTool)) {
        state.armed = false;
    }

    if state.armed && pointer.left.just_pressed {
        // `picking_system` clears the anchor while egui wants the pointer, so
        // the click that armed this can never also place it.
        if let Some(anchor) = pick.anchor {
            state.origin = Some(anchor);
            // Held Shift keeps the tool armed, so the preview can be walked
            // around the world without re-arming between clicks.
            if !input.pressed(GameAction::RepeatTool) {
                state.armed = false;
            }
        }
    }

    if std::mem::take(&mut state.save_requested) {
        // Disk is authoritative, the rule hot reload already runs on (blueprint
        // decision E2). There is no unsaved in-memory species to disagree with
        // the file a `PlaceTree` node will resolve.
        let file = nodegraph_hotreload::SpeciesFile {
            version: nodegraph_hotreload::SPECIES_VERSION,
            name: state.name.clone(),
            species: state.species,
        };
        match file.save(&crate::world::blueprint::species_dir()) {
            Ok(()) => {
                state.status = Some(format!("saved {}.species.json", state.name));
                state.error = None;
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }

    let key = match (state.open, state.origin) {
        (true, Some(v)) => Some((
            v,
            state.seed,
            state.age,
            state.species,
            state.show_radii,
            state.show_wood,
            state.show_capsules,
        )),
        _ => None,
    };
    if key == state.built {
        return;
    }
    state.built = key;

    let Some((origin, seed, age, species, show_radii, show_wood, show_capsules)) = key else {
        debug_lines.set_overlay(&ctx, None);
        state.nodes = 0;
        state.chains = 0;
        state.lobes = 0;
        state.wood = 0;
        state.reach = 0.0;
        state.height = 0.0;
        return;
    };

    let skeleton = Skeleton::derive(seed as u64, &species, age);

    // The root stands on top of the picked voxel, centered in it - matching
    // where `place_blueprints` and the stamp tool put an anchored thing.
    let base = Vec3::new(
        (origin.x as f32 + 0.5) * VOXEL_SCALE,
        (origin.y as f32 + 1.0) * VOXEL_SCALE,
        (origin.z as f32 + 0.5) * VOXEL_SCALE,
    );

    let mut lines: Vec<DebugLineVertex> = Vec::new();
    for seg in skeleton.segments() {
        let color = skeleton_chain_color(seg.chain);
        let a = base + seg.a * VOXEL_SCALE;
        let b = base + seg.b * VOXEL_SCALE;
        lines.push(DebugLineVertex { position: a.to_array(), color });
        lines.push(DebugLineVertex { position: b.to_array(), color });
        if show_radii {
            // A cross at the child end, as wide as the branch is thick. A line
            // drawing has no thickness, so this is the only way the pipe
            // model's taper is visible at all.
            for axis in [Vec3::X, Vec3::Z] {
                let arm = axis * seg.rb * VOXEL_SCALE;
                lines.push(DebugLineVertex { position: (b - arm).to_array(), color });
                lines.push(DebugLineVertex { position: (b + arm).to_array(), color });
            }
        }
        if show_capsules {
            // A ring at each end plus four rails: the taper between them is the
            // thing being judged, and a cross at one end cannot show it.
            let axis = b - a;
            let len = axis.length();
            if len > 1e-5 {
                let dir = axis / len;
                let u = dir.any_orthonormal_vector();
                let v = dir.cross(u);
                const RING: usize = 12;
                let step = std::f32::consts::TAU / RING as f32;
                for (center, radius) in [(a, seg.ra * VOXEL_SCALE), (b, seg.rb * VOXEL_SCALE)] {
                    for s in 0..RING {
                        let at = |t: f32| center + (u * t.cos() + v * t.sin()) * radius;
                        lines.push(DebugLineVertex {
                            position: at(s as f32 * step).to_array(),
                            color: SKELETON_CAPSULE_COLOR,
                        });
                        lines.push(DebugLineVertex {
                            position: at((s + 1) as f32 * step).to_array(),
                            color: SKELETON_CAPSULE_COLOR,
                        });
                    }
                }
                for k in 0..4 {
                    let t = k as f32 * std::f32::consts::FRAC_PI_2;
                    let off = u * t.cos() + v * t.sin();
                    lines.push(DebugLineVertex {
                        position: (a + off * (seg.ra * VOXEL_SCALE)).to_array(),
                        color: SKELETON_CAPSULE_COLOR,
                    });
                    lines.push(DebugLineVertex {
                        position: (b + off * (seg.rb * VOXEL_SCALE)).to_array(),
                        color: SKELETON_CAPSULE_COLOR,
                    });
                }
            }
        }
    }

    // Three orthogonal rings per lobe, so an ellipsoid reads as a volume in a
    // line drawing rather than as a circle.
    for lobe in &skeleton.lobes {
        let c = base + lobe.center * VOXEL_SCALE;
        let r = lobe.radii() * VOXEL_SCALE;
        const SEGS: usize = 20;
        for axis in 0..3 {
            let at = |t: f32| match axis {
                0 => Vec3::new(r.x * t.cos(), r.y * t.sin(), 0.0),
                1 => Vec3::new(0.0, r.y * t.cos(), r.z * t.sin()),
                _ => Vec3::new(r.x * t.cos(), 0.0, r.z * t.sin()),
            };
            for s in 0..SEGS {
                let step = std::f32::consts::TAU / SEGS as f32;
                lines.push(DebugLineVertex {
                    position: (c + at(s as f32 * step)).to_array(),
                    color: SKELETON_LOBE_COLOR,
                });
                lines.push(DebugLineVertex {
                    position: (c + at((s + 1) as f32 * step)).to_array(),
                    color: SKELETON_LOBE_COLOR,
                });
            }
        }
    }

    // What the tree occupies in the voxel grid: the part that collides, can be
    // mined, and that segment emission will later gate on. Deduplicated,
    // because `voxelize` visits overlapping capsules and says so.
    let root_voxels = origin.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
    let mut wood: std::collections::HashSet<glam::IVec3> = std::collections::HashSet::new();
    skeleton.voxelize(root_voxels, |c| {
        wood.insert(c);
    });
    if show_wood {
        for cell in &wood {
            lines.extend(voxel_box_lines(*cell, SKELETON_WOOD_COLOR));
        }
    }

    debug_lines.set_overlay(&ctx, Some(&lines));
    state.wood = wood.len();
    state.nodes = skeleton.nodes.len();
    state.chains = skeleton.chains;
    state.lobes = skeleton.lobes.len();
    state.reach = skeleton.reach;
    state.height = skeleton.height;
}

/// Sample the biome/zone map when requested (roadmap §4.3).
///
/// On the pool and blocking, like the column inspector: a 128² grid is ~33k
/// noise evaluations, and it runs on an explicit change rather than per frame.
pub fn biome_map_system(
    mut ui: ResMut<UiState>,
    world: Res<VoxelWorld>,
    sim: Res<SimulationManager>,
    jobs: Res<JobSystem>,
) {
    // Mirrored every frame so "Camera" is current whenever it is pressed.
    let cam = sim.camera.smooth_target;
    ui.biome_map.camera_x = cam.x as i32;
    ui.biome_map.camera_z = cam.z as i32;

    let state = &mut ui.biome_map;
    if !state.open {
        return;
    }
    state.ensure_defaults();
    if !state.dirty {
        return;
    }
    state.dirty = false;

    let (cx, cz, step) = (state.center_x, state.center_z, state.step);
    let generator = std::sync::Arc::clone(&world.0.generator);
    let dim = crate::ui::biome_map::MAP_DIM;
    let result = jobs
        .pool()
        .install(move || generator.sample_id_map(cx, cz, step, dim));

    match result {
        Ok(map) => {
            state.zone_legend = world.0.generator.zone_labels().to_vec();
            state.biome_legend = world.0.generator.biome_labels().to_vec();
            state.map = Some(map);
            state.version = state.version.wrapping_add(1);
            state.error = None;
        }
        Err(e) => {
            state.map = None;
            state.error = Some(e);
        }
    }
}

// ==========================================================================
// Streaming stage
// ==========================================================================

pub fn streaming_tick_system(
    mut streaming: ResMut<ChunkStreamingManager>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut jobs: ResMut<JobSystem>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut capsules: ResMut<CapsulePass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    _ctx: Res<RenderContext>,
    persistence: Res<WorldPersistence>,
    mut timings: ResMut<FrameTimings>,
) {
    // Explicit start/stop rather than `sub_timer`: the guard would hold
    // `timings` borrowed for the whole body, and the per-phase copy below needs
    // it free. Safe here specifically because this system has one exit — the
    // condition the guard exists to protect against does not apply.
    let started = std::time::Instant::now();
    let cam_pos = sim.camera.smooth_target;
    let cam_cx = (cam_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let cam_cz = (cam_pos.z / CHUNK_WORLD_SIZE).floor() as i32;
    let camera_view = CameraView {
        zoom: sim.camera.zoom,
        aspect: sim.camera.aspect,
        rotation: sim.camera.rotation,
        camera_chunk: IVec3::new(cam_cx, 0, cam_cz),
        camera_world_pos: cam_pos,
        world_y_range: (
            world.min_chunk_y as f32 * CHUNK_WORLD_SIZE,
            world.max_chunk_y as f32 * CHUNK_WORLD_SIZE,
        ),
    };

    let tick_result = streaming.tick(
        &mut world.0,
        &mut meshing,
        &mut jobs,
        &camera_view,
        frame.dt,
        &mut |chunk: &crate::world::chunk::LoadedChunk| {
            if chunk.persist_dirty {
                if let Err(e) = persistence.save_chunk_on_unload(chunk) {
                    log::error!("Save on unload {:?}: {e}", chunk.data.coord);
                }
            }
        },
    );

    // Remove detail paint and water for unloaded chunks
    for pos in &tick_result.unloaded {
        detail_paint.remove_chunk(*pos);
        scatter.remove_chunk(*pos);
        capsules.remove_chunk(*pos);
        water_pass.remove_chunk_water(*pos);
    }

    // NOTE: We do NOT add foliage/water here for inserted chunks.
    // Inserted chunks don't have meshes yet. Foliage/water are added
    // in meshing_tick_system when the mesh upload completes, so they
    // appear on the same frame as the terrain.

    // Update streaming stats for UI
    ui.streaming = streaming.stats();

    // The four phases `tick` reported, named here (see `SUB_DRAIN`), plus the
    // whole-system span so the inner split can be checked against it.
    for (slot, ms) in [SUB_DRAIN, SUB_SUBMIT, SUB_SCAN, SUB_UNLOAD]
        .into_iter()
        .zip(tick_result.phase_ms)
    {
        timings.add_sub(slot, ms);
    }
    timings.add_sub(SUB_STREAM, started.elapsed().as_secs_f32() * 1000.0);
}

/// Dispatch queued jobs onto the shared pool (P14). Runs after every consumer
/// has submitted for this frame and before meshing consumes results, so a job
/// submitted this frame starts this frame.
pub fn job_pump_system(
    mut jobs: ResMut<JobSystem>,
    mut ui: ResMut<UiState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_PUMP);
    jobs.pump();
    // Snapshot after pumping, so the panel shows the dispatch this frame
    // produced rather than the state that led to it.
    for kind in crate::jobs::JobKind::ALL {
        ui.job_stats[kind.index()] = jobs.stats(kind);
    }
    ui.job_running_total = jobs.running_total();
    ui.job_max_running = jobs.max_running();
}

// ==========================================================================
// Meshing stage
// ==========================================================================

pub fn meshing_tick_system(
    mut meshing: ResMut<MeshingCoordinator>,
    mut world: ResMut<VoxelWorld>,
    mut jobs: ResMut<JobSystem>,
    sim: Res<SimulationManager>,
    ctx: Res<RenderContext>,
    mut ui: ResMut<UiState>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut capsules: ResMut<CapsulePass>,
    mut water_pass: ResMut<WaterPass>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_UPLOAD);
    // Phase 1: tick meshing with &mut world - collects meshed positions.
    let cam = sim.camera.smooth_target;
    let camera_chunk = IVec3::new(
        (cam.x / CHUNK_WORLD_SIZE).floor() as i32,
        0,
        (cam.z / CHUNK_WORLD_SIZE).floor() as i32,
    );
    // Read before `ui` is mutably borrowed by the call below.
    let max_snapshots = ui.params.streaming.max_mesh_per_frame as usize;
    let meshed = meshing.tick(
        &mut world.0,
        &mut jobs,
        &ctx,
        &mut ui,
        camera_chunk,
        max_snapshots,
    );

    // Phase 2: for each newly meshed chunk, build per-chunk foliage
    // and water GPU buffers. world.0 is now borrowed immutably.
    if !meshed.is_empty() {
        for pos in &meshed {
            detail_paint.add_chunk(*pos, &world.0, &ctx.device);
            scatter.add_chunk(*pos, &world.0, &ctx.device);
            capsules.add_chunk(*pos, &world.0, &ctx.device);
        }
        // Water welds across seams, so also refresh newly-meshed chunks' neighbors
        // (detail/scatter are per-chunk and don't need this).
        rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, meshed.iter().copied());
    }
}

/// Cross-chunk seam finalization (Substep 2b): once a chunk's six face
/// neighbors are resident, derive the boundary slab demotions the isolated
/// generation pass could not see (surface steps that straddle a chunk border,
/// including chunk-Y seams). One-shot per chunk (`seam_finalized`) and bounded
/// per frame; runs before meshing so the first mesh already carries the seam
/// slabs.
///
/// The derivation is read-only and needs neighbor storage, so it stays here; the
/// write goes through the mutation door as `FinalizeSeam` (P5 - this was the
/// largest remaining bypass, audit §7.1 E-1). It is `System` origin: generation
/// finalization, not an actor edit.
pub fn seam_smoothing_system(
    mut world: ResMut<VoxelWorld>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_SEAM);
    const MAX_PER_FRAME: usize = 16;

    let mut candidates: Vec<IVec3> = Vec::new();
    for (coord, chunk) in world.0.chunks.iter() {
        if chunk.seam_finalized {
            continue;
        }
        let pos: IVec3 = (*coord).into();
        if world.0.has_face_neighbors(pos) {
            candidates.push(pos);
            if candidates.len() >= MAX_PER_FRAME {
                break;
            }
        }
    }

    for pos in candidates {
        let nb = crate::world::seam::NeighborStorages::capture(&world.0, pos);
        let distances = world
            .0
            .get_chunk(pos)
            .and_then(|c| c.smoothing_distances.clone());
        let demotions = match &distances {
            Some(d) => crate::world::seam::seam_demotions(&nb, d.as_ref()),
            None => Vec::new(),
        };

        world
            .0
            .execute(MutationCommand::system(WorldMutation::FinalizeSeam {
                chunk: pos,
                demotions,
            }))
            .expect("system seam finalization accepted in any mode");
    }
}

// ==========================================================================
// UniformWrite stage
// ==========================================================================

pub fn write_uniforms_system(
    ctx: Res<RenderContext>,
    surface: Res<SurfaceState>,
    frame: Res<FrameState>,
    ui: Res<UiState>,
    sim: Res<SimulationManager>,
    global_buf: Res<GlobalUniformBuffer>,
    shadow_buf: Res<ShadowUniformBuffer>,
    reflection_buf: Res<ReflectionUniformBuffer>,
    post_process: Res<PostProcessPass>,
    outline: Res<OutlinePass>,
    palette_pass: Res<PalettePass>,
    upscale: Res<UpscalePass>,
    water_pass: Res<WaterPass>,
    render_targets: Res<RenderTargets>,
    loaded_palette: Res<LoadedPalette>,
) {
    let reflect_plane = water_pass.avg_surface_y;
    crate::rendering::uniform_writer::write_all_uniforms(
        &ctx,
        &frame,
        &ui.params,
        &sim.cloud_shadow,
        &global_buf.0,
        &shadow_buf.0,
        &reflection_buf.0,
        reflect_plane,
        &post_process,
        &outline,
        &palette_pass,
        &upscale,
        &water_pass,
        &render_targets,
        &loaded_palette.0,
        surface.surface_config.width,
        surface.surface_config.height,
    );
}

/// Rebuild + upload the 64^3 visibility mask when the flood changed, and drive the
/// mask uniforms every frame (FrameState is rebuilt per frame). The mask marks the
/// visible set dilated by one cell - the +1 pulls in the solid cave-wall shell so
/// walls render; everything else stays 0 (capped), which is what hides the surface
/// world (R3). Runs in UniformWrite before `write_uniforms_system`.
pub fn visibility_mask_upload_system(
    ctx: Res<RenderContext>,
    mut room: ResMut<crate::world::room::RoomState>,
    mask: Res<VisibilityMask>,
    mut frame: ResMut<FrameState>,
    world: Res<VoxelWorld>,
) {
    const N: i32 = 64;
    const HALF: i32 = 32;

    // Drive the uniform every frame from the current window center.
    let Some(head) = room.last_origin else {
        frame.mask_enabled = 0;
        return;
    };
    let origin = head - IVec3::splat(HALF); // min corner of the window (world cells)
    frame.mask_origin = [origin.x as f32, origin.y as f32, origin.z as f32];
    frame.mask_enabled = if room.enclosed { 1 } else { 0 };
    frame.volume_radius = room.volume_radius;

    // Re-upload texture contents only when the flood actually changed.
    if !room.dirty {
        return;
    }
    room.dirty = false;

    let mut bytes = vec![0u8; (N * N * N) as usize];
    for (&cell, &cost) in room.visible.iter() {
        let l = cell - origin;
        if l.x >= 0 && l.x < N && l.y >= 0 && l.y < N && l.z >= 0 && l.z < N {
            // Low 7 bits = flood cost + 1 (0 = unknown). High bit flags a slab cell:
            // visible for rendering, but half-solid - the shader's occluder march must
            // not count it as free space behind a face, or slab floors/slopes cull the
            // faces sitting in front of them. (Visible cells are empty or slab; a slab
            // has a solid_interval, empty does not.)
            let slab_bit: u8 = if world.0.solid_interval(cell).is_some() { 0x80 } else { 0 };
            bytes[(l.x + l.y * N + l.z * N * N) as usize] = ((cost + 1).min(0x7F) as u8) | slab_bit;
        }
    }

    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &mask.0,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(N as u32),
            rows_per_image: Some(N as u32),
        },
        wgpu::Extent3d { width: N as u32, height: N as u32, depth_or_array_layers: N as u32 },
    );
}

/// Re-upload the material color table each frame so live material-color edits in
/// the UI recolor terrain immediately (the color is composed in the shader from
/// this table, not baked into the mesh). The table is `MATERIAL_COLOR_SLOTS`
/// `vec4`s (~4 KB) - cheap enough to write unconditionally.
pub fn material_color_upload_system(
    ctx: Res<RenderContext>,
    ui: Res<UiState>,
    buffer: Res<MaterialColorBuffer>,
) {
    let table = crate::rendering::uniforms::material_color_table(&ui.params.materials);
    ctx.queue.write_buffer(&buffer.0, 0, bytemuck::cast_slice(&table));
}

pub fn compute_stats_system(
    world: Res<VoxelWorld>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    jobs: Res<JobSystem>,
    mut ui: ResMut<UiState>,
) {
    ui.frame_time_ms = frame.dt * 1000.0;
    let mut total_tris: u64 = 0;
    let mut chunks_visible: u32 = 0;
    let mut chunks_total: u32 = 0;
    for chunk in world.0.chunks.values() {
        if chunk.mesh.is_some() {
            chunks_total += 1;
            if sim.frustum.is_chunk_visible(chunk.data.coord.into()) {
                chunks_visible += 1;
                total_tris += chunk.mesh.as_ref().unwrap().index_count as u64 / 3;
            }
        }
    }
    ui.total_triangles = total_tris;
    ui.chunks_visible = chunks_visible;
    ui.chunks_total = chunks_total;

    ui.mutation_log = world.0.mutation_log();

    // Explicit operator action: ~9 ms per chunk regenerated, so it produces one
    // long frame by design rather than being spread across many.
    if ui.determinism_check_requested {
        ui.determinism_check_requested = false;
        let cam = sim.camera.smooth_target;
        let camera_chunk = IVec3::new(
            (cam.x / CHUNK_WORLD_SIZE).floor() as i32,
            0,
            (cam.z / CHUNK_WORLD_SIZE).floor() as i32,
        );
        ui.determinism = world.0.verify_determinism(32, camera_chunk, jobs.pool());
    }

    // Residency sampling walks every resident chunk summing per-layer bytes, so
    // it runs about once a second rather than joining the per-frame full-set
    // scans that audit §4.9 identifies as the secondary cost at high zoom.
    if ui.residency_countdown == 0 {
        ui.residency = world.0.sample_residency(ui.residency);
        ui.residency_countdown = 60;
    } else {
        ui.residency_countdown -= 1;
    }
}

// ==========================================================================
// Render stage — exclusive system (takes &mut World)
// ==========================================================================

pub fn render_present_system(ecs: &mut bevy_ecs::world::World) {
    // Skip rendering when the window has zero dimensions (e.g. during shutdown).
    {
        let surface = ecs.resource::<SurfaceState>();
        if surface.surface_config.width == 0 || surface.surface_config.height == 0 {
            return;
        }
    }

    // Clone is cheap — Device/Queue are Arc handles internally.
    // This avoids all borrow conflicts with resource_mut calls below.
    let ctx = ecs.resource::<RenderContext>().clone();

    // --- Render target resize check ---
    let dims = {
        let surface = ecs.resource::<SurfaceState>();
        let sim = ecs.resource::<SimulationManager>();
        let ui = ecs.resource::<UiState>();
        compute_render_dimensions(
            surface.surface_config.width,
            surface.surface_config.height,
            sim.camera.zoom,
            ui.params.render_pipeline.world_pixel_density,
        )
    };

    if ecs.resource::<RenderTargets>().needs_recreate(&dims) {
        let new_rt = RenderTargets::new(&ctx, &dims);

        let water_layout = &ecs.resource::<PipelineResources>().water_bind_group_layout.clone();
        ecs.resource_mut::<WaterPass>().rebuild_bind_group(
            &ctx.device,
            water_layout,
            &new_rt.scene_copy_view,
            &new_rt.depth_sample_view,
            &new_rt.reflection_color_view,
            &new_rt.reflection_depth_sample_view,
        );
        ecs.resource_mut::<OutlinePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
            &new_rt.depth_sample_view,
            &new_rt.normal_view,
        );
        ecs.resource_mut::<PostProcessPass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
            &new_rt.depth_sample_view,
        );
        ecs.resource_mut::<PalettePass>().rebuild_bind_group(
            &ctx,
            &new_rt.scene_view,
        );
        ecs.resource_mut::<UpscalePass>().rebuild_bind_group(
            &ctx,
            &new_rt.processed_view,
        );

        *ecs.resource_mut::<RenderTargets>() = new_rt;
    }
    // Zoom moves view/k/s continuously without reallocating - keep fields fresh.
    ecs.resource_mut::<RenderTargets>().update_view(&dims);

    // --- Acquire swapchain ---
    // Timed separately from the rest of the Render stage: under Fifo this call
    // is where the frame blocks on vsync, which is waiting rather than work.
    let acquire_start = std::time::Instant::now();
    let surface_result = ecs.resource::<SurfaceState>().surface.get_current_texture();
    let acquire_ms = acquire_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<FrameTimings>().add_present(acquire_ms);
    let surface_frame = match surface_result {
        Ok(f) => f,
        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
            let size = ecs.resource::<WindowHandle>().0.inner_size();
            ecs.resource_mut::<SurfaceState>().resize(&ctx.device, size.width, size.height);
            return;
        }
        Err(wgpu::SurfaceError::Timeout) => {
            log::warn!("Surface timeout");
            return;
        }
        Err(e) => {
            log::error!("Surface error: {:?}", e);
            return;
        }
    };

    let surface_view = surface_frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // --- Debug lines update ---
    let show_debug = ecs.resource::<UiState>().params.debug.show_chunk_boundaries;
    if show_debug {
        let chunk_positions: Vec<IVec3> = ecs
            .resource::<VoxelWorld>()
            .0
            .chunks
            .keys()
            .map(|c| IVec3::from(*c))
            .collect();
        ecs.resource_scope::<DebugLinePass, _>(|ecs, mut debug| {
            let sim = ecs.resource::<SimulationManager>();
            debug.update(&ctx, &chunk_positions, &sim.frustum);
        });
    }

    // --- Cap mesh update ---
    let (cap_enabled, cos_r, sin_r, clip_max, clip_min) = {
        let frame = ecs.resource::<FrameState>();
        let cs_enabled = ecs.resource::<UiState>().params.cross_section.enabled;
        (
            cs_enabled && frame.clip_enabled == 1,
            frame.cos_r,
            frame.sin_r,
            frame.clip_max,
            frame.clip_min,
        )
    };

    if cap_enabled {
        let clip_dirs: [f32; 3] = [
            if cos_r >= 0.0 { 1.0 } else { -1.0 },
            1.0,
            if sin_r >= 0.0 { 1.0 } else { -1.0 },
        ];
        let clip_pos = [
            if clip_dirs[0] > 0.0 { clip_max[0] } else { clip_min[0] },
            clip_max[1],
            if clip_dirs[2] > 0.0 { clip_max[2] } else { clip_min[2] },
        ];
        ecs.resource_scope::<CapPass, _>(|ecs, mut cap| {
            let world = ecs.resource::<VoxelWorld>();
            let ui = ecs.resource::<UiState>();
            cap.maybe_rebuild(&ctx, &world.0, &ui.params.cross_section, clip_pos, clip_dirs);
        });
    }

    // --- Build and execute render graph ---
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("render_encoder"),
    });

    {
        let ui = ecs.resource::<UiState>();
        let world = ecs.resource::<VoxelWorld>();
        let sim = ecs.resource::<SimulationManager>();
        let pipeline_registry = ecs.resource::<PipelineRegistry>();
        let shadow_bind_group = ecs.resource::<ShadowBindGroup>();
        let shadow_depth_view = ecs.resource::<ShadowDepthView>();
        let uniform_bind_group = ecs.resource::<GlobalUniformBindGroup>();
        let render_targets = ecs.resource::<RenderTargets>();
        let detail_paint_pass = ecs.resource::<DetailPaintPass>();
        let scatter_pass = ecs.resource::<ScatterPass>();
        let capsule_pass = ecs.resource::<CapsulePass>();
        let water_pass = ecs.resource::<WaterPass>();
        let debug_line_pass = ecs.resource::<DebugLinePass>();
        let cap_pass = ecs.resource::<CapPass>();
        let outline_pass = ecs.resource::<OutlinePass>();
        let post_process = ecs.resource::<PostProcessPass>();
        let palette_pass = ecs.resource::<PalettePass>();
        let upscale_pass = ecs.resource::<UpscalePass>();

        let terrain_pipeline = if ui.params.debug.show_wireframe {
            &pipeline_registry.terrain_wireframe_pipeline
        } else {
            &pipeline_registry.terrain_pipeline
        };

        // Compute shadow frustum from the light-space (shadow VP) matrix.
        // Frustum::from_view_projection works for orthographic projections too.
        let frame = ecs.resource::<FrameState>();
        let shadow_frustum = Frustum::from_view_projection(frame.light_space);

        // Pre-filter chunks: one list for the camera frustum (main scene),
        // one for the shadow frustum (shadow pass).
        let visible_chunks: Vec<&LoadedChunk> = world.0.chunks.values()
            .filter(|c| c.mesh.is_some() && sim.frustum.is_chunk_visible(c.data.coord.into()))
            .collect();
        let shadow_chunks: Vec<&LoadedChunk> = world.0.chunks.values()
            .filter(|c| c.mesh.is_some() && shadow_frustum.is_chunk_visible(c.data.coord.into()))
            .collect();

        let shadow_node = ShadowPassNode {
            pipeline: &pipeline_registry.shadow_pipeline,
            bind_group: &shadow_bind_group.0,
            shadow_depth_view: &shadow_depth_view.0,
            chunks: &shadow_chunks,
            capsule_pass: ecs.resource::<CapsulePass>(),
            capsule_shadow_pipeline: &pipeline_registry.capsule_shadow_pipeline,
        };
        
        let player_pass = ecs.resource::<PlayerPass>();

        let room_enclosed = ecs.resource::<crate::world::room::RoomState>().enclosed;

        let sky = if room_enclosed {
            [0.02, 0.02, 0.028] // dark void-gray while enclosed; the wireframe sits just above this
        } else {
            ui.params.render_pipeline.sky_color
        };
        let scene_node = MainScenePassNode {
            sky_color: wgpu::Color {
                r: sky[0] as f64,
                g: sky[1] as f64,
                b: sky[2] as f64,
                a: 1.0,
            },
            terrain_pipeline,
            uniform_bind_group: &uniform_bind_group.0,
            chunks: &visible_chunks,
            frustum: &sim.frustum,
            cap_pass: &cap_pass,
            cap_config: CapConfig {
                enabled: cap_enabled,
                clip_dirs: [
                    if cos_r >= 0.0 { 1.0 } else { -1.0 },
                    1.0,
                    if sin_r >= 0.0 { 1.0 } else { -1.0 },
                ],
                clip_pos: [0.0; 3],
            },
            detail_paint_pass: &detail_paint_pass,
            detail_paint_pipeline: &pipeline_registry.detail_paint_pipeline,
            scatter_pass: &scatter_pass,
            scatter_pipeline: &pipeline_registry.scatter_pipeline,
            capsule_pass: &capsule_pass,
            capsule_pipeline: &pipeline_registry.capsule_pipeline,
            debug_line_pass: &debug_line_pass,
            show_debug_lines: ui.params.debug.show_chunk_boundaries,
            hide_foliage: ui.params.debug.hide_foliage,
            player_pass: &player_pass,
        };

        let reflection_bind_group = ecs.resource::<ReflectionUniformBindGroup>();
        let reflection_node = ReflectionPassNode {
            color_view: &render_targets.reflection_color_view,
            normal_view: &render_targets.reflection_normal_view,
            depth_view: &render_targets.reflection_depth_view,
            sky_color: wgpu::Color { r: sky[0] as f64, g: sky[1] as f64, b: sky[2] as f64, a: 1.0 },
            terrain_pipeline: &pipeline_registry.terrain_reflection_pipeline,
            reflection_bind_group: &reflection_bind_group.0,
            chunks: &visible_chunks,
            frustum: &sim.frustum,
            detail_paint_pass: &detail_paint_pass,
            detail_paint_pipeline: &pipeline_registry.detail_paint_pipeline,
            scatter_pass: &scatter_pass,
            scatter_pipeline: &pipeline_registry.scatter_pipeline,
            capsule_pass: &capsule_pass,
            capsule_pipeline: &pipeline_registry.capsule_pipeline,
            player_pass: &player_pass,
            hide_foliage: ui.params.debug.hide_foliage,
            enabled: !ui.params.debug.hide_water && !water_pass.chunk_meshes.is_empty(),
        };

        let water_node = crate::rendering::water_scene_pass::WaterScenePassNode {
            pipeline: &pipeline_registry.water_pipeline,
            global_bind_group: &uniform_bind_group.0,
            water_pass: &water_pass,
            frustum: &sim.frustum,
            hide_water: ui.params.debug.hide_water,
            scene_tex: &render_targets.scene,
            scene_copy_tex: &render_targets.scene_copy,
            tex_size: wgpu::Extent3d {
                width: render_targets.alloc_w,
                height: render_targets.alloc_h,
                depth_or_array_layers: 1,
            },
        };

        let mut graph = RenderGraph::new();
        graph.add_pass(&shadow_node);
        graph.add_pass(&reflection_node);
        graph.add_pass(&scene_node);
        graph.add_pass(&water_node);
        graph.add_pass(&*outline_pass);
        graph.add_pass(&*post_process);
        graph.add_pass(&*palette_pass);
        graph.add_pass(&*upscale_pass);

        #[cfg(debug_assertions)]
        graph
            .validate(&[
                ResourceId::SHADOW_DEPTH,
                ResourceId::SCENE,
                ResourceId::SCENE_COPY,
                ResourceId::NORMAL,
                ResourceId::DEPTH,
                ResourceId::PROCESSED,
                ResourceId::SURFACE,
            ])
            .unwrap();

        let mut resources = ResourceMap::new();
        resources.insert(ResourceId::SCENE, &render_targets.scene_view);
        resources.insert(ResourceId::SCENE_COPY, &render_targets.scene_copy_view);
        resources.insert(ResourceId::NORMAL, &render_targets.normal_view);
        resources.insert(ResourceId::DEPTH, &render_targets.depth_view);
        resources.insert(ResourceId::PROCESSED, &render_targets.processed_view);
        resources.insert(ResourceId::SHADOW_DEPTH, &shadow_depth_view.0);
        resources.insert(ResourceId::SURFACE, &surface_view);

        graph.execute(&mut encoder, &resources);
    }

    // --- Egui draw ---
    let window_arc = ecs.resource::<WindowHandle>().0.clone();
    let registry = ecs.resource::<MaterialRegistryRes>().0.clone();
    ecs.resource_scope::<UiState, _>(|ecs, mut ui_state| {
        ecs.resource_scope::<ui::field_probe::FieldProbe, _>(|ecs, mut probe| {
            let mode = ecs.resource::<VoxelWorld>().0.mode;
            let material_idx = ecs.resource::<crate::interaction::PlayerToolState>().material_index;
            let (material_name, material_color): (String, [f32; 3]) =
                match ui_state.params.materials.entries.get(material_idx) {
                    Some(e) => (e.name.clone(), e.color),
                    None => ("-".to_string(), [0.6, 0.6, 0.6]),
                };

            let mut egui = ecs.non_send_resource_mut::<ui::EguiRenderer>();
            egui.draw(
                &mut *ui_state,
                &mut *probe,
                &registry,
                &ctx,
                &mut encoder,
                &surface_view,
                &window_arc,
                mode,
                &material_name,
                material_color,
            );
        });
    });

    // --- Submit + present ---
    ctx.queue.submit(std::iter::once(encoder.finish()));
    let present_start = std::time::Instant::now();
    surface_frame.present();
    let present_ms = present_start.elapsed().as_secs_f32() * 1000.0;
    ecs.resource_mut::<FrameTimings>().add_present(present_ms);
}

// ==========================================================================
// PostFrame stage
// ==========================================================================

pub fn world_regen_system(
    mut regen: ResMut<WorldRegenCoordinator>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    ctx: Res<RenderContext>,
    persistence: Res<WorldPersistence>,
    mut streaming: ResMut<ChunkStreamingManager>,
    mut events: EventReader<crate::world::events::WorldEvent>,
) {
    // **Bus subscriber #2**, independent of the log: it reads the same stream,
    // filters for what is cares about, and neither subscriber knows the other
    // exists. Queued rather than acted on directly, because an edit arriving
    // mid-regeneration must wait for the next idle tick.
    for event in events.read() {
        if let crate::world::events::WorldEvent::GraphChanged { slot } = event {
            regen.queue_slot(*slot);
        }
    }
    regen.tick(
        &mut world.0,
        &mut meshing,
        &mut detail_paint,
        &mut scatter,
        &mut water_pass,
        &mut ui,
        &ctx,
        &persistence,
        &mut streaming,
    );
}

pub fn persistence_autosave_system(
    mut persistence: ResMut<WorldPersistence>,
    mut world: ResMut<VoxelWorld>,
    frame: Res<FrameState>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_SAVE);
    persistence.tick_autosave(frame.dt, &mut world.0);
}
