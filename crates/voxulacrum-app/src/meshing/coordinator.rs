use std::collections::HashMap;
use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;
use crate::meshing::MeshingPipeline;
use crate::rendering::pipelines::FaceVertex;
use crate::rendering::render_context::RenderContext;
use crate::ui::panels::UiState;
use crate::world::World;

#[derive(Resource)]
pub struct MeshingCoordinator {
    pub pipeline: MeshingPipeline,
}

/// One chunk's terrain mesh on the GPU.
pub struct ChunkMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    /// Vertex + index buffer bytes. Recorded at upload rather than inferred from
    /// `index_count` and quad topology, so the residency panel reports the real
    /// GPU footprint instead of an estimate.
    pub gpu_bytes: u64,
}

/// The terrain meshes of resident chunks, keyed by chunk coordinate. Client
/// state: the world holds no mesh (R4). An entry goes when its chunk unloads
/// (`streaming_tick_system`); the render world takes these over later in R4.
#[derive(Resource, Default)]
pub struct ChunkMeshes {
    pub meshes: HashMap<IVec3, ChunkMesh>,
}

impl ChunkMeshes {
    /// Upload a finished mesh for `pos`, replacing the old one. An empty mesh
    /// removes the entry, since the chunk has nothing to draw.
    pub fn upload(
        &mut self,
        pos: IVec3,
        vertices: &[FaceVertex],
        indices: &[u32],
        device: &wgpu::Device,
    ) {
        if vertices.is_empty() || indices.is_empty() {
            self.meshes.remove(&pos);
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

        self.meshes.insert(
            pos,
            ChunkMesh {
                vertex_buffer,
                index_buffer,
                index_count: indices.len() as u32,
                gpu_bytes: (size_of_val(vertices) + size_of_val(indices)) as u64,
            },
        );
    }
    
    /// Drop the mesh of an unloaded chunk. GPU buffers are freed on drop.
    pub fn remove(&mut self, pos: IVec3) {
        self.meshes.remove(&pos);
    }
    
    /// Vertex + index bytes across every mesh.
    pub fn gpu_bytes(&self) -> u64 {
        self.meshes.values().map(|m| m.gpu_bytes).sum()
    }
}

impl MeshingCoordinator {
    pub fn new(pipeline: MeshingPipeline) -> Self {
        Self { pipeline }
    }
    
    /// Poll for completed meshes, handle UI requests.
    /// Returns the chunk positions that received new mesh data this frame.
    pub fn tick(
        &mut self,
        world: &World,
        meshes: &mut ChunkMeshes,
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
        self.pipeline.submit_dirty();

        // Drain pending snapshot submissions (bounded per frame)
        self.pipeline.drain_pending_submissions(world, jobs, camera_chunk, max_snapshots);
        
        // Poll completed chunks
        let completed = self.pipeline.poll();
        let mut meshed_positions = Vec::with_capacity(completed.len());
        for result in completed {
            let pos = result.chunk_key;
            // The chunk unloaded while its mesh was in flight.
            if world.get_chunk(pos).is_none() {
                continue;
            }
            meshes.upload(pos, &result.vertices, &result.indices, &ctx.device);
            self.pipeline.mark_meshed(pos, result.mesh_seq);
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
            for &coord in world.chunks.keys() {
                self.pipeline.invalidate(coord.into(), false);
            }
            self.pipeline.submit_dirty();
        }
        
        meshed_positions
    }
}
