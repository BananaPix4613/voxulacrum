pub mod voxel;
pub mod chunk;
pub mod generation;

use chunk::{Chunk, ChunkMesh, ChunkNeighbors, CHUNK_VOLUME};
use generation::{TerrainGenerator, WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z};
use voxel::{MATERIAL_COUNT, MATERIAL_TABLE};
use wgpu::util::DeviceExt;

pub struct World {
    pub chunks: Vec<Chunk>,
    pub chunks_x: usize,
    pub chunks_y: usize,
    pub chunks_z: usize,
    pub generator: TerrainGenerator,
}

use crate::params::TerrainGenParams;

impl World {
    pub fn generate(params: &TerrainGenParams) -> Self {
        let generator = TerrainGenerator::new(params);
        let chunks = generator.generate_world(params);

        Self {
            chunks,
            chunks_x: WORLD_CHUNKS_X,
            chunks_y: WORLD_CHUNKS_Y,
            chunks_z: WORLD_CHUNKS_Z,
            generator,
        }
    }
    
    /// Upload a completed mesh result to the GPU for a specific chunk.
    pub fn upload_mesh_result(
        &mut self,
        chunk_index: usize,
        vertices: &[crate::rendering::pipelines::TerrainVertex],
        indices: &[u32],
        device: &wgpu::Device,
    ) {
        if vertices.is_empty() || indices.is_empty() {
            self.chunks[chunk_index].mesh = None;
            self.chunks[chunk_index].mesh_dirty = false;
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
        
        self.chunks[chunk_index].mesh = Some(ChunkMesh {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        });
        self.chunks[chunk_index].mesh_dirty = false;
    }

    pub fn build_neighbors(&self, cx: usize, cy: usize, cz: usize) -> ChunkNeighbors {
        let mut neighbors = ChunkNeighbors::empty();
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 && dz == 0 { continue; }
                    let nx = cx as i32 + dx;
                    let ny = cy as i32 + dy;
                    let nz = cz as i32 + dz;
                    if nx >= 0 && (nx as usize) < self.chunks_x
                        && ny >= 0 && (ny as usize) < self.chunks_y
                        && nz >= 0 && (nz as usize) < self.chunks_z
                    {
                        neighbors.set(dx, dy, dz, self.get_chunk(nx as usize, ny as usize, nz as usize));
                    }
                }
            }
        }
        neighbors
    }

    /// Get a chunk by its chunk-space coordinates, or None if out of bounds.
    pub fn get_chunk(&self, cx: usize, cy: usize, cz: usize) -> Option<&Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get(index)
    }

    /// Get a mutable chunk by its chunk-space coordinates.
    pub fn get_chunk_mut(&mut self, cx: usize, cy: usize, cz: usize) -> Option<&mut Chunk> {
        if cx >= self.chunks_x || cy >= self.chunks_y || cz >= self.chunks_z {
            return None;
        }
        let index = cx + cy * self.chunks_x + cz * self.chunks_x * self.chunks_y;
        self.chunks.get_mut(index)
    }

    /// Print debug statistics about the generated world.
    pub fn print_debug_stats(&self) {
        let mut total_solid: u64 = 0;
        let mut total_air: u64 = 0;
        let mut material_counts = [0u64; MATERIAL_COUNT];
        let mut min_density: i8 = i8::MAX;
        let mut max_density: i8 = i8::MIN;
        let mut flora_count: u64 = 0;

        for chunk in &self.chunks {
            for voxel in chunk.voxels.iter() {
                if voxel.density > 0 {
                    total_solid += 1;
                } else {
                    total_air += 1;
                }
                let mat = voxel.material as usize;
                if mat < MATERIAL_COUNT {
                    material_counts[mat] += 1;
                }
                if voxel.density < min_density {
                    min_density = voxel.density;
                }
                if voxel.density > max_density {
                    max_density = voxel.density;
                }
                if voxel.flora_id != 0 {
                    flora_count += 1;
                }
            }
        }

        let total = total_solid + total_air;
        log::info!("=== World Generation Stats ===");
        log::info!("Chunks: {}", self.chunks.len());
        log::info!(
            "Total voxels: {} ({} solid, {} air)",
            total,
            total_solid,
            total_air
        );
        log::info!(
            "Solid: {:.1}%, Air: {:.1}%",
            total_solid as f64 / total as f64 * 100.0,
            total_air as f64 / total as f64 * 100.0
        );
        log::info!("Density range: {} to {}", min_density, max_density);
        log::info!("Flora voxels: {}", flora_count);
        log::info!("--- Material distribution ---");
        for (id, count) in material_counts.iter().enumerate() {
            if *count > 0 {
                log::info!(
                    "  [{}] {}: {} ({:.1}%)",
                    id,
                    MATERIAL_TABLE[id].name,
                    count,
                    *count as f64 / total as f64 * 100.0
                );
            }
        }
        log::info!(
            "Voxel memory: ~{} MB",
            self.chunks.len() * CHUNK_VOLUME * 12 / (1024 * 1024)
        );
        log::info!("==============================")
    }
}