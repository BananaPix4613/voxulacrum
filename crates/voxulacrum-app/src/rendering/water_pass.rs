use std::collections::HashMap;
use wgpu::util::DeviceExt;
use bevy_ecs::prelude::Resource;
use glam::IVec3;

use crate::rendering::pipelines::WaterVertex;
use crate::rendering::render_context::RenderContext;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::generation::TerrainGenerator;
use crate::params::TerrainGenParams;

/// Per-chunk water mesh on GPU.
pub struct ChunkWaterMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

#[derive(Resource)]
pub struct WaterPass {
    /// Per-chunk water meshes, keyed by chunk position.
    pub chunk_meshes: HashMap<IVec3, ChunkWaterMesh>,
}

impl WaterPass {
    /// Full rebuild - generates water for all loaded chunks.
    /// Used for initial load and terrain regen.
    pub fn new(
        ctx: &RenderContext,
        generator: &TerrainGenerator,
        terrain_params: &TerrainGenParams,
        water_level: f32,
        chunk_positions: impl Iterator<Item = IVec3>,
    ) -> Self {
        let mut chunk_meshes = HashMap::new();
        let mut total_indices = 0u32;

        for pos in chunk_positions {
            let (vertices, indices) = generate_chunk_water_mesh(
                pos, generator, terrain_params, water_level,
            );
            if !indices.is_empty() {
                total_indices += indices.len() as u32;
                let mesh = upload_chunk_water_mesh(&ctx.device, &vertices, &indices);
                chunk_meshes.insert(pos, mesh);
            }
        }

        log::info!("Water pass: {} chunks with water, {} total indices",
            chunk_meshes.len(), total_indices);

        Self { chunk_meshes }
    }

    /// Add water mesh for a single chunk. Uses TerrainGenerator directly.
    pub fn add_chunk_water(
        &mut self,
        pos: IVec3,
        generator: &TerrainGenerator,
        terrain_params: &TerrainGenParams,
        water_level: f32,
        device: &wgpu::Device,
    ) {
        let (vertices, indices) = generate_chunk_water_mesh(
            pos, generator, terrain_params, water_level,
        );
        if !indices.is_empty() {
            let mesh = upload_chunk_water_mesh(device, &vertices, &indices);
            self.chunk_meshes.insert(pos, mesh);
        } else {
            // Remove stale entry if water no longer present
            self.chunk_meshes.remove(&pos);
        }
    }

    /// Remove water mesh for an unloaded chunk. GPU buffers dropped.
    pub fn remove_chunk_water(&mut self, pos: IVec3) {
        self.chunk_meshes.remove(&pos);
    }

    /// Clear all water meshes (used during regen).
    pub fn clear_all(&mut self) {
        self.chunk_meshes.clear();
    }
}

/// Upload water vertex/index data to GPU buffers.
fn upload_chunk_water_mesh(
    device: &wgpu::Device,
    vertices: &[WaterVertex],
    indices: &[u32],
) -> ChunkWaterMesh {
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("water_chunk_vertex_buffer"),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("water_chunk_index_buffer"),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    ChunkWaterMesh {
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
    }
}

/// Generate water mesh for one chunk's 32x32 XZ footprint
///
/// For each voxel-column in the chunk, sample terrain_height. If terrain is
/// below water_level, emit a quad at water_level with appropriate depth.
/// Vertex sharing within the chunk uses an index map.
fn generate_chunk_water_mesh(
    chunk_pos: IVec3,
    generator: &TerrainGenerator,
    terrain_params: &TerrainGenParams,
    water_level: f32,
) -> (Vec<WaterVertex>, Vec<u32>) {
    let cs = CHUNK_SIZE; // 32
    let base_x = chunk_pos.x * cs as i32;
    let base_z = chunk_pos.z * cs as i32;

    // Grid of cs x cs cells, (cs+1) x (cs+1) vertices
    let vw = cs + 1;

    // Determine which cells have water
    let mut has_water = vec![false; cs * cs];
    let mut terrain_heights = vec![0.0f32; cs * cs];
    let mut any_water = false;

    for z in 0..cs {
        for x in 0..cs {
            let wx = (base_x + x as i32) as f32 * VOXEL_SCALE;
            let wz = (base_z + z as i32) as f32 * VOXEL_SCALE;
            let h = generator.terrain_height(wx, wz, terrain_params);
            let idx = z * cs + x;
            terrain_heights[idx] = h;
            if h < water_level {
                has_water[idx] = true;
                any_water = true;
            }
        }
    }

    if !any_water {
        return (Vec::new(), Vec::new());
    }

    // Compute per-vertex depth (averaged from adjacent cells)
    let mut vertex_depths = vec![0.0f32; vw * vw];
    let mut vertex_index_map = vec![u32::MAX; vw * vw];

    for vz in 0..vw {
        for vx in 0..vw {
            let vidx = vz * vw + vx;
            let mut depth_sum = 0.0f32;
            let mut count = 0u32;

            // Check the 4 cells sharing this vertex
            for &(dx, dz) in &[(0i32, 0i32) ,(-1, 0), (0, -1), (-1, -1)] {
                let cx = vx as i32 + dx;
                let cz = vz as i32 + dz;
                if cx >= 0 && cx < cs as i32 && cz >= 0 && cz < cs as i32 {
                    let cidx = cz as usize * cs + cx as usize;
                    if has_water[cidx] {
                        depth_sum += (water_level - terrain_heights[cidx]).max(0.0);
                        count += 1;
                    }
                }
            }

            if count > 0 {
                vertex_depths[vidx] = depth_sum / count as f32;
            }
        }
    }

    // Build vertices and indices
    let mut vertices: Vec<WaterVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for cz in 0..cs {
        for cx in 0..cs {
            if !has_water[cz * cs + cx] { continue; }

            let corners = [
                (cx,     cz),
                (cx + 1, cz),
                (cx + 1, cz + 1),
                (cx,     cz + 1),
            ];
            let mut ci = [0u32; 4];

            for (i, &(vx, vz)) in corners.iter().enumerate() {
                let vidx = vz * vw + vx;
                if vertex_index_map[vidx] == u32::MAX {
                    vertex_index_map[vidx] = vertices.len() as u32;
                    vertices.push(WaterVertex {
                        position: [
                            (base_x + vx as i32) as f32 * VOXEL_SCALE,
                            water_level,
                            (base_z + vz as i32) as f32 * VOXEL_SCALE,
                        ],
                        flow: [0.0, 0.0],
                        depth: vertex_depths[vidx].clamp(0.0, 10.0),
                    });
                }
                ci[i] = vertex_index_map[vidx];
            }

            indices.extend_from_slice(&[ci[0], ci[1], ci[2], ci[0], ci[2], ci[3]]);
        }
    }

    (vertices, indices)
}