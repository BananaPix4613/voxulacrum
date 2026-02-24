use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};
use wgpu::util::DeviceExt;

use crate::rendering::frustum::Frustum;
use crate::world::chunk::CHUNK_WORLD_SIZE;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DebugLineVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

impl DebugLineVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<DebugLineVertex>() as wgpu::BufferAddress,
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
            ],
        }
    }
}

/// Generates 12 line edges per chunk AABB.
/// Visible chunks are green, culled chunks are red.
pub fn generate_chunk_boundary_lines(
    chunk_positions: &[IVec3],
    frustum: &Frustum,
) -> Vec<DebugLineVertex> {
    let mut lines = Vec::with_capacity(chunk_positions.len() * 24); // 12 edges * 2 verts

    for &pos in chunk_positions {
        let min = Vec3::new(
            pos.x as f32 * CHUNK_WORLD_SIZE,
            pos.y as f32 * CHUNK_WORLD_SIZE,
            pos.z as f32 * CHUNK_WORLD_SIZE,
        );
        let max = min + Vec3::splat(CHUNK_WORLD_SIZE);
        let visible = frustum.intersects_aabb(min, max);
        let color: [f32; 3] = if visible {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };

        let corners = [
            [min.x, min.y, min.z], // 0
            [max.x, min.y, min.z], // 1
            [max.x, min.y, max.z], // 2
            [min.x, min.y, max.z], // 3
            [min.x, max.y, min.z], // 4
            [max.x, max.y, min.z], // 5
            [max.x, max.y, max.z], // 6
            [min.x, max.y, max.z], // 7
        ];

        let edges: [(usize, usize); 12] = [
            (0, 1), (1, 2), (2, 3), (3, 0), // bottom
            (4, 5), (5, 6), (6, 7), (7, 4), // top
            (0, 4), (1, 5), (2, 6), (3, 7), // verticals
        ];

        for (a, b) in edges {
            lines.push(DebugLineVertex { position: corners[a], color });
            lines.push(DebugLineVertex { position: corners[b], color });
        }
    }

    lines
}

pub struct DebugLinePass {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub vertex_count: u32,
}

impl DebugLinePass {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> Self {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug_lines_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug_lines_pipeline_layout"),
            bind_group_layouts: &[global_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug_lines_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[DebugLineVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
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
        });

        Self {
            pipeline,
            vertex_buffer: None,
            vertex_count: 0,
        }
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        chunk_positions: &[IVec3],
        frustum: &Frustum,
    ) {
        let lines = generate_chunk_boundary_lines(chunk_positions, frustum);
        self.vertex_count = lines.len() as u32;

        if lines.is_empty() {
            self.vertex_buffer = None;
            return;
        }

        self.vertex_buffer = Some(device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("debug_lines_vertex_buffer"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::VERTEX,
            },
        ));
    }
}