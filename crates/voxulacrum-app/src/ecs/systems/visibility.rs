//! Underground visibility: the flood from the player's head cell, and the
//! mask texture the terrain shader reads.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::ecs::resources::{VisibilityMask, VoxelWorld};
use crate::player::sim::WorldView;
use crate::rendering::render_context::RenderContext;
use crate::simulation::manager::FrameState;
use crate::world::mutation::EngineMode;

/// Visibility flood each time the player crosses into a new head cell. Produces
/// undergroundness `u` and the `visible` air set the mask marks; the shader's
/// air-side face rule does the rest (note v2 §3 — no cut set, view-independent, so
/// no rotation recompute). A cheap overhead pre-filter skips the flood outdoors.
pub fn room_detection_system(
    world: Res<VoxelWorld>,
    players: Query<&crate::player::systems::Player>,
    play: Option<Res<crate::play::PlaySettings>>,
    mut room: ResMut<crate::world::room::RoomState>,
) {
    use crate::player::sim::WorldView;
    use crate::world::room::{flood_visibility, overhead_coverage};

    const BUDGET: u32 = 22;             // through-air flood budget (note §4.1)
    const OPEN_SKY_COVERAGE: f32 = 0.3; // below this overhead, skip the flood
    const U_THRESHOLD: f32 = 0.6;       // u above this = enclosed (mask on)
    const CLARITY_RADIUS: u32 = 24;     // MUST match CLARITY_RADIUS in terrain.wgsl

    let player = players.iter().next();
    if world.0.mode != EngineMode::Play || player.is_none() || play.is_none() {
        if room.enclosed {
            log::info!("player left enclosed space (play/mode ended)");
        }
        room.enclosed = false;
        room.undergroundness = 0.0;
        room.last_origin = None;
        return;
    }

    let feet = player.unwrap().curr.position;
    let eye_height = play.unwrap().player.eye_height;
    let head = glam::IVec3::new(
        feet.x.floor() as i32,
        (feet.y + eye_height).floor() as i32,
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
