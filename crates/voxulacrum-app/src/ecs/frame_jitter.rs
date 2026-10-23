//! The frame-jitter test (roadmap R2). Each tick phase is a function of world
//! state, the tick number and the input queued for that tick, so the way frame
//! time happens to fall must not change the state at a given tick.

use std::collections::HashMap;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use glam::{IVec3, Vec2, Vec3};
use voxel_core::{ChunkCoord, MaterialId, Voxel};

use crate::ecs::clocks::TickDriver;
use crate::ecs::resources::{FluidChanges, VoxelWorld};
use crate::ecs::schedule::build_tick_schedule;
use crate::play::{PlaySettings, PlayerTuning, SpawnRule};
use crate::player::sim::{InputFrame, PlayerState};
use crate::player::systems::{Player, PlayerInputBuffer};
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE};
use crate::world::config::ServerConfig;
use crate::world::mutation::{MutationCommand, WorldMutation};
use crate::world::persistence::{self, WorldPersistence};
use crate::world::storage::ChunkStorage;
use crate::world::tick::{run_tick, secs_to_ticks, CurrentTick, SimTick};
use crate::world::{split_world_voxel, world_generator};

/// The tick every run stops at.
const END: SimTick = SimTick(3_000);

/// The sandbox's tuning, written out so this test doesn't follow its file.
const TUNING: PlayerTuning = PlayerTuning {
    half_width: 0.3,
    height: 1.5,
    eye_height: 1.6,
    gravity: 24.0,
    move_speed: 4.5,
    max_fall_speed: 50.0,
    step_height: 1.0,
    reach: 4.5,
};

/// Where the player starts: above the floor, so every run includes a fall.
const START: Vec3 = Vec3::new(8.5, 3.0, 8.5);

/// The basin's walls, the same on both horizontal axes. They enclose x and z
/// from -3 to 2, which straddles the seams between the four chunks.
const WALL_MIN: i32 = -4;
const WALL_MAX: i32 = 3;

/// How high the basin's walls rise above the floor, in voxels: room for every
/// pour a run makes.
const WALL_HEIGHT: i32 = 6;

/// Simulated seconds between pours into the basin.
const POUR_EVERY_SECS: f32 = 10.0;

/// Four chunk columns meeting at the origin, one chunk tall, with a one-voxel
/// floor at y = 0 and a walled basin around the origin. Water poured there
/// crosses the seams between all four chunks and stays held, so the state at
/// the end still has water to compare.
fn basin_world() -> crate::world::World {
    let mut storages: HashMap<IVec3, ChunkStorage> = [(-1, -1), (-1, 0), (0, -1), (0, 0)]
        .into_iter()
        .map(|(x, z)| (IVec3::new(x, 0, z), ChunkStorage::new_air()))
        .collect();
    let mut solid = |p: IVec3| {
        let (chunk, local) = split_world_voxel(p);
        storages
            .get_mut(&chunk)
            .expect("inside the four chunks")
            .set_voxel(local.to_index(), Voxel::cube(MaterialId(1)));
    };
    let d = CHUNK_SIZE as i32;
    for x in -d..d {
        for z in -d..d {
            solid(IVec3::new(x, 0, z));
        }
    }
    for y in 1..=WALL_HEIGHT {
        for i in WALL_MIN..=WALL_MAX {
            solid(IVec3::new(WALL_MIN, y, i));
            solid(IVec3::new(WALL_MAX, y, i));
            solid(IVec3::new(i, y, WALL_MIN));
            solid(IVec3::new(i, y, WALL_MAX));
        }
    }
    let chunks = storages
        .into_iter()
        .map(|(pos, storage)| (ChunkCoord::from(pos), LoadedChunk::new(pos, Arc::new(storage))))
        .collect();
    crate::world::World::from_chunks(
        chunks,
        world_generator::load_default().expect("default generator loads"),
        Arc::new(crate::materials::embedded_registry()),
        0,
        1,
    )
}

/// An ECS world holding only what the tick schedule reads. It has no window,
/// renderer or UI resource, so the test also shows the schedule needs none,
/// which the headless server (R3c) depends on.
fn tick_world() -> (World, Entity) {
    let mut ecs = World::new();
    ecs.insert_resource(VoxelWorld(basin_world()));
    ecs.insert_resource(ServerConfig::default());
    ecs.insert_resource(FluidChanges::default());
    ecs.insert_resource(WorldPersistence::disabled());
    ecs.insert_resource(CurrentTick::default());
    ecs.insert_resource(PlaySettings {
        player: TUNING,
        spawn: SpawnRule::SurfaceUnderCamera,
        hud: false,
        build: None,
    });
    let state = PlayerState { position: START, ..Default::default() };
    let player = ecs
        .spawn((Player { curr: state, prev: state }, PlayerInputBuffer::default()))
        .id();
    ecs.add_schedule(build_tick_schedule());
    (ecs, player)
}

/// The input queued for `tick`: one simulated second each of +X, +Z, -X and
/// -Z, repeating, so the player walks a square and stays on the floor.
fn input_for(tick: SimTick) -> InputFrame {
    let move_world = match (tick.0 / secs_to_ticks(1.0)) % 4 {
        0 => Vec2::X,
        1 => Vec2::Y,
        2 => -Vec2::X,
        _ => -Vec2::Y,
    };
    InputFrame { move_world, jump: false }
}

/// Apply the commands queued for `tick`, before it runs, where tick phase 1
/// will apply them once commands queue (R5): a pour into the basin every
/// `POUR_EVERY_SECS`, so the fluid phase has work for the whole run.
fn apply_commands_for(ecs: &mut World, tick: SimTick) {
    if tick.is_multiple_of(secs_to_ticks(POUR_EVERY_SECS)) {
        ecs.resource_mut::<VoxelWorld>()
            .0
            .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn {
                anchor: IVec3::ZERO,
            }))
            .expect("authoring pour in authoring mode");
    }
}

/// Frame times from 5 to 60 ms, from a seeded splitmix64 stream.
fn jitter(seed: u64) -> impl Iterator<Item = f32> {
    let mut state = seed;
    std::iter::repeat_with(move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        0.005 + 0.055 * (z >> 40) as f32 / (1u64 << 24) as f32
    })
}

/// What one run reached tick `END` with.
struct Run {
    /// Hash of the player's state and every chunk's fluid layer.
    hash: u64,
    /// Frames it took to get there.
    frames: usize,
    /// Furthest the player got from its start, horizontally.
    walked: f32,
    /// Chunks other than the one poured into that hold water at the end.
    water_chunks: usize,
}

/// Drive the tick schedule through the tick driver, one frame time at a time,
/// until tick `END` has run.
fn run_to_end(frame_times: impl IntoIterator<Item = f32>) -> Run {
    let (mut ecs, player) = tick_world();
    let mut driver = TickDriver::default();
    let mut walked: f32 = 0.0;
    for (frame, dt) in frame_times.into_iter().enumerate().take(100_000) {
        for _ in 0..driver.advance(dt) {
            let tick = ecs.resource::<CurrentTick>().0;
            ecs.get_mut::<PlayerInputBuffer>(player).expect("player").pending = input_for(tick);
            apply_commands_for(&mut ecs, tick);
            run_tick(&mut ecs);
            let p = ecs.get::<Player>(player).expect("player").curr.position;
            walked = walked.max(Vec2::new(p.x - START.x, p.z - START.z).length());
            if ecs.resource::<CurrentTick>().0 == END {
                return finish(&ecs, player, frame + 1, walked);
            }
        }
    }
    panic!("the frames ran out before tick {}", END.0);
}

/// Hash the state the run ended in, over the persistence encoding.
fn finish(ecs: &World, player: Entity, frames: usize, walked: f32) -> Run {
    let p = ecs.get::<Player>(player).expect("player").curr;
    let mut state = Vec::new();
    for v in [
        p.position.x, p.position.y, p.position.z,
        p.velocity.x, p.velocity.y, p.velocity.z,
        p.facing,
    ] {
        state.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    state.push(u8::from(p.on_ground));
    state.push(p.flags);

    let world = &ecs.resource::<VoxelWorld>().0;
    let mut coords: Vec<IVec3> = world.chunks.keys().map(|&c| IVec3::from(c)).collect();
    coords.sort_unstable_by_key(|c| (c.x, c.y, c.z));
    let mut water_chunks = 0;
    for pos in coords {
        let fluids = &world.get_chunk(pos).expect("chunks stay resident").data.fluids;
        if pos != IVec3::ZERO && !fluids.cells.is_empty() {
            water_chunks += 1;
        }
        persistence::write_fluid_layer(&mut state, fluids);
    }
    Run { hash: seahash::hash(&state), frames, walked, water_chunks }
}

#[test]
fn state_at_a_tick_is_the_same_however_the_frames_fall() {
    let runs = [
        ("constant 16.7 ms", run_to_end(std::iter::repeat(0.0167))),
        ("constant 7 ms", run_to_end(std::iter::repeat(0.007))),
        ("jitter from 5 to 60 ms", run_to_end(jitter(0x5EED_F00D))),
    ];
    for (name, run) in &runs {
        assert!(run.walked > 2.0, "{name}: the player got only {} from its start", run.walked);
        assert_eq!(
            run.water_chunks, 3,
            "{name}: the basin straddles four chunks, so water must end up in the three \
             it wasn't poured into",
        );
    }
    let frames: Vec<usize> = runs.iter().map(|(_, run)| run.frames).collect();
    assert!(
        frames[0] != frames[1] && frames[1] != frames[2] && frames[0] != frames[2],
        "the sequences must reach tick {} in different numbers of frames: {frames:?}",
        END.0,
    );
    for (name, run) in &runs[1..] {
        assert_eq!(
            run.hash, runs[0].1.hash,
            "{name} reached tick {} in a different state from {}",
            END.0, runs[0].0,
        );
    }
}
