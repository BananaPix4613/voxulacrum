use bytemuck::{Pod, Zeroable};

pub const MAX_PALETTE_COLORS: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PaletteUniforms {
    pub colors_srgb: [[f32; 4]; MAX_PALETTE_COLORS],
    pub colors_lab: [[f32; 4]; MAX_PALETTE_COLORS],
    pub count: u32,
    pub enabled: u32,
    pub mode: u32,
    pub l_levels: u32,
    pub ab_levels: u32,
    pub _padding: [u32; 3],
}

impl Default for PaletteUniforms {
    fn default() -> Self {
        Self {
            colors_srgb: [[0.0; 4]; MAX_PALETTE_COLORS],
            colors_lab: [[0.0; 4]; MAX_PALETTE_COLORS],
            count: 0,
            enabled: 0,
            mode: 0,
            l_levels: 8,
            ab_levels: 8,
            _padding: [0; 3],
        }
    }
}

pub struct PalettePass {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
    pub sampler: wgpu::Sampler,
}

impl PalettePass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        source_view: &wgpu::TextureView,
        shader_source: &str,
    ) -> Self {
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("palette_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float {
                                filterable: true,
                            },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(
                            wgpu::SamplerBindingType::Filtering,
                        ),
                        count: None,
                    },
                ],
            });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("palette_point_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("palette_uniform_buffer"),
            size: std::mem::size_of::<PaletteUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = Self::create_bind_group(
            device,
            &bind_group_layout,
            &uniform_buffer,
            source_view,
            &sampler,
        );

        let pipeline = Self::create_pipeline(
            device,
            surface_format,
            &bind_group_layout,
            shader_source,
        );

        Self {
            pipeline,
            bind_group,
            bind_group_layout,
            uniform_buffer,
            sampler,
        }
    }

    fn create_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform_buffer: &wgpu::Buffer,
        source_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("palette_bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn create_pipeline(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> wgpu::RenderPipeline {
        let shader_module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("palette_shader"),
                source: wgpu::ShaderSource::Wgsl(shader_source.into()),
            });

        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("palette_pipeline_layout"),
                bind_group_layouts: &[bind_group_layout],
                push_constant_ranges: &[],
            });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("palette_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    pub fn rebuild_bind_group(
        &mut self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
    ) {
        self.bind_group = Self::create_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniform_buffer,
            source_view,
            &self.sampler,
        );
    }

    pub fn try_reload_shader(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        shader_source: &str,
    ) -> Result<(), String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let new_pipeline = Self::create_pipeline(
            device,
            surface_format,
            &self.bind_group_layout,
            shader_source,
        );

        match pollster::block_on(device.pop_error_scope()) {
            Some(err) => Err(format!("Shader compile error: {}", err)),
            None => {
                self.pipeline = new_pipeline;
                Ok(())
            }
        }
    }

    pub fn update_uniforms(&self, queue: &wgpu::Queue, uniforms: PaletteUniforms) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );
    }
}