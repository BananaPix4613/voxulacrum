use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use glam::IVec3;

use super::chunk::{Chunk, CHUNK_SIZE};
use super::voxel::*;

/// World dimensions in chunks
pub const WORLD_CHUNKS_X: usize = 8;
pub const WORLD_CHUNKS_Y: usize = 4;
pub const WORLD_CHUNKS_Z: usize = 8;

/// Base terrain height (sea level reference)
const BASE_HEIGHT: f32 = 64.0;
/// Water level for river channel
const WATER_LEVEL: f32 = 60.0;
/// Steepness threshold for limestone cliffs
const CLIFF_THRESHOLD: f32 = 2.0;
/// River channel half-width
const RIVER_WIDTH: f32 = 5.0;

pub struct TerrainGenerator {
    height_fbm: FastNoiseLite,
    height_ridged: FastNoiseLite,
    height_detail: FastNoiseLite,
    cave_noise: FastNoiseLite,
    material_noise: FastNoiseLite,
    flora_noise: FastNoiseLite,
    river_noise: FastNoiseLite,
}

impl TerrainGenerator {
    pub fn new(seed: i32) -> Self {
        let mut height_fbm = FastNoiseLite::with_seed(seed);
        height_fbm.set_noise_type(Some(NoiseType::OpenSimplex2));
        height_fbm.set_fractal_type(Some(FractalType::FBm));
        height_fbm.set_fractal_octaves(Some(5));
        height_fbm.set_frequency(Some(1.0));

        let mut height_ridged = FastNoiseLite::with_seed(seed + 1);
        height_ridged.set_noise_type(Some(NoiseType::OpenSimplex2));
        height_ridged.set_fractal_type(Some(FractalType::Ridged));
        height_ridged.set_fractal_octaves(Some(3));
        height_ridged.set_frequency(Some(1.0));

        let mut height_detail = FastNoiseLite::with_seed(seed + 2);
        height_detail.set_noise_type(Some(NoiseType::OpenSimplex2));
        height_detail.set_frequency(Some(1.0));

        let mut cave_noise = FastNoiseLite::with_seed(seed + 3);
        cave_noise.set_noise_type(Some(NoiseType::OpenSimplex2));
        cave_noise.set_frequency(Some(1.0));

        let mut material_noise = FastNoiseLite::with_seed(seed + 4);
        material_noise.set_noise_type(Some(NoiseType::OpenSimplex2));
        material_noise.set_frequency(Some(1.0));

        let mut flora_noise = FastNoiseLite::with_seed(seed + 5);
        flora_noise.set_noise_type(Some(NoiseType::OpenSimplex2));
        flora_noise.set_frequency(Some(1.0));

        let mut river_noise = FastNoiseLite::with_seed(seed + 6);
        river_noise.set_noise_type(Some(NoiseType::OpenSimplex2));
        river_noise.set_frequency(Some(1.0));

        Self {
            height_fbm,
            height_ridged,
            height_detail,
            cave_noise,
            material_noise,
            flora_noise,
            river_noise,
        }
    }

    /// Compute terrain height at a world XZ position.
    pub fn terrain_height(&self, wx: f32, wz: f32) -> f32 {
        BASE_HEIGHT
            + 20.0 * self.height_fbm.get_noise_2d(wx * 0.005, wz * 0.005)
            + 8.0 * self.height_ridged.get_noise_2d(wx * 0.02, wz * 0.02)
            + 2.0 * self.height_detail.get_noise_2d(wx * 0.1, wz * 0.1)
    }

    /// Compute cliff steepness via finite differences on the heightmap.
    fn cliff_steepness(&self, wx: f32, wz: f32) -> f32 {
        let dx = self.terrain_height(wx + 1.0, wz) - self.terrain_height(wx - 1.0, wz);
        let dz = self.terrain_height(wx, wz + 1.0) - self.terrain_height(wx, wz - 1.0);
        (dx * dx + dz * dz).sqrt() * 0.5
    }

    /// Compute the Z coordinate of the river center at a given X.
    pub fn river_center_z(&self, wx: f32) -> f32 {
        128.0
            + 20.0 * self.river_noise.get_noise_2d(wx * 0.015, 0.0)
            + 6.0 * (wx * 0.05).sin()
    }

    /// Distance from a world XZ point to the river center line.
    pub fn distance_to_river(&self, wx: f32, wz: f32) -> f32 {
        (wz - self.river_center_z(wx)).abs()
    }
    
    /// Generate all chunks for the world.
    pub fn generate_world(&self) -> Vec<Chunk> {
        let total = WORLD_CHUNKS_X * WORLD_CHUNKS_Y * WORLD_CHUNKS_Z;
        let mut chunks = Vec::with_capacity(total);
        
        for cz in 0..WORLD_CHUNKS_Z {
            for cy in 0..WORLD_CHUNKS_Y {
                for cx in 0..WORLD_CHUNKS_X {
                    let mut chunk = Chunk::new(IVec3::new(cx as i32, cy as i32, cz as i32));
                    self.generate_chunk(&mut chunk);
                    chunks.push(chunk);
                }
            }
        }
        
        chunks
    }
    
    /// Fill a single chunk with terrain data.
    fn generate_chunk(&self, chunk: &mut Chunk) {
        let chunk_world_x = chunk.position.x as f32 * CHUNK_SIZE as f32;
        let chunk_world_y = chunk.position.y as f32 * CHUNK_SIZE as f32;
        let chunk_world_z = chunk.position.z as f32 * CHUNK_SIZE as f32;
        
        for lz in 0..CHUNK_SIZE {
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let wx = chunk_world_x + lx as f32;
                    let wy = chunk_world_y + ly as f32;
                    let wz = chunk_world_z + lz as f32;
                    
                    let voxel = chunk.get_voxel_mut(lx, ly, lz);
                    self.generate_voxel(voxel, wx, wy, wz);
                }
            }
        }
    }
    
    /// Generate a single voxel at the given world position.
    fn generate_voxel(&self, voxel: &mut Voxel, wx: f32, wy: f32, wz: f32) {
        // 1. Compute heightmap with river channel carving
        let mut height = self.terrain_height(wx, wz);
        let river_dist = self.distance_to_river(wx, wz);
        
        // Carve river channel
        let channel_depth = (RIVER_WIDTH - river_dist).max(0.0) * 1.5;
        height -= channel_depth;
        
        // 2. Density field
        let mut density = height - wy;
        // Add 3D noise for caves and overhangs
        density += 4.0 * self.cave_noise.get_noise_3d(wx * 0.03, wy * 0.03, wz * 0.03);
        // Clamp to i8 range
        voxel.density = density.clamp(-128.0, 127.0) as i8;
        
        // 3. Material assignment (only for solid materials
        if voxel.density > 0 {
            let depth_below_surface = height - wy;
            let steepness = self.cliff_steepness(wx, wz);
            let mat_noise = self.material_noise.get_noise_2d(wx * 0.05, wz * 0.05);
            
            voxel.material = if river_dist < RIVER_WIDTH && wy < WATER_LEVEL {
                // River bed materials
                if river_dist < 2.0 {
                    MAT_SAND
                } else {
                    MAT_GRAVEL
                }
            } else if steepness > CLIFF_THRESHOLD && depth_below_surface < 12.0 {
                MAT_LIMESTONE
            } else if depth_below_surface < 1.0 {
                MAT_GRASS_SOIL
            } else if depth_below_surface < 4.0 {
                MAT_SOIL
            } else if depth_below_surface < 8.0 {
                if mat_noise > 0.3 {
                    MAT_CLAY
                } else {
                    MAT_SOIL
                }
            } else {
                MAT_GRANITE
            };
            
            // 4. Moisture
            let river_moisture = ((1.0 - river_dist / 30.0) * 200.0).max(0.0);
            let depth_moisture = (depth_below_surface * 5.0).min(100.0);
            voxel.moisture = (river_moisture + depth_moisture).clamp(0.0, 255.0) as u8;
            
            // 5. Flora placement (surface grass voxels only)
            if voxel.material == MAT_GRASS_SOIL && depth_below_surface < 1.5 {
                let flora_val = self.flora_noise.get_noise_2d(wx * 0.08, wz * 0.08);
                if flora_val > 0.3 {
                    voxel.flora_id = 1; // Generic grass
                    // Growth varies from 128-255 based on noise
                    let growth_noise = self.flora_noise.get_noise_2d(wx * 0.2, wz * 0.2);
                    voxel.flora_growth =
                        128 + ((growth_noise + 1.0) * 0.5 * 127.0).clamp(0.0, 127.0) as u8;
                }
            }
        } else {
            voxel.material = MAT_AIR;
        }
    }
}