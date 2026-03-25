use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use glam::IVec3;

use super::chunk::{Chunk, CHUNK_SIZE, CHUNK_WORLD_SIZE, VOXEL_SCALE};
use super::voxel::*;
use crate::params::TerrainGenParams;

/// Initial load radius dimensions in chunks
pub const WORLD_CHUNKS_X: usize = 8;
pub const WORLD_CHUNKS_Y: usize = 4;
pub const WORLD_CHUNKS_Z: usize = 8;

pub struct TerrainGenerator {
    height_fbm: FastNoiseLite,
    height_ridged: FastNoiseLite,
    height_detail: FastNoiseLite,
    // Cave system - 3 morphology layers + domain warp
    cave_spaghetti: FastNoiseLite,
    cave_noodle: FastNoiseLite,
    cave_cheese: FastNoiseLite,
    cave_warp: FastNoiseLite,        // Domain warp generator
    cave_entrance: FastNoiseLite,    // 2D noise for selective surface breaches
    material_noise: FastNoiseLite,
    flora_noise: FastNoiseLite,
}

impl TerrainGenerator {
    pub fn new(params: &TerrainGenParams) -> Self {
        let seed = params.seed;

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

        // Spaghetti caves: OpenSimplex2 with Ridged fractal
        // Ridged noise naturally has values near 0 along connected manifolds,
        // which threshold carving turns into tunnel networks.
        let mut cave_spaghetti = FastNoiseLite::with_seed(seed + 3);
        cave_spaghetti.set_noise_type(Some(NoiseType::OpenSimplex2));
        cave_spaghetti.set_fractal_type(Some(FractalType::Ridged));
        cave_spaghetti.set_fractal_octaves(Some(2));
        cave_spaghetti.set_frequency(Some(1.0));

        // Noodle caves: OpenSimplex2 with PingPong fractal
        // PingPong creates zigzag patterns that fold back on themselves,
        // producing thin branching passages.
        let mut cave_noodle = FastNoiseLite::with_seed(seed + 6);
        cave_noodle.set_noise_type(Some(NoiseType::OpenSimplex2));
        cave_noodle.set_fractal_type(Some(FractalType::PingPong));
        cave_noodle.set_fractal_octaves(Some(2));
        cave_noodle.set_frequency(Some(1.0));

        // Cheese caves: Cellular noise for large open chambers
        // CellularReturnType::Distance2Sub creates bubble-like regions
        // where noise exceeds the threshold, forming chambers.
        let mut cave_cheese = FastNoiseLite::with_seed(seed + 7);
        cave_cheese.set_noise_type(Some(NoiseType::Cellular));
        cave_cheese.set_cellular_distance_function(Some(
            fastnoise_lite::CellularDistanceFunction::EuclideanSq,
        ));
        cave_cheese.set_cellular_return_type(Some(
            fastnoise_lite::CellularReturnType::Distance2Sub,
        ));
        cave_cheese.set_frequency(Some(1.0));

        // Domain warp: distorts sampling coordinates for organic shapes
        let mut cave_warp = FastNoiseLite::with_seed(seed + 8);
        cave_warp.set_domain_warp_type(Some(fastnoise_lite::DomainWarpType::OpenSimplex2));
        cave_warp.set_domain_warp_amp(Some(1.0)); // Amplitude set at sample time
        cave_warp.set_fractal_type(Some(FractalType::DomainWarpProgressive));
        cave_warp.set_fractal_octaves(Some(3));
        cave_warp.set_frequency(Some(1.0));

        // Entrance noise: 2D noise to selectively allow surface breaches
        let mut cave_entrance = FastNoiseLite::with_seed(seed + 9);
        cave_entrance.set_noise_type(Some(NoiseType::OpenSimplex2));
        cave_entrance.set_frequency(Some(1.0));

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
            cave_spaghetti,
            cave_noodle,
            cave_cheese,
            cave_warp,
            cave_entrance,
            material_noise,
            flora_noise,
        }
    }

    pub fn terrain_height(&self, wx: f32, wz: f32, params: &TerrainGenParams) -> f32 {
        params.base_height
            + params.hill_amplitude
            * self.height_fbm.get_noise_2d(wx * params.hill_frequency, wz * params.hill_frequency)
            + params.ridge_amplitude
            * self.height_ridged.get_noise_2d(wx * params.ridge_frequency, wz * params.ridge_frequency)
            + params.detail_amplitude
            * self.height_detail.get_noise_2d(wx * params.detail_frequency, wz * params.detail_frequency)
    }

    fn cliff_steepness(&self, wx: f32, wz: f32, params: &TerrainGenParams) -> f32 {
        let step = VOXEL_SCALE;
        let dx = self.terrain_height(wx + step, wz, params)
            - self.terrain_height(wx - step, wz, params);
        let dz = self.terrain_height(wx, wz + step, params)
            - self.terrain_height(wx, wz - step, params);
        (dx * dx + dz * dz).sqrt() / (2.0 * step)
    }

    /// Returns true if this voxel should be carved (made air) by the cave system.
    fn is_cave(&self, wx: f32, wy: f32, wz: f32, surface_depth: f32, params: &TerrainGenParams) -> bool {
        if !params.cave_enabled || surface_depth < 0.0 {
            return false; // Above terrain surface - never carve
        }

        // --- Surface attenuation ---
        // Fully suppress caves within cave_surface_margin of surface.
        // Ramp to full strength over the next cave_surface_margin units.
        let surface_ramp = ((surface_depth - params.cave_surface_margin) / params.cave_surface_margin)
            .clamp(0.0, 1.0);

        if surface_ramp <= 0.0 {
            // Check for selective surface entrances
            let entrance_val = self.cave_entrance.get_noise_2d(wx * 0.015, wz * 0.015);
            if entrance_val < 0.7 {
                return false; // No entrance here - surface protected
            }
            // Allow entrance: fall through to cave check with partial attenuation
        }

        // --- Water proximity suppression ---
        let water_margin = 4.0;
        let water_dist = (wy - params.water_level).abs();
        let water_ramp = (water_dist / water_margin).clamp(0.0, 1.0);
        if water_ramp <= 0.0 {
            return false;
        }

        // --- Domain warp: distort sampling coordinates for organic tunnel shapes ---
        let (wx_w, wy_w, wz_w) = self.cave_warp.domain_warp_3d(wx, wy, wz);
        // Blend between warped and unwarped based on warp amplitude param
        let warp_t = params.cave_warp_amp / 30.0; // Normalize around default
        let sx = wx + (wx_w - wx) * warp_t;
        let sy = wy + (wy_w - wy) * warp_t;
        let sz = wz + (wz_w - wz) * warp_t;

        // Y-axis squash: lower Y frequency = horizontal bias
        let sy_squashed = sy * params.cave_y_squash;

        // --- Layer 1: Spaghetti tunnels (threshold carving) ---
        let spaghetti = self.cave_spaghetti.get_noise_3d(
            sx * params.cave_spaghetti_freq,
            sy_squashed * params.cave_spaghetti_freq,
            sz * params.cave_spaghetti_freq,
        );
        // Ridged noise has valleys near 0. Threshold carving: |noise| < thickness = tunnel
        let spaghetti_thickness = params.cave_spaghetti_thickness * surface_ramp * water_ramp;
        if spaghetti.abs() < spaghetti_thickness {
            return true;
        }

        // --- Layer 2: Noodle passages (threshold carving, thinner) ---
        let noodle = self.cave_noodle.get_noise_3d(
            sx * params.cave_noodle_freq,
            sy_squashed * params.cave_noodle_freq,
            sz * params.cave_noodle_freq,
        );
        let noodle_thickness = params.cave_noodle_thickness * surface_ramp * water_ramp;
        if noodle.abs() < noodle_thickness {
            return true;
        }

        // --- Layer 3: Cheese chambers (above-threshold carving) ---
        let cheese = self.cave_cheese.get_noise_3d(
            sx * params.cave_cheese_freq,
            sy_squashed * params.cave_cheese_freq,
            sz * params.cave_cheese_freq,
        );
        let cheese_threshold = params.cave_cheese_threshold / (surface_ramp * water_ramp).max(0.01);
        if cheese > cheese_threshold {
            return true;
        }

        false
    }

    pub fn generate_world(
        &self,
        params: &TerrainGenParams,
        min_y: i32,
        max_y: i32,
    ) -> std::collections::HashMap<IVec3, Chunk> {
        let half_x = WORLD_CHUNKS_X as i32 / 2;
        let half_z = WORLD_CHUNKS_Z as i32 / 2;
        let mut chunks = std::collections::HashMap::new();

        for cz in -half_z..half_z {
            for cy in min_y..max_y {
                for cx in -half_x..half_x {
                    let pos = IVec3::new(cx, cy, cz);
                    let mut chunk = Chunk::new(pos);
                    self.generate_chunk(&mut chunk, params);
                    chunks.insert(pos, chunk);
                }
            }
        }

        chunks
    }

    pub fn generate_chunk(&self, chunk: &mut Chunk, params: &TerrainGenParams) {
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
                    self.generate_voxel(voxel, wx, wy, wz, params);
                }
            }
        }
    }

    fn generate_voxel(&self, voxel: &mut Voxel, wx: f32, wy: f32, wz: f32, params: &TerrainGenParams) {
        let height = self.terrain_height(wx, wz, params);
        let surface_depth = height - wy; // positive = below surface

        // --- Base density: signed distance from terrain surface ---
        let density = height - wy;

        // --- Cave carving: threshold-based (does NOT modify density) ---
        let is_carved = self.is_cave(wx, wy, wz, surface_depth, params);

        if is_carved {
            // Cave interior: force air
            voxel.density = -1;
            voxel.material = MAT_AIR;
            return;
        }

        voxel.density = density.clamp(-128.0, 127.0) as i8;

        // --- Material assignment ---
        if voxel.density > 0 {
            let depth_below_surface = height - wy;
            let steepness = self.cliff_steepness(wx, wz, params);
            let mat_noise = self.material_noise.get_noise_2d(wx * 0.05, wz * 0.05);
            let cliff = params.cliff_threshold;

            voxel.material = if steepness > cliff && depth_below_surface < 6.0 {
                // Very steep cliff faces: limestone
                MAT_LIMESTONE
            } else if steepness > cliff * 0.6 && depth_below_surface < 4.0 {
                // Moderately steep exposed rock: granite
                MAT_GRANITE
            } else if wy < params.water_level + 1.5 && depth_below_surface < 2.0 {
                // Beach/shoreline: sand near water level
                MAT_SAND
            } else if wy < params.water_level + 3.0 && depth_below_surface < 3.0
                && steepness < cliff * 0.3
            {
                // Lowland near water: gravel
                MAT_GRAVEL
            } else if depth_below_surface < 1.5 && steepness < cliff * 0.5 {
                // Gentle surface: grass (1.5 units = 3 voxels thick)
                MAT_GRASS_SOIL
            } else if depth_below_surface < 1.5 && steepness >= cliff * 0.5 {
                // Moderate slopes near surface: exposed soil
                MAT_SOIL
            } else if depth_below_surface < 3.0 {
                // Subsurface: soil
                MAT_SOIL
            } else if depth_below_surface < 6.0 {
                // Transition zone: clay/soil mix
                if mat_noise > 0.3 { MAT_CLAY } else { MAT_SOIL }
            } else {
                // Deep bedrock: granite
                MAT_GRANITE
            };

            // Moisture: increases with depth
            let depth_moisture = (depth_below_surface * 10.0).min(100.0);
            voxel.moisture = depth_moisture.clamp(0.0, 255.0) as u8;

            // Flora: only on grass-covered soil near the surface
            if voxel.material == MAT_GRASS_SOIL && depth_below_surface < 1.5 {
                let flora_val = self.flora_noise.get_noise_2d(wx * 0.08, wz * 0.08);
                if flora_val > 0.3 {
                    voxel.flora_id = 1;
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