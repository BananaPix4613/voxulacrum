use bevy_ecs::prelude::Resource;

use crate::params::{WaterVisualParams, TerrainGenParams};
use crate::world::World;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};

const WORLD_WIDTH: usize = 8 * CHUNK_SIZE;
const WORLD_DEPTH: usize = 8 * CHUNK_SIZE;

#[derive(Resource)]
pub struct StaticWater {
    terrain_heights: Vec<f32>,
    water_level_value: f32,
    pub width: usize,
    pub depth: usize,
}

impl StaticWater {
    pub fn new(world: &World, water_params: &WaterVisualParams, terrain_params: &TerrainGenParams) -> Self {
        let width = WORLD_WIDTH;
        let depth = WORLD_DEPTH;
        let total = width * depth;
        let gen = &world.generator;
        let water_level = water_params.water_level;

        let mut terrain_heights = vec![0.0f32; total];
        let mut water_cells = 0u32;

        for z in 0..depth {
            for x in 0..width {
                let wx = x as f32 * VOXEL_SCALE;
                let wz = z as f32 * VOXEL_SCALE;
                let height = gen.terrain_height(wx, wz, terrain_params);
                terrain_heights[z * width + x] = height;
                if height < water_level {
                    water_cells += 1;
                }
            }
        }

        log::info!("Static water: {} cells below water level {}", water_cells, water_level);

        Self { terrain_heights, water_level_value: water_level, width, depth }
    }

    pub fn water_level(&self) -> f32 { self.water_level_value }

    pub fn has_water(&self, x: usize, z: usize) -> bool {
        self.terrain_heights[z * self.width + x] < self.water_level_value
    }

    pub fn terrain_height(&self, x: usize, z: usize) -> f32 {
        self.terrain_heights[z * self.width + x]
    }

    pub fn water_depth(&self, x: usize, z: usize) -> f32 {
        (self.water_level_value - self.terrain_heights[z * self.width + x]).max(0.0)
    }
}