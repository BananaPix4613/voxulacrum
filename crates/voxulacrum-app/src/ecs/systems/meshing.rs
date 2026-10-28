//! The client's per-chunk builds: terrain meshes as they land, and the foliage
//! and water built alongside them, all queued into `ChunkUploads`.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::diagnostics::{FrameTimings, SUB_UPLOAD};
use crate::ecs::resources::VoxelWorld;
use crate::foliage::FoliageRegistryRes;
use crate::jobs::JobSystem;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::meshing::foliage::{build_capsules, build_detail_paint, build_lobes, build_scatter};
use crate::meshing::uploads::{ChunkUpload, ChunkUploads};
use crate::meshing::water::build_water_surface;
use crate::prefabs::PrefabRegistryRes;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::chunk::CHUNK_WORLD_SIZE;

/// Re-mesh water for `chunks` plus each one's 4 horizontal neighbors. The water
/// mesher welds surface heights across chunk seams (design §7 rendering), so a
/// chunk whose fluid changed shifts its neighbors' boundary columns too. Only
/// X/Z neighbors matter: the mesher samples horizontally within one chunk-Y band.
/// Neighbors that aren't resident are skipped, and the set dedups overlap so
/// each chunk rebuilds at most once per call.
pub(super) fn rebuild_water_with_neighbors(
    uploads: &mut ChunkUploads,
    world: &crate::world::World,
    chunks: impl IntoIterator<Item = IVec3>,
) {
    let mut set: std::collections::HashSet<IVec3> = std::collections::HashSet::new();
    for pos in chunks {
        set.insert(pos);
        for d in [
            IVec3::new(1, 0, 0), IVec3::new(-1, 0, 0),
            IVec3::new(0, 0, 1), IVec3::new(0, 0, -1),
        ] {
            set.insert(pos + d);
        }
    }
    for pos in set {
        // Not resident: nothing to build, and its unload already queued the removal.
        let Some(chunk) = world.get_chunk(pos) else { continue };
        uploads.water.insert(pos, ChunkUpload::Set {
            version: chunk.version,
            data: build_water_surface(pos, world),
        });
    }
}

pub fn meshing_tick_system(
    mut meshing: ResMut<MeshingCoordinator>,
    world: Res<VoxelWorld>,
    mut jobs: ResMut<JobSystem>,
    sim: Res<SimulationManager>,
    mut ui: ResMut<UiState>,
    foliage: Res<FoliageRegistryRes>,
    prefabs: Res<PrefabRegistryRes>,
    mut uploads: ResMut<ChunkUploads>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_UPLOAD);
    // Phase 1: tick meshing, queuing finished meshes - collects meshed positions.
    let cam = sim.camera.smooth_target;
    let camera_chunk = IVec3::new(
        (cam.x / CHUNK_WORLD_SIZE).floor() as i32,
        0,
        (cam.z / CHUNK_WORLD_SIZE).floor() as i32,
    );
    // Read before `ui` is mutably borrowed by the call below.
    let max_snapshots = ui.params.meshing.max_mesh_per_frame as usize;
    let meshed = meshing.tick(
        &world.0,
        &mut uploads,
        &mut jobs,
        &mut ui,
        camera_chunk,
        max_snapshots,
    );

    // Phase 2: for each newly meshed chunk, build its foliage and water on the
    // CPU and queue them beside its terrain, so they reach the GPU on the same
    // frame. Startup chunks arrive here too, through the change feed.
    if !meshed.is_empty() {
        for pos in &meshed {
            // `tick` only reports resident chunks.
            let Some(chunk) = world.0.get_chunk(*pos) else { continue };
            let version = chunk.version;
            uploads.detail_paint.insert(*pos, ChunkUpload::Set {
                version,
                data: build_detail_paint(chunk, &foliage),
            });
            uploads.scatter.insert(*pos, ChunkUpload::Set {
                version,
                data: build_scatter(chunk, &prefabs),
            });
            uploads.capsules.insert(*pos, ChunkUpload::Set { version, data: build_capsules(chunk) });
            uploads.lobes.insert(*pos, ChunkUpload::Set { version, data: build_lobes(chunk) });
        }
        // Water welds across seams, so also refresh newly-meshed chunks' neighbors
        // (detail/scatter are per-chunk and don't need this).
        rebuild_water_with_neighbors(&mut uploads, &world.0, meshed.iter().copied());
    }
}
