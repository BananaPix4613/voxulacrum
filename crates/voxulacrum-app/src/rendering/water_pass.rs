use std::collections::HashMap;

use bevy_ecs::prelude::Resource;
use glam::IVec3;
use wgpu::util::DeviceExt;

use crate::rendering::pipelines::WaterVertex;
use crate::world::changes::ChunkVersion;

/// One chunk's water surface, built on the CPU (`meshing::water`).
pub struct WaterSurface {
    pub vertices: Vec<WaterVertex>,
    pub indices: Vec<u32>,
    /// Mean world-space height of the chunk's surface columns. The reflection
    /// plane is the mean of these across chunks.
    pub surface_y: f32,
}

/// Per-chunk water surface mesh on GPU.
pub struct ChunkWaterMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    /// The chunk version this mesh was built from.
    pub version: ChunkVersion,
}

/// Water surface pass: holds one surface mesh per chunk that has visible water.
///
/// Geometry is built from a chunk's `FluidLayer` by `meshing::water` when the
/// chunk meshes or its fluid changes; the pipeline + shader (`water.wgsl`)
/// handle transparency, depth shading, and waves.
#[derive(Resource)]
pub struct WaterPass {
    /// Per-chunk water meshes, keyed by chunk position.
    pub chunk_meshes: HashMap<IVec3, ChunkWaterMesh>,
    chunk_surface: HashMap<IVec3, f32>,
    pub avg_surface_y: f32,
    pub uniform_buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
}

impl WaterPass {
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene_copy_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        reflection_color_view: &wgpu::TextureView,
        reflection_depth_view: &wgpu::TextureView,
    ) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water_uniform_buffer"),
            size: std::mem::size_of::<crate::rendering::uniforms::WaterUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("water_scene_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = crate::rendering::uniforms::create_water_bind_group(
            device, layout, &uniform_buffer, scene_copy_view, &sampler, scene_depth_view,
            reflection_color_view, reflection_depth_view,
        );
        Self {
            chunk_meshes: HashMap::new(),
            chunk_surface: HashMap::new(),
            avg_surface_y: 0.0,
            uniform_buffer,
            bind_group,
            sampler,
        }
    }

    fn recompute_avg(&mut self) {
        if self.chunk_surface.is_empty() {
            return; // keep the last known plane so a momentary empty set doesn't reset it
        }
        let sum: f32 = self.chunk_surface.values().sum();
        self.avg_surface_y = sum / self.chunk_surface.len() as f32;
    }

    /// Rebuild the bind group after render targets are recreated (resize).
    pub fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        scene_copy_view: &wgpu::TextureView,
        scene_depth_view: &wgpu::TextureView,
        reflection_color_view: &wgpu::TextureView,
        reflection_depth_view: &wgpu::TextureView,
    ) {
        self.bind_group = crate::rendering::uniforms::create_water_bind_group(
            device, layout, &self.uniform_buffer, scene_copy_view, &self.sampler, scene_depth_view,
            reflection_color_view, reflection_depth_view,
        );
    }

    /// Upload per-frame water uniforms.
    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        u: &crate::rendering::uniforms::WaterUniforms,
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(std::slice::from_ref(u)));
    }

    /// Replace the water mesh for `pos` with `surface`, built from the chunk at
    /// `version`, or drop it when the chunk has no visible water surface (`None`).
    pub fn set(
        &mut self,
        pos: IVec3,
        version: ChunkVersion,
        surface: Option<&WaterSurface>,
        device: &wgpu::Device,
    ) {
        debug_assert!(
            self.chunk_meshes.get(&pos).map_or(true, |old| old.version <= version),
            "water for chunk {pos} was built from an older version than the one it replaces",
        );
        let Some(surface) = surface else {
            self.remove(pos);
            return;
        };
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water_vertex_buffer"),
            contents: bytemuck::cast_slice(&surface.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water_index_buffer"),
            contents: bytemuck::cast_slice(&surface.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.chunk_meshes.insert(pos, ChunkWaterMesh {
            vertex_buffer,
            index_buffer,
            index_count: surface.indices.len() as u32,
            version,
        });
        self.chunk_surface.insert(pos, surface.surface_y);
        self.recompute_avg();
    }

    /// Drop the water mesh of an unloaded chunk. GPU buffers are freed on drop.
    pub fn remove(&mut self, pos: IVec3) {
        self.chunk_meshes.remove(&pos);
        self.chunk_surface.remove(&pos);
        self.recompute_avg();
    }
}
