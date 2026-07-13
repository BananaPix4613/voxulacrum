//! Player world interaction: cursor-ray picking and (Substep 8c) scatter edits.
//!
//! Each frame the picking system casts a ray from the cursor through the
//! orthographic camera into the world via a voxel DDA and records the first
//! solid voxel - the *anchor* the player interacts with (design doc §8). The
//! anchor is highlighted by reusing the debug-line pipeline.

use bevy_ecs::prelude::*;
use glam::{IVec3, Mat4, Vec3};
use voxel_core::LocalPos;

use crate::ecs::resources::VoxelWorld;
use crate::input::{PointerState, RawInputBuffer};
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::surface_state::SurfaceState;
use crate::simulation::manager::SimulationManager;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::mutation::{MutationCommand, WorldMutation};
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
    // No world pick while the cursor is over an egui panel.
    pick.anchor = if buffer.egui_wants_pointer {
        None
    } else {
        compute_pick(&pointer, &sim, &surface, &world.0)
    };
    debug_lines.set_highlight(&ctx, pick.anchor);
}

/// Unproject the cursor to a world ray and DDA-march it to the first solid voxel.
fn compute_pick(
    pointer: &PointerState,
    sim: &SimulationManager,
    surface: &SurfaceState,
    world: &World,
) -> Option<IVec3> {
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

    raycast_voxel(world, near, dir)
}

/// Amanatides-Woo voxel traversal in voxel space (1 unit = 1 voxel).
fn raycast_voxel(world: &World, origin_world: Vec3, dir: Vec3) -> Option<IVec3> {
    let p = origin_world / VOXEL_SCALE;

    let mut v = IVec3::new(p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
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
            return Some(v);
        }
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

/// Apply scatter edits on left/right click at the picked anchor voxel:
/// left removes the props there, right places one. Rebuilds the chunk's scatter
/// instance buffer so the change shows immediately.
pub fn scatter_edit_system(
    pointer: Res<PointerState>,
    buffer: Res<RawInputBuffer>,
    pick: Res<PickState>,
    ctx: Res<RenderContext>,
    mut world: ResMut<VoxelWorld>,
    mut scatter_pass: ResMut<ScatterPass>,
) {
    if buffer.egui_wants_pointer {
        return;
    }
    let left = pointer.left.just_pressed;
    let right = pointer.right.just_pressed;
    if !left && !right {
        return;
    }
    let Some(anchor_voxel) = pick.anchor else {
        return;
    };

    let dim = CHUNK_SIZE as i32;
    let chunk_pos = IVec3::new(
        anchor_voxel.x.div_euclid(dim),
        anchor_voxel.y.div_euclid(dim),
        anchor_voxel.z.div_euclid(dim),
    );
    let anchor = LocalPos::new_unchecked(
        anchor_voxel.x.rem_euclid(dim) as u8,
        anchor_voxel.y.rem_euclid(dim) as u8,
        anchor_voxel.z.rem_euclid(dim) as u8,
    );

    let mut rebuilt = false;
    if left {
        let out = world
            .0
            .execute(MutationCommand::authoring(WorldMutation::RemoveScatter {
                chunk: chunk_pos,
                anchor,
            }))
            .expect("authoring scatter edit in authoring mode");
        rebuilt |= !out.scatter_rebuild.is_empty();
    }
    if right {
        let out = world
            .0
            .execute(MutationCommand::authoring(WorldMutation::PlaceScatter {
                chunk: chunk_pos,
                anchor,
                world_voxel: anchor_voxel,
            }))
            .expect("authoring scatter edit in authoring mode");
        rebuilt |= !out.scatter_rebuild.is_empty();
    }

    if rebuilt {
        scatter_pass.add_chunk(chunk_pos, &world.0, &ctx.device);
    }
}
