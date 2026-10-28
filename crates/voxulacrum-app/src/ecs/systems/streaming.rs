//! Chunk streaming, and the seam pass that finalizes a chunk once its
//! neighbors are resident.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::diagnostics::{
    FrameTimings, SUB_DRAIN, SUB_SCAN, SUB_SEAM, SUB_STREAM, SUB_SUBMIT, SUB_UNLOAD,
};
use crate::ecs::resources::VoxelWorld;
use crate::jobs::JobSystem;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::meshing::uploads::ChunkUploads;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::chunk::CHUNK_WORLD_SIZE;
use crate::world::mutation::{MutationCommand, WorldMutation};
use crate::world::persistence::WorldPersistence;
use crate::world::streaming::{CameraView, ChunkStreamingManager};

pub fn streaming_tick_system(
    mut streaming: ResMut<ChunkStreamingManager>,
    mut world: ResMut<VoxelWorld>,
    mut meshing: ResMut<MeshingCoordinator>,
    mut jobs: ResMut<JobSystem>,
    sim: Res<SimulationManager>,
    mut uploads: ResMut<ChunkUploads>,
    mut ui: ResMut<UiState>,
    persistence: Res<WorldPersistence>,
    mut timings: ResMut<FrameTimings>,
) {
    // Explicit start/stop rather than `sub_timer`: the guard would hold
    // `timings` borrowed for the whole body, and the per-phase copy below needs
    // it free. Safe here specifically because this system has one exit — the
    // condition the guard exists to protect against does not apply.
    let started = std::time::Instant::now();
    let cam_pos = sim.camera.smooth_target;
    let cam_cx = (cam_pos.x / CHUNK_WORLD_SIZE).floor() as i32;
    let cam_cz = (cam_pos.z / CHUNK_WORLD_SIZE).floor() as i32;
    let camera_view = CameraView {
        zoom: sim.camera.zoom,
        aspect: sim.camera.aspect,
        rotation: sim.camera.rotation,
        camera_chunk: IVec3::new(cam_cx, 0, cam_cz),
        camera_world_pos: cam_pos,
        world_y_range: (
            world.min_chunk_y as f32 * CHUNK_WORLD_SIZE,
            world.max_chunk_y as f32 * CHUNK_WORLD_SIZE,
        ),
    };

    let tick_result = streaming.tick(
        &mut world.0,
        &mut meshing,
        &mut jobs,
        &camera_view,
        &mut |chunk: &crate::world::chunk::LoadedChunk| {
            if chunk.persist_dirty {
                if let Err(e) = persistence.save_chunk_on_unload(chunk) {
                    log::error!("Save on unload {:?}: {e}", chunk.data.coord);
                }
            }
        },
    );

    // Queue the removal of the unloaded chunks' meshes and per-chunk buffers.
    // R6 moves this to the client's chunk view.
    for pos in &tick_result.unloaded {
        uploads.remove(*pos);
    }

    // NOTE: We do NOT add foliage/water here for inserted chunks.
    // Inserted chunks don't have meshes yet. Foliage/water are added
    // in meshing_tick_system when the mesh upload completes, so they
    // appear on the same frame as the terrain.

    // Update streaming stats for UI
    ui.streaming = streaming.stats();

    // The four phases `tick` reported, named here (see `SUB_DRAIN`), plus the
    // whole-system span so the inner split can be checked against it.
    for (slot, ms) in [SUB_DRAIN, SUB_SUBMIT, SUB_SCAN, SUB_UNLOAD]
        .into_iter()
        .zip(tick_result.phase_ms)
    {
        timings.add_sub(slot, ms);
    }
    timings.add_sub(SUB_STREAM, started.elapsed().as_secs_f32() * 1000.0);
}

/// Cross-chunk seam finalization (Substep 2b): once a chunk's six face
/// neighbors are resident, derive the boundary slab demotions the isolated
/// generation pass could not see (surface steps that straddle a chunk border,
/// including chunk-Y seams). One-shot per chunk (`seam_finalized`) and bounded
/// per frame; runs before meshing so the first mesh already carries the seam
/// slabs.
///
/// The derivation is read-only and needs neighbor storage, so it stays here; the
/// write goes through the mutation door as `FinalizeSeam` (P5 - this was the
/// largest remaining bypass, audit §7.1 E-1). It is `System` origin: generation
/// finalization, not an actor edit.
pub fn seam_smoothing_system(
    mut world: ResMut<VoxelWorld>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_SEAM);
    const MAX_PER_FRAME: usize = 16;

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

        world
            .0
            .execute(MutationCommand::system(WorldMutation::FinalizeSeam {
                chunk: pos,
                demotions,
            }))
            .expect("system seam finalization accepted in any mode");
    }
}
