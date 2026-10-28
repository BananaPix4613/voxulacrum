//! Chunk streaming, the local camera's area of interest, and the seam pass that
//! finalizes a chunk once its neighbors are resident.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::diagnostics::{
    FrameTimings, SUB_DRAIN, SUB_SCAN, SUB_SEAM, SUB_STREAM, SUB_SUBMIT, SUB_UNLOAD,
};
use crate::ecs::resources::VoxelWorld;
use crate::jobs::JobSystem;
use crate::rendering::frame_state::FrameState;
use crate::ui::panels::UiState;
use crate::world::mutation::{MutationCommand, WorldMutation};
use crate::world::persistence::WorldPersistence;
use crate::world::streaming::{ChunkStreamingManager, ObserverId, Observers};

/// Send the local camera's area of interest to residency when it changes, as
/// observer 0, the local player (design §12, "Residency"). The camera module
/// computes the area into this frame's camera description.
pub fn local_interest_system(frame: Res<FrameState>, mut observers: ResMut<Observers>) {
    let area = frame.camera.area_of_interest;
    if observers.get(ObserverId::LOCAL) != Some(&area) {
        observers.set(ObserverId::LOCAL, area);
    }
}

pub fn streaming_tick_system(
    mut streaming: ResMut<ChunkStreamingManager>,
    mut world: ResMut<VoxelWorld>,
    mut jobs: ResMut<JobSystem>,
    observers: Res<Observers>,
    mut ui: ResMut<UiState>,
    persistence: Res<WorldPersistence>,
    mut timings: ResMut<FrameTimings>,
) {
    // Explicit start/stop rather than `sub_timer`: the guard would hold
    // `timings` borrowed for the whole body, and the per-phase copy below needs
    // it free. Safe here specifically because this system has one exit — the
    // condition the guard exists to protect against does not apply.
    let started = std::time::Instant::now();
    let tick_result = streaming.tick(
        &mut world.0,
        &mut jobs,
        &observers,
        &mut |chunk: &crate::world::chunk::LoadedChunk| {
            if chunk.persist_dirty {
                if let Err(e) = persistence.save_chunk_on_unload(chunk) {
                    log::error!("Save on unload {:?}: {e}", chunk.data.coord);
                }
            }
        },
    );

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
