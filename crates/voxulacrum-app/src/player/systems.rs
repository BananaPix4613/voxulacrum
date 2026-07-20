//! ECS glue for the player sim: components, the fixed clock, the `WorldView` impl,
//! and the systems that wrap the pure `player_step`. No renderer type touches the
//! player entity (the Phase-10 boundary); systems read the camera only to build the
//! input frame and the spawn position.

use bevy_ecs::prelude::*;
use glam::{IVec3, Vec2, Vec3};
use voxel_core::ShapeId;

use crate::ecs::resources::VoxelWorld;
use crate::input::{GameAction, InputState};
use crate::player::sim::{player_step, InputFrame, PlayerState, WorldView};
use crate::simulation::manager::{FrameState, SimulationManager};
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

/// Fixed-timestep accumulator for the player sim (30 Hz), same pattern as
/// `FluidClock`. `alpha` is the fractional progress toward the next tick, for
/// Substep 6's camera interpolation.
#[derive(Resource)]
pub struct PlayerClock {
    accumulator: f32,
    tick_dt: f32,
    max_ticks_per_frame: u32,
}

impl PlayerClock {
    /// 30 Hz, at most 5 catch-up ticks per frame.
    pub fn new() -> Self {
        Self { accumulator: 0.0, tick_dt: crate::player::sim::TICK_DT, max_ticks_per_frame: 5 }
    }

    /// Accumulate a frame's `dt` and return how many fixed ticks to run.
    pub fn ticks_for(&mut self, dt: f32) -> u32 {
        self.accumulator += dt.min(0.25);
        let mut n = 0;
        while self.accumulator >= self.tick_dt && n < self.max_ticks_per_frame {
            self.accumulator -= self.tick_dt;
            n += 1;
        }
        if self.accumulator > self.tick_dt {
            self.accumulator = 0.0; // drop backlog beyond the cap
        }
        n
    }

    /// Fractional progress into the current tick (0..=1), for render interpolation.
    #[allow(dead_code)] // consumed by camera interpolation (Substep 6)
    pub fn alpha(&self) -> f32 {
        (self.accumulator / self.tick_dt).clamp(0.0, 1.0)
    }
}

/// Read-only world access for the pure sim: a voxel's solid vertical interval,
/// honoring slabs, by world coordinate.
impl WorldView for crate::world::World {
    fn solid_interval(&self, voxel: IVec3) -> Option<(f32, f32)> {
        let dim = CHUNK_SIZE as i32;
        let chunk = IVec3::new(
            voxel.x.div_euclid(dim),
            voxel.y.div_euclid(dim),
            voxel.z.div_euclid(dim),
        );
        let v = self.get_chunk(chunk)?.voxel(
            voxel.x.rem_euclid(dim) as usize,
            voxel.y.rem_euclid(dim) as usize,
            voxel.z.rem_euclid(dim) as usize,
        );
        match v.shape {
            ShapeId::Cube => Some((0.0, 1.0)),
            ShapeId::SlabBottom => Some((0.0, 0.5)),
            ShapeId::SlabTop => Some((0.5, 1.0)),
            ShapeId::Empty => None,
        }
    }
}

/// Explicit `Authoring <-> Play` toggle (F5). Entering Play spawns the player at the
/// camera target and flips the runtime mode; leaving Play despawns it. No silent
/// switching - the mode gate on the mutation API becomes load-bearing from here.
pub fn player_mode_toggle_system(
    input: Res<InputState>,
    sim: Res<SimulationManager>,
    mut world: ResMut<VoxelWorld>,
    players: Query<Entity, With<Player>>,
    mut commands: Commands,
) {
    if !input.just_pressed(GameAction::TogglePlayMode) {
        return;
    }
    match world.0.mode {
        EngineMode::Authoring => {
            let cam = sim.camera.smooth_target;
            let position = surface_spawn(&world.0, cam.x, cam.z).unwrap_or(cam);
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

/// Drive the player sim at the fixed tick rate, wrapping the pure `player_step`.
/// Advances `prev`/`curr` per tick (interpolation source for Substep 6).
pub fn player_sim_system(
    frame: Res<FrameState>,
    world: Res<VoxelWorld>,
    mut clock: ResMut<PlayerClock>,
    mut q: Query<(&mut Player, &PlayerInputBuffer)>,
) {
    let Some((mut player, buffer)) = q.iter_mut().next() else {
        return; // no player: don't accumulate, so spawn doesn't burst catch-up ticks
    };
    let ticks = clock.ticks_for(frame.dt);
    for _ in 0..ticks {
        player.prev = player.curr;
        player.curr = player_step(&player.curr, &buffer.pending, &world.0);
    }
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
