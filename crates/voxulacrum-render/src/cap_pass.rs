use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use bevy_ecs::prelude::Resource;
use crate::render_context::RenderContext;
use crate::versioned::{Uploaded, Versioned};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CapVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

impl CapVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CapVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

/// The cross-section cap mesh, built on the main side (`meshing::cap`).
#[derive(Default)]
pub struct CapMeshData {
    pub vertices: Vec<CapVertex>,
    pub indices: Vec<u32>,
}

#[derive(Resource)]
pub struct CapPass {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub index_buffer: Option<wgpu::Buffer>,
    pub index_count: u32,
    uploaded: Uploaded,
}

impl CapPass {
    pub fn new(
        ctx: &RenderContext,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> Self {
        let pipeline = Self::create_pipeline(&ctx.device, ctx.surface_format, global_bind_group_layout, shader_source);
        Self {
            pipeline,
            vertex_buffer: None,
            index_buffer: None,
            index_count: 0,
            uploaded: Uploaded::default(),
        }
    }

    fn create_pipeline(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> wgpu::RenderPipeline {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cap_pipeline_layout"),
            bind_group_layouts: &[global_bind_group_layout],
            push_constant_ranges: &[],
        });

        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cap_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[CapVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::render_targets::SCENE_DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    }

    /// Try to hot-reload the cap shader.
    pub fn try_reload_shader(
        &mut self,
        ctx: &RenderContext,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        source: &str,
    ) -> Result<(), String> {
        // Attempt to create module first to catch compile errors
        let _test_module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cap_shader_test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        self.pipeline = Self::create_pipeline(&ctx.device, ctx.surface_format, global_bind_group_layout, source);
        Ok(())
    }

    /// Upload the cap mesh when it moved since the last call. An empty mesh
    /// drops the buffers.
    pub fn upload(&mut self, device: &wgpu::Device, source: &Versioned<CapMeshData>) {
        let Some(mesh) = self.uploaded.changed(source) else { return };
        self.index_count = mesh.indices.len() as u32;

        if mesh.vertices.is_empty() {
            self.vertex_buffer = None;
            self.index_buffer = None;
        } else {
            self.vertex_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("cap_vertex_buffer"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
            self.index_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("cap_index_buffer"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
            ));
        }
    }
}
