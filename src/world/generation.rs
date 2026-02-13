use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use glam::IVec3;

use super::chunk::{Chunk, CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use super::voxel::*;

/// World dimensions in chunks
pub const WORLD_CHUNKS_X: usize = 8;
pub const WORLD_CHUNKS_Y: usize = 4;
pub const WORLD_CHUNKS_Z: usize = 8;

/// World dimensions in meters
pub const WORLD_SIZE_X: f32 = WORLD_CHUNKS_X as f32 * CHUNK_WORLD_SIZE;
pub const WORLD_SIZE_Y: f32 = WORLD_CHUNKS_Y as f32 * CHUNK_WORLD_SIZE;
pub const WORLD_SIZE_Z: f32 = WORLD_CHUNKS_Z as f32 * CHUNK_WORLD_SIZE;

/// Base terrain height (sea level reference) in world meters
const BASE_HEIGHT: f32 = 32.0;
/// Steepness threshold for limestone cliffs
const CLIFF_THRESHOLD: f32 = 2.0;

pub struct TerrainGenerator {
    height_fbm: FastNoiseLite,
    height_ridged: FastNoiseLite,
    height_detail: FastNoiseLite,
    cave_noise: FastNoiseLite,
    material_noise: FastNoiseLite,
    flora_noise: FastNoiseLite,
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

        Self {
            height_fbm,
            height_ridged,
            height_detail,
            cave_noise,
            material_noise,
            flora_noise,
        }
    }

    /// Compute terrain height at a world-space XZ position (in meters).
    pub fn terrain_height(&self, wx: f32, wz: f32) -> f32 {
        BASE_HEIGHT
            + 20.0 * self.height_fbm.get_noise_2d(wx * 0.0025, wz * 0.0025)
            + 8.0 * self.height_ridged.get_noise_2d(wx * 0.01, wz * 0.01)
            + 2.0 * self.height_detail.get_noise_2d(wx * 0.05, wz * 0.05)
    }

    /// Compute cliff steepness via finite differences on the heightmap.
    fn cliff_steepness(&self, wx: f32, wz: f32) -> f32 {
        let step = VOXEL_SCALE;
        let dx = self.terrain_height(wx + step, wz) - self.terrain_height(wx - step, wz);
        let dz = self.terrain_height(wx, wz + step) - self.terrain_height(wx, wz - step);
        (dx * dx + dz * dz).sqrt() / (2.0 * step)
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
        let chunk_world_x = chunk.position.x as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_y = chunk.position.y as f32 * CHUNK_WORLD_SIZE;
        let chunk_world_z = chunk.position.z as f32 * CHUNK_WORLD_SIZE;

        for lz in 0..CHUNK_SIZE {
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let wx = chunk_world_x + lx as f32 * VOXEL_SCALE;
                    let wy = chunk_world_y + ly as f32 * VOXEL_SCALE;
                    let wz = chunk_world_z + lz as f32 * VOXEL_SCALE;

                    let voxel = chunk.get_voxel_mut(lx, ly, lz);
                    self.generate_voxel(voxel, wx, wy, wz);
                }
            }
        }
    }

    /// Generate a single voxel at the given world position (meters).
    fn generate_voxel(&self, voxel: &mut Voxel, wx: f32, wy: f32, wz: f32) {
        let mut height = self.terrain_height(wx, wz);

        let mut density = height - wy;
        density += 4.0 * self.cave_noise.get_noise_3d(wx * 0.015, wy * 0.015, wz * 0.015);
        voxel.density = density.clamp(-128.0, 127.0) as i8;

        if voxel.density > 0 {
            let depth_below_surface = height - wy;
            let steepness = self.cliff_steepness(wx, wz);
            let mat_noise = self.material_noise.get_noise_2d(wx * 0.05, wz * 0.05);
            
            voxel.material = if steepness > CLIFF_THRESHOLD && depth_below_surface < 6.0 {
                MAT_LIMESTONE
            } else if depth_below_surface < 0.5 {
                MAT_GRASS_SOIL
            } else if depth_below_surface < 2.0 {
                MAT_SOIL
            } else if depth_below_surface < 4.0 {
                if mat_noise > 0.3 {
                    MAT_CLAY
                } else {
                    MAT_SOIL
                }
            } else {
                MAT_GRANITE
            };

            // Moisture (depth-based only, no river)
            let depth_moisture = (depth_below_surface * 10.0).min(100.0);
            voxel.moisture = depth_moisture.clamp(0.0, 255.0) as u8;

            // Flora placement (surface grass voxels only)
            if voxel.material == MAT_GRASS_SOIL && depth_below_surface < 1.5 {
                let flora_val = self.flora_noise.get_noise_2d(wx * 0.08, wz * 0.08);
                if flora_val > 0.3 {
                    voxel.flora_id = 1; // Generic grass
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