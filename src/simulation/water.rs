use crate::world::World;
use crate::world::chunk::CHUNK_SIZE;

const WORLD_WIDTH: usize = 8 * CHUNK_SIZE;
const WORLD_DEPTH: usize = 8 * CHUNK_SIZE;
const RIVER_WIDTH: f32 = 5.0;
const WATER_LEVEL: f32 = 60.0;

pub struct StaticWater {
    terrain_heights: Vec<f32>,
    pub width: usize,
    pub depth: usize,
}

impl StaticWater {
    pub fn new(world: &World) -> Self {
        let width = WORLD_WIDTH;
        let depth = WORLD_DEPTH;
        let total = width * depth;
        let gen = &world.generator;

        let mut terrain_heights = vec![0.0f32; total];
        let mut water_cells = 0u32;

        for z in 0..depth {
            for x in 0..width {
                let wx = x as f32;
                let wz = z as f32;
                let mut height = gen.terrain_height(wx, wz);
                let river_dist = gen.distance_to_river(wx, wz);
                let channel_depth = (RIVER_WIDTH - river_dist).max(0.0) * 1.5;
                height -= channel_depth;
                terrain_heights[z * width + x] = height;
                if height < WATER_LEVEL {
                    water_cells += 1;
                }
            }
        }

        log::info!("Static water: {} cells below water level {}", water_cells, WATER_LEVEL);

        Self { terrain_heights, width, depth }
    }

    pub fn water_level(&self) -> f32 { WATER_LEVEL }

    pub fn has_water(&self, x: usize, z: usize) -> bool {
        self.terrain_heights[z * self.width + x] < WATER_LEVEL
    }

    pub fn terrain_height(&self, x: usize, z: usize) -> f32 {
        self.terrain_heights[z * self.width + x]
    }

    pub fn water_depth(&self, x: usize, z: usize) -> f32 {
        let ground = self.terrain_heights[z * self.width + x];
        (WATER_LEVEL - ground).max(0.0)
    }
}