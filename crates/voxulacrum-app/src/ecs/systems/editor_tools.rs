//! Authoring tools: the column inspector, blueprint capture and stamping, the
//! tree skeleton preview, and the biome map.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::ecs::resources::VoxelWorld;
use crate::input::{InputState, PointerState};
use crate::interaction::PickState;
use crate::jobs::JobSystem;
use crate::materials::MaterialRegistryRes;
use crate::rendering::debug_lines::{voxel_box_lines, DebugLinePass};
use crate::rendering::render_context::RenderContext;
use crate::rendering::skeleton_preview_pass::SkeletonPreviewPass;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::mutation::{MutationCommand, WorldMutation};

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
            match voxel_core::Blueprint::load(&dir, &name)
                .and_then(|bp| bp.resolve(&registry.0, crate::paths::namespace()))
            {
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
                        .and_then(|bp| bp.resolve(&registry.0, crate::paths::namespace()))
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
    mut preview: ResMut<SkeletonPreviewPass>,
) {
    use crate::input::GameAction;
    use crate::rendering::debug_lines::DebugLineVertex;
    use crate::rendering::pipelines::{CapsuleInstanceGpu, LobeInstanceGpu};
    use crate::ui::blueprint_panel::ArmedTool;
    use crate::ui::skeleton_panel::PreviewRender;
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

    if std::mem::take(&mut state.list_dirty) {
        state.species_list =
            nodegraph_hotreload::list_species(&crate::world::blueprint::species_dir());
        state.selected_species = state.selected_species.min(state.species_list.len().saturating_sub(1));
    }

    if std::mem::take(&mut state.load_requested) {
        // Disk is authoritative both ways: what loads here is exactly what a
        // `PlaceTree` node resolves, so tuning starts from the shipped tree
        // rather than from defaults that only resemble it.
        if let Some(name) = state.species_list.get(state.selected_species).cloned() {
            match nodegraph_hotreload::load_species(
                &crate::world::blueprint::species_dir(),
                &name,
            ) {
                Ok(species) => {
                    state.species = species;
                    state.name = name.clone();
                    state.status = Some(format!("loaded {name}.species.json"));
                    state.error = None;
                }
                Err(e) => state.error = Some(e.to_string()),
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
                // A species saved here must appear in this panel's own picker.
                state.list_dirty = true;
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
            state.render,
        )),
        _ => None,
    };
    if key == state.built {
        return;
    }
    state.built = key;

    let Some((origin, seed, age, species, show_radii, show_wood, show_capsules, render)) = key
    else {
        debug_lines.set_overlay(&ctx, None);
        preview.clear();
        state.nodes = 0;
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

    // Occupancy is derived either way: it feeds the readout, and its outline is
    // the one overlay that complements a solid render rather than restating it.
    let root_voxels = origin.as_vec3() + Vec3::new(0.5, 1.0, 0.5);
    let mut wood: std::collections::HashSet<glam::IVec3> = std::collections::HashSet::new();
    skeleton.voxelize(root_voxels, |c| {
        wood.insert(c);
    });

    let mut lines: Vec<DebugLineVertex> = Vec::new();
    if show_wood {
        for cell in &wood {
            lines.extend(voxel_box_lines(*cell, SKELETON_WOOD_COLOR));
        }
    }

    if render == PreviewRender::Solid {
        // The same two instance kinds `stamp_features` emits, built the same
        // way: cones parent-to-child, then one sphere per node as a degenerate
        // capsule. Reproducing the emission rather than the geometry is what
        // makes this a preview of generation and not of a second renderer.
        let mut capsules: Vec<CapsuleInstanceGpu> = Vec::new();
        for seg in skeleton.segments() {
            capsules.push(CapsuleInstanceGpu {
                a: (base + seg.a * VOXEL_SCALE).to_array(),
                ra: seg.ra * VOXEL_SCALE,
                b: (base + seg.b * VOXEL_SCALE).to_array(),
                rb: seg.rb * VOXEL_SCALE,
            });
        }
        for node in &skeleton.nodes {
            let p = (base + node.pos * VOXEL_SCALE).to_array();
            let r = node.radius * VOXEL_SCALE;
            capsules.push(CapsuleInstanceGpu { a: p, ra: r, b: p, rb: r });
        }
        let lobe_instances: Vec<LobeInstanceGpu> = skeleton
            .lobes
            .iter()
            .map(|l| LobeInstanceGpu {
                center: (base + l.center * VOXEL_SCALE).to_array(),
                radii: (l.radii() * VOXEL_SCALE).to_array(),
            })
            .collect();
        preview.set(&ctx, &capsules, &lobe_instances);

        debug_lines.set_overlay(&ctx, Some(&lines));
        state.wood = wood.len();
        state.nodes = skeleton.nodes.len();
        state.chains = skeleton.chains;
        state.lobes = skeleton.lobes.len();
        state.reach = skeleton.reach;
        state.height = skeleton.height;
        return;
    }

    preview.clear();
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
