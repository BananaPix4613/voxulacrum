//! ECS glue for the player sim: components, and the systems that wrap the pure
//! `player_step`, which runs as a tick phase. No renderer type touches the player
//! entity (the Phase-10 boundary); systems read the camera only to build the input
//! frame and the spawn position.

use bevy_ecs::prelude::*;
use glam::{IVec3, Vec2, Vec3};

use crate::ecs::resources::VoxelWorld;
use crate::input::{GameAction, InputState};
use crate::player::sim::{player_step, InputFrame, PlayerState, WorldView};
use crate::simulation::manager::SimulationManager;
use crate::world::chunk::CHUNK_SIZE;
use crate::world::mutation::EngineMode;

/// The player entity's simulation state, double-buffered for camera interpolation
/// (Substep 6): `curr` is the latest tick, `prev` the one before.
#[derive(Component)]
pub struct Player {
    pub curr: PlayerState,
    pub prev: PlayerState,
}

/// The input frame the next sim tick will consume, filled each frame by
/// `player_input_gather_system` from the semantic action layer.
#[derive(Component, Default)]
pub struct PlayerInputBuffer {
    pub pending: InputFrame,
}

/// Explicit `Authoring <-> Play` toggle (F5). Entering Play spawns the player at the
/// camera target and flips the runtime mode; leaving Play despawns it. No silent
/// switching - the mode gate on the mutation API becomes load-bearing from here.
pub fn player_mode_toggle_system(
    input: Res<InputState>,
    sim: Res<SimulationManager>,
    mut world: ResMut<VoxelWorld>,
    players: Query<Entity, With<Player>>,
    play: Option<Res<crate::play::PlaySettings>>,
    mut commands: Commands,
) {
    if !input.just_pressed(GameAction::TogglePlayMode) {
        return;
    }
    match world.0.mode {
        EngineMode::Authoring => {
            let Some(play) = play else {
                log::info!("This package has no {}, so it has no Play mode", crate::play::PLAY_FILE);
                return;
            };
            let cam = sim.camera.smooth_target;
            let position = match play.spawn {
                crate::play::SpawnRule::SurfaceUnderCamera => {
                    surface_spawn(&world.0, cam.x, cam.z).unwrap_or(cam)
                }
                // `SpawnRule` is non-exhaustive: a rule this build doesn't know
                // spawns at the camera target itself.
                _ => cam,
            };
            let state = PlayerState { position, ..Default::default() };
            commands.spawn((Player { curr: state, prev: state }, PlayerInputBuffer::default()));
            world.0.mode = EngineMode::Play;
            log::info!("Entered Play mode; player spawned at {position:?}");
        }
        EngineMode::Play => {
            for e in &players {
                commands.entity(e).despawn();
            }
            world.0.mode = EngineMode::Authoring;
            log::info!("Returned to Authoring mode; player despawned");
        }
    }
}

/// Build the next tick's input frame from the semantic action layer. The
/// screen-relative move vector is rotated into world space here (camera basis on
/// the ground plane), so the pure `player_step` stays camera-agnostic.
pub fn player_input_gather_system(
    input: Res<InputState>,
    sim: Res<SimulationManager>,
    mut q: Query<&mut PlayerInputBuffer>,
) {
    let Some(mut buffer) = q.iter_mut().next() else {
        return;
    };
    let mv = input.move_vector(); // screen-relative: x = right, y = forward
    let rot = sim.camera.rotation;
    let forward = Vec2::new(-rot.cos(), -rot.sin()); // world XZ of "screen up"
    let right = Vec2::new(rot.sin(), -rot.cos());
    let move_world = right * mv.x + forward * mv.y;
    buffer.pending = InputFrame { move_world, jump: false };
}

/// Tick phase `Players`: step the player once with its pending input, keeping
/// the previous state as the source the frame interpolates from.
pub fn player_tick_system(
    world: Res<VoxelWorld>,
    mut q: Query<(&mut Player, &PlayerInputBuffer)>,
    play: Option<Res<crate::play::PlaySettings>>,
) {
    let Some(play) = play else {
        return; // no play settings, so no player was ever spawned
    };
    let Some((mut player, buffer)) = q.iter_mut().next() else {
        return;
    };
    player.prev = player.curr;
    player.curr = player_step(&player.curr, &buffer.pending, &world.0, &play.player);
}

/// Find a grounded spawn: the top surface of the highest solid voxel in the
/// column under `(x, z)`, or `None` if the column is empty within the world.
fn surface_spawn(world: &crate::world::World, x: f32, z: f32) -> Option<Vec3> {
    let vx = x.floor() as i32;
    let vz = z.floor() as i32;
    let top = world.max_chunk_y * CHUNK_SIZE as i32;
    let bottom = world.min_chunk_y * CHUNK_SIZE as i32;
    for y in (bottom..top).rev() {
        if let Some((_, iy1)) = world.solid_interval(IVec3::new(vx, y, vz)) {
            return Some(Vec3::new(x, y as f32 + iy1, z));
        }
    }
    None
}
