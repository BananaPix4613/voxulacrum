//! Tier-2/3 scatter instance render pass (design doc §6, §11).
//!
//! Scatter instances are built on the CPU from each chunk's `ScatterStore`, with
//! player overrides applied, and grouped by `PrefabId` (`meshing::foliage`).
//! This pass instance-draws them: one procedural mesh per prefab, one instanced
//! draw per prefab. Instance color/scale come from the prefab definition.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::{ScatterInstanceGpu, ScatterVertex};
use crate::rendering::render_context::RenderContext;
use voxel_core::ChunkVersion;

/// GPU mesh for one prefab shape.
pub struct PrefabMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

/// Per-chunk, per-prefab instance buffer.
pub struct PrefabInstances {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

/// Per-chunk scatter: one optional instance buffer per prefab id (index = `PrefabId`).
pub struct ChunkScatter {
    pub by_prefab: Vec<Option<PrefabInstances>>,
    /// The chunk version these instances were built from.
    pub version: ChunkVersion,
}

#[derive(Resource)]
pub struct ScatterPass {
    /// One mesh per prefab id (index = `PrefabId`).
    pub prefab_meshes: Vec<PrefabMesh>,
    /// Per-chunk instance buffers, keyed by chunk position.
    pub chunk_scatter: HashMap<IVec3, ChunkScatter>,
}

impl ScatterPass {
    /// Upload one mesh per prefab, in `PrefabId` order (`meshing::foliage::prefab_meshes`).
    /// Chunks are added as they mesh.
    pub fn new(ctx: &RenderContext, prefab_meshes: &[(Vec<ScatterVertex>, Vec<u32>)]) -> Self {
        let prefab_meshes: Vec<PrefabMesh> = prefab_meshes
            .iter()
            .map(|(verts, indices)| {
                let vertex_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scatter_prefab_vertex_buffer"),
                    contents: bytemuck::cast_slice(verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let index_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("scatter_prefab_index_buffer"),
                    contents: bytemuck::cast_slice(indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                PrefabMesh { vertex_buffer, index_buffer, index_count: indices.len() as u32 }
            })
            .collect();

        Self { prefab_meshes, chunk_scatter: HashMap::new() }
    }

    /// Replace `pos`'s instance buffers with `instances` (one list per
    /// `PrefabId`), built from the chunk at `version`, or drop them when the
    /// chunk has no scatter (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        instances: Option<&[Vec<ScatterInstanceGpu>]>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.chunk_scatter.get(&pos).map_or(true, |old| old.version <= version),
            "scatter for chunk {pos} was built from an older version than the one it replaces",
        );
        let Some(instances) = instances else {
            self.chunk_scatter.remove(&pos);
            return;
        };
        let by_prefab = instances
            .iter()
            .map(|insts| {
                if insts.is_empty() {
                    None
                } else {
                    let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("scatter_instance_buffer_chunk"),
                        contents: bytemuck::cast_slice(insts),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    Some(PrefabInstances { instance_buffer, instance_count: insts.len() as u32 })
                }
            })
            .collect();
        self.chunk_scatter.insert(pos, ChunkScatter { by_prefab, version });
    }

    /// Drop a chunk's instances on unload.
    pub fn remove(&mut self, pos: IVec3) {
        self.chunk_scatter.remove(&pos);
    }
}
