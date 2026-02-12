use wgpu::util::DeviceExt;

use crate::rendering::pipelines::{GrassInstance, GrassVertex};
use crate::world::World;
use crate::world::chunk::CHUNK_SIZE;
use crate::world::voxel::MATERIAL_TABLE;

pub struct VegetationPass {
    pub grass_vertex_buffer: wgpu::Buffer,
    pub grass_index_buffer: wgpu::Buffer,
    pub grass_index_count: u32,
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

impl VegetationPass {
    pub fn new(device: &wgpu::Device, world: &World) -> Self {
        let (vertices, indices) = create_grass_blade_mesh();

        let grass_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grass_blade_vertex_buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let grass_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("grass_blade_index_buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let instances = collect_grass_instances(world);
        let instance_count = instances.len() as u32;
        log::info!("Collected {} grass instances", instance_count);

        let instance_buffer = if instances.is_empty() {
            // Create a minimal buffer even if empty to avoid wgpu errors
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grass_instance_buffer_empty"),
                size: std::mem::size_of::<GrassInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grass_instance_buffer"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };

        Self {
            grass_vertex_buffer,
            grass_index_buffer,
            grass_index_count: indices.len() as u32,
            instance_buffer,
            instance_count,
        }
    }
}

/// Single upward triangle — reads better in isometric view than crossed quads.
/// 3 vertices, 6 indices (front + back face).
fn create_grass_blade_mesh() -> (Vec<GrassVertex>, Vec<u32>) {
    let hw = 0.12; // half-width of blade
    let h = 1.0;   // height of blade

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

/// Simple deterministic hash for position-based randomness (no rand dependency).
fn hash_position(x: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374761393);
    h = h.wrapping_add((z as u32).wrapping_mul(668265263));
    hash_u32(h)
}

fn hash_position_seed(x: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374761393);
    h = h.wrapping_add((z as u32).wrapping_mul(668265263));
    h = h.wrapping_add(seed.wrapping_mul(1911520717));
    hash_u32(h)
}

/// Convert a hash value to a float in [0.0, 1.0).
fn hash_to_float(h: u32) -> f32 {
    (h & 0x00FF_FFFF) as f32 / 16777216.0
}

fn hash_to_float_signed(h: u32) -> f32 {
    hash_to_float(h) * 2.0 - 1.0
}

fn is_air_at(world: &World, wx: i32, wy: i32, wz: i32) -> bool {
    if wy < 0 || wy >= (world.chunks_y * CHUNK_SIZE) as i32 { return true; }
    if wx < 0 || wx >= (world.chunks_x * CHUNK_SIZE) as i32 { return true; }
    if wz < 0 || wz >= (world.chunks_z * CHUNK_SIZE) as i32 { return true; }
    let cx = (wx as usize) / CHUNK_SIZE;
    let cy = (wy as usize) / CHUNK_SIZE;
    let cz = (wz as usize) / CHUNK_SIZE;
    if let Some(chunk) = world.get_chunk(cx, cy, cz) {
        let lx = (wx as usize) % CHUNK_SIZE;
        let ly = (wy as usize) % CHUNK_SIZE;
        let lz = (wz as usize) % CHUNK_SIZE;
        chunk.get_voxel(lx, ly, lz).density <= 0
    } else {
        true
    }
}

fn density_at(world: &World, wx: i32, wy: i32, wz: i32) -> i8 {
    if wx < 0 || wx >= (world.chunks_x * CHUNK_SIZE) as i32 { return 0; }
    if wy < 0 || wy >= (world.chunks_y * CHUNK_SIZE) as i32 { return 0; }
    if wz < 0 || wz >= (world.chunks_z * CHUNK_SIZE) as i32 { return 0; }
    let cx = (wx as usize) / CHUNK_SIZE;
    let cy = (wy as usize) / CHUNK_SIZE;
    let cz = (wz as usize) / CHUNK_SIZE;
    if let Some(chunk) = world.get_chunk(cx, cy, cz) {
        let lx = (wx as usize) % CHUNK_SIZE;
        let ly = (wy as usize) % CHUNK_SIZE;
        let lz = (wz as usize) % CHUNK_SIZE;
        chunk.get_voxel(lx, ly, lz).density
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

const BLADES_PER_VOXEL: u32 = 3;

/// Scan the world for flora voxels and generate grass instance data.
fn collect_grass_instances(world: &World) -> Vec<GrassInstance> {
    let mut instances = Vec::new();
    let grass_color = MATERIAL_TABLE[6].color; // MAT_GRASS_SOIL

    for chunk in &world.chunks {
        let base_x = chunk.position.x * CHUNK_SIZE as i32;
        let base_y = chunk.position.y * CHUNK_SIZE as i32;
        let base_z = chunk.position.z * CHUNK_SIZE as i32;

        for lz in 0..CHUNK_SIZE {
            for ly in 0..CHUNK_SIZE {
                for lx in 0..CHUNK_SIZE {
                    let voxel = chunk.get_voxel(lx, ly, lz);
                    if voxel.flora_id == 0 { continue; }

                    let wx = base_x + lx as i32;
                    let wy = base_y + ly as i32;
                    let wz = base_z + lz as i32;

                    // Only topmost flora voxel
                    if !is_air_at(world, wx, wy + 1, wz) { continue; }

                    // Skip steep slopes
                    if terrain_slope(world, wx, wy, wz) > 1.5 { continue; }

                    for blade_idx in 0..BLADES_PER_VOXEL {
                        let h1 = hash_position_seed(wx, wz, blade_idx * 3);
                        let h2 = hash_position_seed(wx, wz, blade_idx * 3 + 1);
                        let h3 = hash_position_seed(wx, wz, blade_idx * 3 + 2);
                        let h4 = hash_position_seed(wx, wz, blade_idx * 3 + 100);

                        let scale = 0.5 + hash_to_float(h1) * 0.7;
                        let rotation = hash_to_float(h2) * std::f32::consts::TAU;
                        let blade_phase = hash_to_float(h4) * std::f32::consts::TAU;
                        let jitter_x = hash_to_float_signed(h3) * 0.4;
                        let jitter_z = hash_to_float_signed(hash_u32(h3)) * 0.4;

                        let cv = hash_to_float(hash_u32(h1)) * 0.15 - 0.075;
                        let terrain_color = [
                            (grass_color[0] + cv).clamp(0.0, 1.0),
                            (grass_color[1] + cv).clamp(0.0, 1.0),
                            (grass_color[2] + cv * 0.5).clamp(0.0, 1.0),
                        ];

                        instances.push(GrassInstance {
                            position: [
                                wx as f32 + 0.5 + jitter_x,
                                wy as f32 + 1.0,
                                wz as f32 + 0.5 + jitter_z,
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
    }

    instances
}