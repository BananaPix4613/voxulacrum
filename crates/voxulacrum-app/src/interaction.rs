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
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, VOXEL_SCALE};
use crate::world::layers::{PrefabId, ScatterFlags, ScatterInstance, StableInstanceId};
use crate::world::overrides::ChunkOverrides;
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

    let changed = {
        let Some(chunk) = world.0.get_chunk_mut(chunk_pos) else {
            return;
        };
        let mut c = false;
        if left {
            c |= remove_scatter_at(chunk, anchor);
        }
        if right {
            c |= place_scatter_at(chunk, anchor, anchor_voxel);
        }
        if c {
            chunk.persist_dirty = true; // override changed -> needs save
        }
        c
    };

    if changed {
        scatter_pass.add_chunk(chunk_pos, &world.0, &ctx.device);
    }
}

/// Remove every effective scatter instance anchored at `anchor`: generated ones
/// are recorded in `scatter_removed`, player-added ones are dropped.
fn remove_scatter_at(chunk: &mut LoadedChunk, anchor: LocalPos) -> bool {
    let generated_ids: Vec<StableInstanceId> = chunk
        .data
        .scatter_instances
        .by_type
        .values()
        .flatten()
        .filter(|si| si.anchor == anchor)
        .map(|si| si.stable_id)
        .collect();

    let overrides = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
    let mut changed = false;
    for id in generated_ids {
        if overrides.scatter_removed.insert(id) {
            changed = true;
        }
    }
    let before = overrides.scatter_added.len();
    overrides.scatter_added.retain(|si| si.anchor != anchor);
    if overrides.scatter_added.len() != before {
        changed = true;
    }
    changed
}

/// Place one player scatter instance at `anchor`, jittered within the cell.
fn place_scatter_at(chunk: &mut LoadedChunk, anchor: LocalPos, world_voxel: IVec3) -> bool {
    let seq = effective_count_at(chunk, anchor);
    let h = player_stable_id(world_voxel, seq);
    let overrides = chunk.data.overrides.get_or_insert_with(ChunkOverrides::default);
    overrides.scatter_added.push(ScatterInstance {
        anchor,
        sub_offset: [(h & 0xFF) as u8 as i8, 0, ((h >> 8) & 0xFF) as u8 as i8],
        rotation_y: ((h >> 16) & 0xFF) as u8,
        scale_variant: 0,
        prefab_id: PrefabId(0),
        flags: ScatterFlags(ScatterFlags::PLAYER_PLACED),
        stable_id: StableInstanceId(h),
    });
    true
}

/// Count effective instances at `anchor` (generated-minus-removed + added).
fn effective_count_at(chunk: &LoadedChunk, anchor: LocalPos) -> u32 {
    let ovr = chunk.data.overrides.as_ref();
    let gen = chunk
        .data
        .scatter_instances
        .by_type
        .values()
        .flatten()
        .filter(|si| {
            si.anchor == anchor
                && !ovr.map_or(false, |o| o.scatter_removed.contains(&si.stable_id))
        })
        .count();
    let added = ovr.map_or(0, |o| {
        o.scatter_added.iter().filter(|si| si.anchor == anchor).count()
    });
    (gen + added) as u32
}

/// Stable id for a player-placed instance. Salted so it can't collide with a
/// generated id (which derives from the world seed), and varied by `seq` so
/// repeated placements on one anchor get distinct ids.
fn player_stable_id(v: IVec3, seq: u32) -> u64 {
    let mut h: u64 = 0xA11C_E1A5_0FF1_CE11; // player-placed salt
    h = mix64(h ^ (v.x as i64 as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    h = mix64(h ^ (v.y as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h = mix64(h ^ (v.z as i64 as u64).wrapping_mul(0xABC9_8388_FB8F_AC03));
    h = mix64(h ^ (seq as u64).wrapping_mul(0xC4CE_B9FE_1A85_EC53));
    h
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^= x >> 33;
    x
}
