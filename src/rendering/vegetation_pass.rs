use std::collections::HashMap;
use wgpu::util::DeviceExt;
use bevy_ecs::prelude::Resource;
use glam::IVec3;
use crate::rendering::render_context::RenderContext;
use crate::rendering::pipelines::{GrassInstance, GrassVertex};
use crate::params::VegetationParams;
use crate::world::World;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::voxel::MATERIAL_TABLE;

/// Per-chunk GPU vegetation data.
pub struct ChunkVegetation {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

#[derive(Resource)]
pub struct VegetationPass {
    pub grass_vertex_buffer: wgpu::Buffer,
    pub grass_index_buffer: wgpu::Buffer,
    pub grass_index_count: u32,
    /// Per-chunk vegetation GPU buffers, keyed by chunk positions.
    pub chunk_vegetation: HashMap<IVec3, ChunkVegetation>,
}

impl VegetationPass {
    /// Full rebuild — scans all loaded chunks. Used for initial load and terrain regen.
    pub fn new(
        ctx: &RenderContext,
        world: &World,
        params: &VegetationParams,
    ) -> Self {
        let (vertices, indices) = create_grass_blade_mesh(params);

        let grass_vertex_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grass_blade_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let grass_index_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grass_blade_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Build per-chunk GPU buffers
        let mut chunk_vegetation = HashMap::new();
        let mut total_instances = 0u32;
        for (&pos, _) in &world.chunks {
            let instances = collect_chunk_grass_instances(pos, world, params);
            if !instances.is_empty() {
                total_instances += instances.len() as u32;
                let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("grass_instance_buffer_chunk"),
                    contents: bytemuck::cast_slice(&instances),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                chunk_vegetation.insert(pos, ChunkVegetation {
                    instance_buffer: buffer,
                    instance_count: instances.len() as u32,
                });
            }
        }

        log::info!(
            "Collected {} grass instances across {} chunks",
            total_instances, chunk_vegetation.len()
        );

        Self {
            grass_vertex_buffer,
            grass_index_buffer,
            grass_index_count: indices.len() as u32,
            chunk_vegetation,
        }
    }

    /// Add vegetation instances for a single chunk. Creates a per-chunk GPU buffer.
    pub fn add_chunk_vegetation(
        &mut self,
        pos: IVec3,
        world: &World,
        params: &VegetationParams,
        device: &wgpu::Device,
    ) {
        let instances = collect_chunk_grass_instances(pos, world, params);
        if !instances.is_empty() {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grass_instance_buffer_chunk"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.chunk_vegetation.insert(pos, ChunkVegetation {
                instance_buffer: buffer,
                instance_count: instances.len() as u32,
            });
        } else {
            // Remove stale entry if chunk had vegetation before but doesn't now
            self.chunk_vegetation.remove(&pos);
        }
    }

    /// Remove vegetation instances for an unloaded chunk. GPU buffer dropped.
    pub fn remove_chunk_vegetation(&mut self, pos: IVec3) {
        self.chunk_vegetation.remove(&pos);
    }

    /// Clear all per-chunk vegetation (used during regen).
    pub fn clear_all(&mut self) {
        self.chunk_vegetation.clear();
    }
}

/// Grass blade sized for 0.5m voxels.
fn create_grass_blade_mesh(params: &VegetationParams) -> (Vec<GrassVertex>, Vec<u32>) {
    let hw = params.blade_half_width;
    let h = params.blade_height;

    let vertices = vec![
        GrassVertex { position: [-hw, 0.0, 0.0], uv: [0.0, 0.0], _pad: 0.0 },
        GrassVertex { position: [ hw, 0.0, 0.0], uv: [1.0, 0.0], _pad: 0.0 },
        GrassVertex { position: [0.0,   h, 0.0], uv: [0.5, 1.0], _pad: 0.0 },
    ];

    let indices: Vec<u32> = vec![
        0, 1, 2, // Front
        2, 1, 0, // Back
    ];

    (vertices, indices)
}

fn hash_u32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45d9f3b);
    h ^= h >> 16;
    h
}

fn hash_position_seed(x: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374761393);
    h = h.wrapping_add((z as u32).wrapping_mul(668265263));
    h = h.wrapping_add(seed.wrapping_mul(1911520717));
    hash_u32(h)
}

fn hash_to_float(h: u32) -> f32 {
    (h & 0x00FF_FFFF) as f32 / 16777216.0
}

fn hash_to_float_signed(h: u32) -> f32 {
    hash_to_float(h) * 2.0 - 1.0
}

fn is_air_at(world: &World, wx: i32, wy: i32, wz: i32) -> bool {
    let cx = wx.div_euclid(CHUNK_SIZE as i32);
    let cy = wy.div_euclid(CHUNK_SIZE as i32);
    let cz = wz.div_euclid(CHUNK_SIZE as i32);
    if let Some(chunk) = world.get_chunk(IVec3::new(cx, cy, cz)) {
        let lx = wx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let ly = wy.rem_euclid(CHUNK_SIZE as i32) as usize;
        let lz = wz.rem_euclid(CHUNK_SIZE as i32) as usize;
        chunk.density(lx, ly, lz) <= 0
    } else {
        true
    }
}

fn density_at(world: &World, wx: i32, wy: i32, wz: i32) -> i8 {
    let cx = wx.div_euclid(CHUNK_SIZE as i32);
    let cy = wy.div_euclid(CHUNK_SIZE as i32);
    let cz = wz.div_euclid(CHUNK_SIZE as i32);
    if let Some(chunk) = world.get_chunk(IVec3::new(cx, cy, cz)) {
        let lx = wx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let ly = wy.rem_euclid(CHUNK_SIZE as i32) as usize;
        let lz = wz.rem_euclid(CHUNK_SIZE as i32) as usize;
        chunk.density(lx, ly, lz)
    } else {
        0
    }
}

fn terrain_slope(world: &World, wx: i32, wy: i32, wz: i32) -> f32 {
    let dx = density_at(world, wx + 1, wy, wz) as f32 - density_at(world, wx - 1, wy, wz) as f32;
    let dz = density_at(world, wx, wy, wz + 1) as f32 - density_at(world, wx, wy, wz - 1) as f32;
    let dy = density_at(world, wx, wy + 1, wz) as f32 - density_at(world, wx, wy - 1, wz) as f32;
    if dy.abs() < 1.0 { return 10.0; }
    (dx * dx + dz * dz).sqrt() / dy.abs()
}

/// Generate grass instances for a single chunk. Scans only that chunk's voxels
/// but reads neighbor chunks for air/slope checks at boundaries.
fn collect_chunk_grass_instances(
    chunk_pos: IVec3,
    world: &World,
    params: &VegetationParams,
) -> Vec<GrassInstance> {
    let chunk = match world.chunks.get(&chunk_pos) {
        Some(c) => c,
        None => return Vec::new(),
    };

    let grass_color = MATERIAL_TABLE[6].color; // MAT_GRASS_SOIL
    let mut instances = Vec::new();

    let base_x = chunk.position.x * CHUNK_SIZE as i32;
    let base_y = chunk.position.y * CHUNK_SIZE as i32;
    let base_z = chunk.position.z * CHUNK_SIZE as i32;

    for lz in 0..CHUNK_SIZE {
        for ly in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                // TODO: flora_id dropped from storage (Phase 3). Reconstruct
                // eligibility from material type — matches generation logic.
                let mat = chunk.material(lx, ly, lz);
                if mat != crate::world::voxel::MAT_GRASS_SOIL { continue; }
                // Only surface voxels (density > 0 but thin layer) qualify
                let d = chunk.density(lx, ly, lz);
                if d <= 0 { continue; }

                let wx = base_x + lx as i32;
                let wy = base_y + ly as i32;
                let wz = base_z + lz as i32;

                if !is_air_at(world, wx, wy + 1, wz) { continue; }
                if terrain_slope(world, wx, wy, wz) > 1.5 { continue; }

                for blade_idx in 0..params.blades_per_voxel {
                    let h1 = hash_position_seed(wx, wz, blade_idx * 3);
                    let h2 = hash_position_seed(wx, wz, blade_idx * 3 + 1);
                    let h3 = hash_position_seed(wx, wz, blade_idx * 3 + 2);
                    let h4 = hash_position_seed(wx, wz, blade_idx * 3 + 100);

                    let scale = 0.3 + hash_to_float(h1) * 0.5;
                    let rotation = hash_to_float(h2) * std::f32::consts::TAU;
                    let blade_phase = hash_to_float(h4) * std::f32::consts::TAU;
                    let jitter_x = hash_to_float_signed(h3) * 0.2;
                    let jitter_z = hash_to_float_signed(hash_u32(h3)) * 0.2;

                    let cv = hash_to_float(hash_u32(h1)) * 0.15 - 0.075;
                    let terrain_color = [
                        (grass_color[0] + cv).clamp(0.0, 1.0),
                        (grass_color[1] + cv).clamp(0.0, 1.0),
                        (grass_color[2] + cv * 0.5).clamp(0.0, 1.0),
                    ];

                    instances.push(GrassInstance {
                        position: [
                            wx as f32 * VOXEL_SCALE + VOXEL_SCALE * 0.5 + jitter_x,
                            wy as f32 * VOXEL_SCALE + VOXEL_SCALE,
                            wz as f32 * VOXEL_SCALE + VOXEL_SCALE * 0.5 + jitter_z,
                        ],
                        scale,
                        rotation,
                        blade_phase,
                        terrain_color,
                        _pad1: 0.0,
                    });
                }
            }
        }
    }

    instances
}
