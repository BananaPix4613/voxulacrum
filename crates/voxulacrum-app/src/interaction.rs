//! Player world interaction: cursor-ray picking and (Substep 8c) scatter edits.
//!
//! Each frame the picking system casts a ray from the cursor through the
//! orthographic camera into the world via a voxel DDA and records the first
//! solid voxel - the *anchor* the player interacts with (design doc §8). The
//! anchor is highlighted by reusing the debug-line pipeline.

use bevy_ecs::prelude::*;
use glam::{IVec3, Mat4, Vec3};
use voxel_core::{MaterialId, Voxel};

use crate::ecs::resources::VoxelWorld;
use crate::input::{GameAction, InputState, PointerState, RawInputBuffer};
use crate::player::systems::Player;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::surface_state::SurfaceState;
use crate::simulation::manager::SimulationManager;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::mutation::{EngineMode, MutationCommand, WorldMutation};
use crate::world::World;

/// The voxel anchor currently under the cursor, if the ray hit terrain.
#[derive(Resource, Default)]
pub struct PickState {
    /// World voxel coordinates of the picked surface (anchor) voxel.
    pub anchor: Option<IVec3>,
}

/// Max voxels the pick ray traverses before giving up.
const MAX_PICK_STEPS: i32 = 4096;

/// Cast the cursor ray each frame and update [`PickState`] + the highlight box.
pub fn picking_system(
    pointer: Res<PointerState>,
    buffer: Res<RawInputBuffer>,
    sim: Res<SimulationManager>,
    surface: Res<SurfaceState>,
    world: Res<VoxelWorld>,
    ctx: Res<RenderContext>,
    mut pick: ResMut<PickState>,
    mut debug_lines: ResMut<DebugLinePass>,
) {
    // Authoring's dev-tool pick only - Play mode has its own reach-aware
    // targeting/highlight (player_targeting_system). Without this gate, both
    // systems drive the same shared highlight buffer and whichever runs second
    // wins - which is exactly why the highlight was stuck on Authoring's fixed
    // yellow regardless of what Play's reach check decided.
    if world.0.mode != EngineMode::Authoring {
        return;
    }
    // No world pick while the cursor is over an egui panel.
    pick.anchor = if buffer.egui_wants_pointer {
        None
    } else {
        compute_pick(&pointer, &sim, &surface, &world.0)
    };
    debug_lines.set_highlight(&ctx, pick.anchor, HIGHLIGHT_ACTIVE);
}

/// Unproject the cursor to a world-space ray (origin, direction), or `None` if
/// the cursor is outside the window or the surface has zero extent. Shared by
/// Authoring's pick and Play mode's reach targeting.
fn cursor_ray(
    pointer: &PointerState,
    sim: &SimulationManager,
    surface: &SurfaceState,
) -> Option<(Vec3, Vec3)> {
    let (cx, cy) = pointer.cursor?;
    let w = surface.surface_config.width as f32;
    let h = surface.surface_config.height as f32;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }

    // Window pixels -> NDC (y flipped).
    let ndc_x = cx / w * 2.0 - 1.0;
    let ndc_y = 1.0 - cy / h * 2.0;

    let vp = Mat4::from_cols_array_2d(&sim.camera.view_projection());
    let inv = vp.inverse();
    let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
    let dir = (far - near).normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    Some((near, dir))
}

/// Unproject the cursor and DDA-march it to the first solid voxel (Authoring's
/// unbounded dev-tool pick).
fn compute_pick(
    pointer: &PointerState,
    sim: &SimulationManager,
    surface: &SurfaceState,
    world: &World,
) -> Option<IVec3> {
    let (near, dir) = cursor_ray(pointer, sim, surface)?;
    raycast_voxel(world, near, dir)
}

/// Amanatides-Woo voxel traversal in voxel space (1 unit = 1 voxel). Authoring's
/// unbounded pick; discards the face-adjacent cell `raycast_voxel_faced` tracks.
fn raycast_voxel(world: &World, origin_world: Vec3, dir: Vec3) -> Option<IVec3> {
    raycast_voxel_faced(world, origin_world, dir, f32::INFINITY).map(|(hit, _)| hit)
}

/// Amanatides-Woo voxel traversal in voxel space (1 unit = 1 voxel). Returns the
/// first solid voxel hit and the empty cell immediately before it along the ray
/// (the face-adjacent placement cell), or `None` if nothing solid is hit within
/// `max_dist_voxels` (pass `f32::INFINITY` for an unbounded march, as Authoring's
/// pick does; Play-mode reach targeting instead post-filters by distance, per
/// Substep 11's plan, so this is also called with `f32::INFINITY` today).
fn raycast_voxel_faced(
    world: &World,
    origin_world: Vec3,
    dir: Vec3,
    max_dist_voxels: f32,
) -> Option<(IVec3, IVec3)> {
    let p = origin_world / VOXEL_SCALE;

    let mut v = IVec3::new(p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
    let mut prev = v;
    let step = IVec3::new(
        if dir.x >= 0.0 { 1 } else { -1 },
        if dir.y >= 0.0 { 1 } else { -1 },
        if dir.z >= 0.0 { 1 } else { -1 },
    );

    // Ray-t to the next voxel boundary on each axis.
    let next_b = |vi: i32, s: i32| if s > 0 { (vi + 1) as f32 } else { vi as f32 };
    let mut t_max = Vec3::new(
        if dir.x != 0.0 { (next_b(v.x, step.x) - p.x) / dir.x } else { f32::INFINITY },
        if dir.y != 0.0 { (next_b(v.y, step.y) - p.y) / dir.y } else { f32::INFINITY },
        if dir.z != 0.0 { (next_b(v.z, step.z) - p.z) / dir.z } else { f32::INFINITY },
    );
    let t_delta = Vec3::new(
        if dir.x != 0.0 { (1.0 / dir.x).abs() } else { f32::INFINITY },
        if dir.y != 0.0 { (1.0 / dir.y).abs() } else { f32::INFINITY },
        if dir.z != 0.0 { (1.0 / dir.z).abs() } else { f32::INFINITY },
    );

    for _ in 0..MAX_PICK_STEPS {
        if is_solid_world(world, v) {
            return Some((v, prev));
        }
        if t_max.x.min(t_max.y).min(t_max.z) > max_dist_voxels {
            return None;
        }
        prev = v;
        if t_max.x < t_max.y {
            if t_max.x < t_max.z {
                v.x += step.x;
                t_max.x += t_delta.x;
            } else {
                v.z += step.z;
                t_max.z += t_delta.z;
            }
        } else if t_max.y < t_max.z {
            v.y += step.y;
            t_max.y += t_delta.y;
        } else {
            v.z += step.z;
            t_max.z += t_delta.z;
        }
    }
    None
}

/// World-space voxel solidity (mirrors `cap_pass::is_solid_at`).
fn is_solid_world(world: &World, v: IVec3) -> bool {
    let dim = CHUNK_SIZE as i32;
    let chunk_pos = IVec3::new(v.x.div_euclid(dim), v.y.div_euclid(dim), v.z.div_euclid(dim));
    match world.get_chunk(chunk_pos) {
        Some(chunk) => chunk.is_solid(
            v.x.rem_euclid(dim) as usize,
            v.y.rem_euclid(dim) as usize,
            v.z.rem_euclid(dim) as usize,
        ),
        None => false,
    }
}

/// Max distance (voxels) the player can target for break/place (Substep 11).
const PLAYER_REACH: f32 = 4.5;

/// Vertical offset from the player's feet to the eye, for the targeting ray
/// origin and reach distance - matches the head-cell offset room detection uses.
const PLAYER_EYE_HEIGHT: f32 = 1.6;

/// Highlight color when the hovered voxel is within reach (Substep 12) - also
/// Authoring's pick-highlight color, unchanged from before this substep.
const HIGHLIGHT_ACTIVE: [f32; 3] = [1.0, 1.0, 0.2];
/// Highlight color when the hovered voxel is outside reach - greyed out rather
/// than hidden, so the player can still see what they're pointing at.
const HIGHLIGHT_OUT_OF_REACH: [f32; 3] = [0.5, 0.5, 0.5];

/// The player's current break/place target, reach-limited, Play-mode only
/// (Substep 11/12). `anchor`/`place` are `None` unless the hovered voxel is
/// within reach (they gate `player_action_system`). `hover` is the raw cursor
/// hit regardless of reach, and `in_reach` whether it's actually reachable -
/// both drive the highlight color and HUD feedback even when out of range.
#[derive(Resource, Default)]
pub struct TargetState {
    pub anchor: Option<IVec3>,
    pub place: Option<IVec3>,
    pub hover: Option<IVec3>,
    pub in_reach: bool,
}

/// The player's currently selected build material (Substep 12), cycled by
/// `CycleTool`. Index is 1-based onto `MaterialParams.entries` (index 0, Air,
/// is skipped - it isn't a placeable tool).
#[derive(Resource)]
pub struct PlayerToolState {
    pub material_index: usize,
}

impl Default for PlayerToolState {
    fn default() -> Self {
        Self { material_index: 1 } // Limestone, matching Substep 11's placeholder
    }
}

/// Cursor-ray reach targeting for Play mode: the same cursor ray as Authoring's
/// pick, always shown (even out of reach, greyed - Substep 12), but `anchor`/
/// `place` only populate within `PLAYER_REACH` of the player's eye.
pub fn player_targeting_system(
    pointer: Res<PointerState>,
    buffer: Res<RawInputBuffer>,
    sim: Res<SimulationManager>,
    surface: Res<SurfaceState>,
    world: Res<VoxelWorld>,
    ctx: Res<RenderContext>,
    players: Query<&Player>,
    mut target: ResMut<TargetState>,
    mut debug_lines: ResMut<DebugLinePass>,
) {
    let miss = world.0.mode != EngineMode::Play || buffer.egui_wants_pointer;
    let player = players.iter().next();

    let hit = if miss {
        None
    } else if let Some(player) = player {
        let eye = (player.curr.position + Vec3::new(0.0, PLAYER_EYE_HEIGHT, 0.0)) / VOXEL_SCALE;
        cursor_ray(&pointer, &sim, &surface)
            .and_then(|(origin, dir)| raycast_voxel_faced(&world.0, origin, dir, f32::INFINITY))
            .map(|(hit, prev)| (hit, prev, hit.as_vec3().distance(eye) <= PLAYER_REACH))
    } else {
        None
    };

    target.hover = hit.map(|(hit, _, _)| hit);
    target.in_reach = hit.is_some_and(|(_, _, in_reach)| in_reach);
    (target.anchor, target.place) = match hit {
        Some((hit, prev, true)) => (Some(hit), Some(prev)),
        _ => (None, None),
    };

    let color = if target.in_reach { HIGHLIGHT_ACTIVE } else { HIGHLIGHT_OUT_OF_REACH };
    debug_lines.set_highlight(&ctx, target.hover, color);
}

/// Cycle the active build material on `CycleTool(delta)`, wrapping through every
/// non-Air registered material. Play-mode only.
pub fn player_tool_system(
    input: Res<InputState>,
    ui: Res<crate::ui::panels::UiState>,
    world: Res<VoxelWorld>,
    mut tool: ResMut<PlayerToolState>,
) {
    if world.0.mode != EngineMode::Play {
        return;
    }
    let count = ui.params.materials.entries.len();
    if count <= 1 {
        return; // only Air registered - nothing to cycle
    }
    let delta = if input.just_pressed(GameAction::CycleTool(1)) {
        1i32
    } else if input.just_pressed(GameAction::CycleTool(-1)) {
        -1i32
    } else {
        return;
    };
    // Wrap through [1, count) - index 0 (Air) is never a selectable tool.
    let span = (count - 1) as i32;
    let zero_based = (tool.material_index as i32 - 1 + delta).rem_euclid(span);
    tool.material_index = (zero_based + 1) as usize;
}

/// Break/place on Primary/Secondary action at the current reach target
/// (Substep 11). Mirrors `scatter_edit_system`'s mutation shape but issues real
/// voxel edits through `MutationCommand::play`.
pub fn player_action_system(
    input: Res<InputState>,
    target: Res<TargetState>,
    tool: Res<PlayerToolState>,
    mut world: ResMut<VoxelWorld>,
) {
    if world.0.mode != EngineMode::Play {
        return;
    }

    if input.just_pressed(GameAction::PrimaryAction) {
        if let Some(anchor) = target.anchor {
            edit_voxel(&mut world.0, anchor, Voxel::EMPTY);
        }
    }
    if input.just_pressed(GameAction::SecondaryAction) {
        if let (Some(place), Some(anchor)) = (target.place, target.anchor) {
            if place != anchor {
                let material = MaterialId(tool.material_index as u16);
                edit_voxel(&mut world.0, place, Voxel::cube(material));
            }
        }
    }
}

/// Convert a world voxel coordinate to (chunk, local index) and issue the edit -
/// the same conversion `scatter_edit_system` uses. No-op if the chunk isn't loaded
/// (a place target can, in rare edge cases, sit just across a streaming boundary).
/// `meshing_tick_system` picks up the resulting mesh-dirty flag every frame.
fn edit_voxel(world: &mut World, world_voxel: IVec3, voxel: Voxel) {
    let (chunk, local) = crate::world::split_world_voxel(world_voxel);
    if world.get_chunk(chunk).is_none() {
        return;
    }
    world
        .execute(MutationCommand::play(WorldMutation::EditVoxel {
            chunk,
            index: local.to_index() as u16,
            voxel,
        }))
        .expect("play-time voxel edit in Play mode");
}
