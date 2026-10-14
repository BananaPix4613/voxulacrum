//! Player presentation: the avatar mesh and its drop shadow.

use bevy_ecs::prelude::*;
use crate::ecs::resources::VoxelWorld;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_targets::RenderTargets;
use crate::simulation::manager::SimulationManager;
use crate::world::mutation::EngineMode;

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
