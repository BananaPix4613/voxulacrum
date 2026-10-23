use bevy_ecs::prelude::Resource;
use glam::IVec3;
use voxel_core::ChunkCoord;
use wgpu::util::DeviceExt;
use crate::meshing::MeshingPipeline;
use crate::rendering::pipelines::FaceVertex;
use crate::rendering::render_context::RenderContext;
use crate::ui::panels::UiState;
use crate::world::chunk::ChunkMesh;
use crate::world::World;

#[derive(Resource)]
pub struct MeshingCoordinator {
    pub pipeline: MeshingPipeline,
}

impl MeshingCoordinator {
    pub fn new(pipeline: MeshingPipeline) -> Self {
        Self { pipeline }
    }
    
    /// Poll for completed meshes, handle UI requests.
    /// Returns the chunk positions that received new mesh data this frame.
    pub fn tick(
        &mut self,
        world: &mut World,
        jobs: &mut crate::jobs::JobSystem,
        ctx: &RenderContext,
        ui_state: &mut UiState,
        camera_chunk: IVec3,
        max_snapshots: usize,
    ) -> Vec<IVec3> {
        // Retry every frame: a chunk marked dirty this frame is still inside its
        // debounce window and gets skipped here, but the next frame's call (or the
        // one after) picks it up once the window clears - this is what makes an
        // edit's mesh update land within a frame or two instead of only on the next
        // incidental streaming-triggered sweep.
        self.pipeline.submit_all_dirty(world);

        // Drain pending snapshot submissions (bounded per frame)
        self.pipeline.drain_pending_submissions(world, jobs, camera_chunk, max_snapshots);
        
        // Poll completed chunks
        let completed = self.pipeline.poll();
        let mut meshed_positions = Vec::with_capacity(completed.len());
        for result in completed {
            let pos = result.chunk_key;
            upload_mesh_result(
                world,
                pos,
                &result.vertices,
                &result.indices,
                &ctx.device,
                result.mesh_seq,
            );
            meshed_positions.push(pos);
        }
        
        // Update meshing stats for UI
        ui_state.meshing_stats = self.pipeline.stats.clone();
        ui_state.remeshing = !self.pipeline.is_idle();
        
        // Handle cache clear request
        if ui_state.clear_cache_requested {
            ui_state.clear_cache_requested = false;
            self.pipeline.clear_cache();
            log::info!("Cache cleared by user");
        }
        
        // Handle explicit "Remesh" button press
        if ui_state.remesh_requested {
            ui_state.remesh_requested = false;
            ui_state.mesh_params_pending = false;
            log::info!("Remesh requested by user");
            self.pipeline.reset_for_new_world();
            for chunk in world.chunks.values_mut() {
                chunk.mark_mesh_dirty();
            }
            self.pipeline.submit_all_dirty(world);
        }
        
        meshed_positions
    }
}

/// Upload a completed mesh result to the GPU for a specific chunk.
///
/// Here rather than on `World`, so the world code names no render type. It
/// still writes the GPU state `LoadedChunk` holds; R4 moves that state into the
/// render world.
fn upload_mesh_result(
    world: &mut World,
    chunk_key: IVec3,
    vertices: &[FaceVertex],
    indices: &[u32],
    device: &wgpu::Device,
    mesh_seq: u64,
) {
    let chunk = match world.chunks.get_mut(&ChunkCoord::from(chunk_key)) {
        Some(c) => c,
        None => return, // Chunk was unloaded while mesh was in flight
    };

    if vertices.is_empty() || indices.is_empty() {
        chunk.mesh = None;
        // Only clear mesh_dirty if the chunk wasn't re-dirtied during meshing
        if chunk.mesh_seq == mesh_seq {
            chunk.mesh_dirty = false;
        }
        return;
    }

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk_vertex_buffer"),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk_index_buffer"),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    chunk.mesh = Some(ChunkMesh {
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
        gpu_bytes: (size_of_val(vertices) + size_of_val(indices)) as u64,
    });
    if chunk.mesh_seq == mesh_seq {
        chunk.mesh_dirty = false;
    }
}
