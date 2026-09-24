//! Canopy lobe instance buffers (spec §6.2).
//! 
//! A direct sibling of `capsule_pass`: each chunk owns the lobes centered in
//! it, turned into one instance buffer, rebuilt on (re)mesh and dropped on
//! unload. Separate from the capsules only because the primitive differs - an
//! ellipsoid and a round cone need different solves, so they cannot share a
//! draw.

use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::LobeInstanceGpu;
use crate::rendering::render_context::RenderContext;
use crate::world::chunk::{LoadedChunk, VOXEL_SCALE};
use crate::world::World;

/// One chunk's lobe instances.
pub struct ChunkLobes {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

#[derive(Resource, Default)]
pub struct CanopyPass {
    pub chunks: HashMap<IVec3, ChunkLobes>,
}

impl CanopyPass {
    /// Full rebuild - scans every loaded chunk. Initial load and regen.
    pub fn new(ctx: &RenderContext, world: &World) -> Self {
        let mut pass = Self::default();
        for &coord in world.chunks.keys() {
            let pos = IVec3::from(coord);
            pass.add_chunk(pos, world, &ctx.device);
        }
        log::info!("Canopy: built lobe instances for {} chunks", pass.chunks.len());
        pass
    }
    
    /// Build or refresh one chunk's instance buffer.
    pub fn add_chunk(&mut self, pos: IVec3, world: &World, device: &wgpu::Device) {
        let Some(chunk) = world.get_chunk(pos) else { return };
        match build(chunk, device) {
            Some(c) => {
                self.chunks.insert(pos, c);
            }
            None => {
                self.chunks.remove(&pos);
            }
        }
    }
    
    /// Drop a chunk's instances on unload.
    pub fn remove_chunk(&mut self, pos: IVec3) {
        self.chunks.remove(&pos);
    }
}

/// `None` when the chunk owns no lobes, so a treeless chunk costs no buffer.
fn build(chunk: &LoadedChunk, device: &wgpu::Device) -> Option<ChunkLobes> {
    if chunk.lobes.is_empty() {
        return None;
    }
    let instances: Vec<LobeInstanceGpu> = chunk
        .lobes
        .iter()
        .map(|l| LobeInstanceGpu {
            // Lobes are in voxel units, as segments are; the render world
            // scales by VOXEL_SCALE. Radii scale with position because the
            // scale is uniform.
            center: (l.center * VOXEL_SCALE).to_array(),
            radii: (l.radii * VOXEL_SCALE).to_array(),
        })
        .collect();
    
    let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("canopy_lobe_instance_buffer_chunk"),
        contents: bytemuck::cast_slice(&instances),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some(ChunkLobes { instance_buffer, instance_count: instances.len() as u32 })
}
