//! Mesh upload, and the per-chunk foliage and water buffers built alongside it.

use bevy_ecs::prelude::*;
use glam::IVec3;
use crate::diagnostics::{FrameTimings, SUB_UPLOAD};
use crate::ecs::resources::VoxelWorld;
use crate::jobs::JobSystem;
use crate::meshing::coordinator::MeshingCoordinator;
use crate::rendering::canopy_pass::CanopyPass;
use crate::rendering::capsule_pass::CapsulePass;
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::scatter_pass::ScatterPass;
use crate::rendering::water_pass::WaterPass;
use crate::simulation::manager::SimulationManager;
use crate::ui::panels::UiState;
use crate::world::chunk::CHUNK_WORLD_SIZE;

/// Re-mesh water for `chunks` plus each one's 4 horizontal neighbors. The water
/// mesher welds surface heights across chunk seams (design §7 rendering), so a
/// chunk whose fluid changed shifts its neighbors' boundary columns too. Only
/// X/Z neighbors matter: the mesher samples horizontally within one chunk-Y band.
/// Neighbors that aren't resident are a no-op (`add_chunk_water` early-outs), and
/// the set dedups overlap so each chunk rebuilds at most once per call.
pub(super) fn rebuild_water_with_neighbors(
    water_pass: &mut WaterPass,
    world: &crate::world::World,
    device: &wgpu::Device,
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
        water_pass.add_chunk_water(pos, world, device);
    }
}

pub fn meshing_tick_system(
    mut meshing: ResMut<MeshingCoordinator>,
    mut world: ResMut<VoxelWorld>,
    mut jobs: ResMut<JobSystem>,
    sim: Res<SimulationManager>,
    ctx: Res<RenderContext>,
    mut ui: ResMut<UiState>,
    mut detail_paint: ResMut<DetailPaintPass>,
    mut scatter: ResMut<ScatterPass>,
    mut capsules: ResMut<CapsulePass>,
    mut canopy: ResMut<CanopyPass>,
    mut water_pass: ResMut<WaterPass>,
    mut timings: ResMut<FrameTimings>,
) {
    let _timer = timings.sub_timer(SUB_UPLOAD);
    // Phase 1: tick meshing with &mut world - collects meshed positions.
    let cam = sim.camera.smooth_target;
    let camera_chunk = IVec3::new(
        (cam.x / CHUNK_WORLD_SIZE).floor() as i32,
        0,
        (cam.z / CHUNK_WORLD_SIZE).floor() as i32,
    );
    // Read before `ui` is mutably borrowed by the call below.
    let max_snapshots = ui.params.meshing.max_mesh_per_frame as usize;
    let meshed = meshing.tick(
        &mut world.0,
        &mut jobs,
        &ctx,
        &mut ui,
        camera_chunk,
        max_snapshots,
    );

    // Phase 2: for each newly meshed chunk, build per-chunk foliage
    // and water GPU buffers. world.0 is now borrowed immutably.
    if !meshed.is_empty() {
        for pos in &meshed {
            detail_paint.add_chunk(*pos, &world.0, &ctx.device);
            scatter.add_chunk(*pos, &world.0, &ctx.device);
            capsules.add_chunk(*pos, &world.0, &ctx.device);
            canopy.add_chunk(*pos, &world.0, &ctx.device);
        }
        // Water welds across seams, so also refresh newly-meshed chunks' neighbors
        // (detail/scatter are per-chunk and don't need this).
        rebuild_water_with_neighbors(&mut water_pass, &world.0, &ctx.device, meshed.iter().copied());
    }
}
