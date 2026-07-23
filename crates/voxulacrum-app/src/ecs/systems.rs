use bevy_ecs::prelude::*;
use glam::IVec3;
use voxel_core::Voxel;
use crate::ecs::resources::*;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::params::ParamChangeKind;
use crate::rendering::cap_pass::CapPass;
use crate::rendering::debug_lines::DebugLinePass;
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
use crate::rendering::water_pass::WaterPass;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::frustum::Frustum;
use crate::shader_reload::ShaderWatcher;
use crate::graph_reload::GraphWatcherRes;
use crate::input::InputState;
use crate::interaction::PickState;
use crate::simulation::manager::{FrameState, SimulationManager};
use crate::ui;
use crate::ui::panels::UiState;
use crate::world::regen::WorldRegenCoordinator;
use crate::{compute_render_dimensions, palette, FrameCounter};
use crate::world::chunk::{LoadedChunk, CHUNK_WORLD_SIZE};
use crate::world::streaming::{CameraView, ChunkStreamingManager};
use crate::world::persistence::WorldPersistence;
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};
use crate::materials::MaterialRegistryRes;
use crate::player::sim::WorldView;

// ==========================================================================
// Input stage
// ==========================================================================

pub fn frame_counter_system(mut counter: ResMut<FrameCounter>, mut ui: ResMut<UiState>) {
    counter.tick();
    ui.fps = counter.fps;
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

/// Hot-reload the active world graph when `biome_meadow.graph.json` changes
/// on disk (external edits / the standalone preview app saving). Skips the
/// reload while the embedded editor has unsaved edits, so in-memory work and
/// the rendered world stay consistent. Routes the new graph to regen via the
/// same `UiState.pending_graph` transport the embedded editor uses.
pub fn graph_hot_reload_system(
    graph_watcher: Res<GraphWatcherRes>,
    mut egui: NonSendMut<ui::EguiRenderer>,
    mut ui_state: ResMut<UiState>,
) {
    let changed = graph_watcher.0.poll_changes();
    if changed.is_empty() {
        return;
    }
    let default_path = crate::world::world_generator::default_graph_path();
    let default_name = default_path.file_name();

    for path in changed {
        // Only the active world graph drives regeneration.
        if path.file_name() != default_name {
            continue;
        }
        // Don't clobber unsaved embedded-editor edits.
        if egui.graph_editor.is_modified() {
            log::warn!(
                "{} changed on disk but the editor has unsaved edits; \
                ignoring the disk change",
                path.display()
            );
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match nodegraph_ir::Graph::from_json(&text) {
                Ok(graph) => {
                    // Refresh the editor view, then hand the graph to regen.
                    let slot = crate::world::world_generator::GraphSlot::Biome(0);
                    // The watched file is the primary biome (biome 0).
                    egui.graph_editor.refresh(slot, graph.clone());
                    ui_state.pending_graph = Some((slot, graph));
                    log::info!("Hot-reloaded {} from disk", path.display());
                }
                Err(e) => log::warn!("graph hot-reload parse error: {e}"),
            },
            Err(e) => log::warn!("graph hot-reload read error: {e}"),
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

pub fn param_change_detection_system(mut ui: ResMut<UiState>) {
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

pub fn fluid_tick_system(
    mut clock: ResMut<FluidClock>,
    frame: Res<FrameState>,
    input: Res<InputState>,
    pick: Res<PickState>,
    mut world: ResMut<VoxelWorld>,
    mut water_pass: ResMut<WaterPass>,
    ctx: Res<RenderContext>,
) {
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
        // Phase 2: commit each plan, recording which *changed* edge cells should
        // wake their neighbor's mirror so flow keeps crossing the seam (waking
        // only changed cells avoids perpetual boundary churn once level).
        let mut wakes: Vec<(voxel_core::ChunkCoord, voxel_core::LocalPos)> = Vec::new();
        for (coord, plan) in plans {
            for &(pos, _) in plan.changed_cells() {
                fluid_edge_mirrors(coord, pos, &mut wakes);
            }
            if let Some(chunk) = world.0.chunks.get_mut(&coord) {
                if crate::world::fluid_sim::commit_chunk(&mut chunk.data.fluids, plan) {
                    changed.insert(glam::IVec3::from(coord));
                }
            }
        }
        // Phase 3: wake neighbor edge cells so they participate next tick.
        for (ncoord, mirror) in wakes {
            if let Some(chunk) = world.0.chunks.get_mut(&ncoord) {
                chunk.data.fluids.activate(mirror);
            }
        }
    }

    // Any chunk whose fluid changed must persist its field. The fluid sim runs in
    // any mode, so this bookkeeping is a System mutation, not an actor edit.
    if !changed.is_empty() {
        world
            .0
            .execute(MutationCommand::system(WorldMutation::MarkFluidDirty {
                chunks: changed.iter().copied().collect(),
            }))
            .expect("system fluid mark accepted in any mode");
    }
    for pos in changed {
        water_pass.add_chunk_water(pos, &world.0, &ctx.device);
    }
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
) {
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

/// For each chunk face `pos` lies on, push the neighbor chunk + the mirror cell
/// on the far side of that face (a corner cell yields up to three).
fn fluid_edge_mirrors(
    coord: voxel_core::ChunkCoord,
    pos: voxel_core::LocalPos,
    out: &mut Vec<(voxel_core::ChunkCoord, voxel_core::LocalPos)>,
) {
    use voxel_core::{ChunkCoord, LocalPos};
    let d = crate::world::chunk::CHUNK_SIZE as u8;
    if pos.x == 0 {
        out.push((ChunkCoord::new(coord.x - 1, coord.y, coord.z), LocalPos::new_unchecked(d - 1, pos.y, pos.z)));
    }
    if pos.x == d - 1 {
        out.push((ChunkCoord::new(coord.x + 1, coord.y, coord.z), LocalPos::new_unchecked(0, pos.y, pos.z)));
    }
    if pos.y == 0 {
        out.push((ChunkCoord::new(coord.x, coord.y - 1, coord.z), LocalPos::new_unchecked(pos.x, d - 1, pos.z)));
    }
    if pos.y == d - 1 {
        out.push((ChunkCoord::new(coord.x, coord.y + 1, coord.z), LocalPos::new_unchecked(pos.x, 0, pos.z)));
    }
    if pos.z == 0 {
        out.push((ChunkCoord::new(coord.x, coord.y, coord.z - 1), LocalPos::new_unchecked(pos.x, pos.y, d - 1)));
    }
    if pos.z == d - 1 {
        out.push((ChunkCoord::new(coord.x, coord.y, coord.z + 1), LocalPos::new_unchecked(pos.x, pos.y, 0)));
    }
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

// ==========================================================================
// Streaming stage
// ==========================================================================

pub fn streaming_tick_system(
    mut streaming: ResMut<ChunkStreamingManager>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    sim: Res<SimulationManager>,
    frame: Res<FrameState>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut water_pass: ResMut<WaterPass>,
    mut ui: ResMut<UiState>,
    _ctx: Res<RenderContext>,
    persistence: Res<WorldPersistence>,
) {
    let cam_pos = sim.camera.smooth_target;
    let cam_cx = (cam_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let cam_cz = (cam_pos.z / CHUNK_WORLD_SIZE).floor() as i32;
    let camera_view = CameraView {
        zoom: sim.camera.zoom,
        aspect: sim.camera.aspect,
        rotation: sim.camera.rotation,
        camera_chunk: IVec3::new(cam_cx, 0, cam_cz),
        camera_world_pos: cam_pos,
    };

    let tick_result = streaming.tick(
        &mut world.0,
        &mut meshing,
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
        water_pass.remove_chunk_water(*pos);
    }

    // NOTE: We do NOT add foliage/water here for inserted chunks.
    // Inserted chunks don't have meshes yet. Foliage/water are added
    // in meshing_tick_system when the mesh upload completes, so they
    // appear on the same frame as the terrain.

    // Update streaming stats for UI
    ui.streaming_loaded = world.0.chunks.len() as u32;
    ui.streaming_pending = streaming.pending_gen_count() as u32;
}

// ==========================================================================
// Meshing stage
// ==========================================================================

pub fn meshing_tick_system(
    mut meshing: ResMut<MeshingCoordinator>,
    mut world: ResMut<VoxelWorld>,
    ctx: Res<RenderContext>,
    mut ui: ResMut<UiState>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut water_pass: ResMut<WaterPass>,
) {
    // Phase 1: tick meshing with &mut world - collects meshed positions.
    let meshed = meshing.tick(&mut world.0, &ctx, &mut ui);

    // Phase 2: for each newly meshed chunk, build per-chunk foliage
    // and water GPU buffers. world.0 is now borrowed immutably.
    if !meshed.is_empty() {
        for pos in &meshed {
            detail_paint.add_chunk(*pos, &world.0, &ctx.device);
            scatter.add_chunk(*pos, &world.0, &ctx.device);
            water_pass.add_chunk_water(*pos, &world.0, &ctx.device);
        }
    }
}

/// Cross-chunk seam finalization (Substep 2b): once a chunk's six face neighbors
/// are resident, derive the boundary slab demotions the isolated generation pass
/// could not see (surface steps that straddle a chunk border, including chunk-Y
/// seams). One-shot per chunk (`seam_finalized`), bounded per frame, and runs
/// before meshing so the first mesh already carries the seam slabs. Mutates
/// generated storage directly (this is generation finalization, not an actor
/// edit - see world::seam).
pub fn seam_smoothing_system(mut world: ResMut<VoxelWorld>) {
    const MAX_PER_FRAME: usize = 16;
    let sea_level = world.0.generator.sea_level();

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

        if let Some(chunk) = world.0.get_chunk_mut(pos) {
            // Never let a worldgen-derived demotion overwrite a player-placed voxel:
            // seam demotions are recomputed from current (already edit-overlaid)
            // storage, so without this guard a border edit could be reverted.
            let applied: Vec<(usize, Voxel)> = demotions
                .iter()
                .filter(|(index, _)| {
                    !chunk.data.overrides.as_ref().is_some_and(|ovr| {
                        ovr.voxel_diffs.contains_key(&voxel_core::LocalPos::from_index(*index))
                    })
                })
                .copied()
                .collect();
            if !applied.is_empty() {
                let mut storage = (*chunk.data.voxels).clone();
                for (index, voxel) in &applied {
                    storage.set_voxel(*index, *voxel);
                }
                chunk.data.voxels = std::sync::Arc::new(storage);
                chunk.mark_mesh_dirty();

                // A seam demotion only ever turns a Cube (fluid capacity 0, so
                // ocean_fill correctly placed no water) into a SlabBottom (fluid
                // capacity SLAB_MASS). ocean_fill already ran before this chunk's
                // neighbors were resident, so it never saw the new empty half -
                // without this, a seam-demoted slab stays permanently dry even
                // when it sits at/below sea level right next to water-bearing
                // cells the isolated pass got right. Submerged fast-path chunks
                // don't need this: their capacity is read live off current
                // storage, so a demotion there is already reflected for free.
                // Pond water is not covered here - the per-column pond level
                // isn't retained after generation, so a seam-demoted slab on a
                // pond shoreline (above sea level) can still come up dry.
                if !matches!(
                    chunk.data.fluids.fill_mode,
                    crate::world::layers::FluidFillMode::Submerged(_)
                ) {
                    for (index, voxel) in &applied {
                        let lp = voxel_core::LocalPos::from_index(*index);
                        let world_y =
                            pos.y * crate::world::chunk::CHUNK_SIZE as i32 + lp.y as i32;
                        if world_y > sea_level {
                            continue;
                        }
                        let capacity = crate::world::fluid_gen::fluid_capacity(*voxel);
                        if capacity > 0 {
                            chunk.data.fluids.cells.insert(
                                lp,
                                crate::world::layers::FluidCell {
                                    fluid_id: crate::world::layers::FluidId::WATER,
                                    mass: capacity,
                                    flags: crate::world::layers::FluidCell::FLAG_SETTLED,
                                },
                            );
                        }
                    }
                }

                // Persist the decision through the same diff mechanism player edits
                // use, so `seam_finalized` (below) can be persisted without the
                // demotion reverting to a sharp cube on the next load. These are
                // worldgen-finalization entries, not player edits, but the override
                // guard above treats "present in voxel_diffs" as "already decided"
                // either way, so sharing the bucket is safe.
                let ovr = chunk
                    .data
                    .overrides
                    .get_or_insert_with(crate::world::overrides::ChunkOverrides::default);
                for (index, voxel) in &applied {
                    ovr.set_voxel(*index, *voxel);
                }
                chunk.persist_dirty = true;
            }
            chunk.seam_finalized = true;
        }

        // Boundary demotions change face culling for the six neighbors: re-mesh them.
        if !demotions.is_empty() {
            for offset in [
                IVec3::new(1, 0, 0),
                IVec3::new(-1, 0, 0),
                IVec3::new(0, 1, 0),
                IVec3::new(0, -1, 0),
                IVec3::new(0, 0, 1),
                IVec3::new(0, 0, -1),
            ] {
                if let Some(n) = world.0.get_chunk_mut(pos + offset) {
                    n.mark_mesh_dirty();
                }
            }
        }
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
    post_process: Res<PostProcessPass>,
    outline: Res<OutlinePass>,
    palette_pass: Res<PalettePass>,
    upscale: Res<UpscalePass>,
    render_targets: Res<RenderTargets>,
    loaded_palette: Res<LoadedPalette>,
) {
    crate::rendering::uniform_writer::write_all_uniforms(
        &ctx,
        &frame,
        &ui.params,
        &sim.cloud_shadow,
        &global_buf.0,
        &shadow_buf.0,
        &post_process,
        &outline,
        &palette_pass,
        &upscale,
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
    let surface_result = ecs.resource::<SurfaceState>().surface.get_current_texture();
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
    {
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
            water_pass: &water_pass,
            water_pipeline: &pipeline_registry.water_pipeline,
            debug_line_pass: &debug_line_pass,
            show_debug_lines: ui.params.debug.show_chunk_boundaries,
            hide_water: ui.params.debug.hide_water || room_enclosed,
            hide_foliage: ui.params.debug.hide_foliage || room_enclosed,
            player_pass: &player_pass,
        };

        let mut graph = RenderGraph::new();
        graph.add_pass(&shadow_node);
        graph.add_pass(&scene_node);
        graph.add_pass(&*outline_pass);
        graph.add_pass(&*post_process);
        graph.add_pass(&*palette_pass);
        graph.add_pass(&*upscale_pass);

        #[cfg(debug_assertions)]
        graph
            .validate(&[
                ResourceId::SHADOW_DEPTH,
                ResourceId::SCENE,
                ResourceId::NORMAL,
                ResourceId::DEPTH,
                ResourceId::PROCESSED,
                ResourceId::SURFACE,
            ])
            .unwrap();

        let mut resources = ResourceMap::new();
        resources.insert(ResourceId::SCENE, &render_targets.scene_view);
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
            // Auto-regen: hand a dirty editor edit to the regen path. Coalesced by
            // WorldRegenCoordinator - only the latest pending graph is applied, and
            // only while no regen is in flight. No-op while the editor is hidden
            // (its `show` isn't called, so it never goes dirty).
            if let Some((slot, graph)) = egui.graph_editor.consume_dirty() {
                // The selector reports which hierarchy graph was edited.
                ui_state.pending_graph = Some((slot, graph));
            }
        });
    });

    // --- Submit + present ---
    ctx.queue.submit(std::iter::once(encoder.finish()));
    surface_frame.present();
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
) {
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
) {
    persistence.tick_autosave(frame.dt, &mut world.0);
}
