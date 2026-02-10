pub mod voxel;
pub mod chunk;
pub mod generation;

use chunk::{Chunk, CHUNK_VOLUME};
use generation::{TerrainGenerator, WORLD_CHUNKS_X, WORLD_CHUNKS_Y, WORLD_CHUNKS_Z};
use voxel::{MATERIAL_COUNT, MATERIAL_TABLE};

pub struct World {
    pub chunks: Vec<Chunk>,
    pub chunks_x: usize,
    pub chunks_y: usize,
    pub chunks_z: usize,
}

impl World {
    pub fn generate(seed: i32) -> Self {
        let generator = TerrainGenerator::new(seed);
        let chunks = generator.generate_world();
        
        Self {
            chunks,
            chunks_x: WORLD_CHUNKS_X,
            chunks_y: WORLD_CHUNKS_Y,
            chunks_z: WORLD_CHUNKS_Z,
        }
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