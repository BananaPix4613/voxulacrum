use crate::rendering::pipelines;
use crate::rendering::uniforms::{self, PostProcessUniforms};

pub struct PostProcessPass {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    pub uniform_buffer: wgpu::Buffer,
    pub sampler: wgpu::Sampler,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl PostProcessPass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        scene_view: &wgpu::TextureView,
    ) -> Self {
        let bind_group_layout = uniforms::create_post_process_bind_group_layout(device);

        let pipeline = pipelines::create_post_process_pipeline(
            device,
            surface_format,
            &bind_group_layout,
        );

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post_process_uniform_buffer"),
            size: std::mem::size_of::<PostProcessUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post_process_scene_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = uniforms::create_post_process_bind_group(
            device,
            &bind_group_layout,
            &uniform_buffer,
            scene_view,
            &sampler,
        );

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            sampler,
            bind_group_layout,
        }
    }

    /// Recreate bind group when scene texture changes (e.g. on resize).
    pub fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        scene_view: &wgpu::TextureView,
    ) {
        self.bind_group = uniforms::create_post_process_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniform_buffer,
            scene_view,
            &self.sampler,
        );
    }

    pub fn update_uniforms(&self, queue: &wgpu::Queue, uniforms: PostProcessUniforms) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
    }
}