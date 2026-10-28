use bytemuck::{Pod, Zeroable};
use bevy_ecs::prelude::Resource;

use crate::rendering::cap_pass::CapVertex;
use crate::rendering::render_context::RenderContext;
use crate::rendering::versioned::{Uploaded, Versioned};

/// Upper bound for the avatar mesh (capsule + visor + shadow disc = 180 verts / 860 indices).
const MAX_VERTICES: usize = 512;
const MAX_INDICES: usize = 1024;

/// The player avatar + drop shadow. A small mesh rebuilt each frame at the
/// interpolated render position, drawn in the main scene pass between foliage and
/// water. Reuses `CapVertex` (position, normal, color) and the global bind group.
#[derive(Resource)]
pub struct PlayerPass {
    pub pipeline: wgpu::RenderPipeline,
    pub silhouette_mark_pipeline: wgpu::RenderPipeline,
    pub silhouette_pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub body_index_count: u32,
    pub visible: bool,
    uploaded: Uploaded,
}

/// One frame's avatar mesh, built on the main side (`meshing::player`).
pub struct PlayerMeshData {
    pub vertices: Vec<CapVertex>,
    pub indices: Vec<u32>,
    /// Indices of the body alone (capsule + visor); the drop shadow follows them.
    pub body_index_count: u32,
}

impl PlayerPass {
    pub fn new(
        ctx: &RenderContext,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> Self {
        let shader_module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("player_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("player_pipeline_layout"),
            bind_group_layouts: &[global_bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("player_pipeline"),
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
                        format: ctx.surface_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL
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
                format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        let silhouette_pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("player_silhouette_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[CapVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_silhouette"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: ctx.surface_format,
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
                format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always, // ignore depth: draw over

                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Equal,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Equal,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Keep,
                    },
                    read_mask: 0xFF,
                    write_mask: 0x00, // read-only
                },
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        let silhouette_mark_pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("player_silhouette_mark_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[CapVertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_silhouette"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: ctx.surface_format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::empty(),
                    }),
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::empty(),
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
                format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Greater, // only where behind terrain
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Replace,
                    },
                    back: wgpu::StencilFaceState {
                        compare: wgpu::CompareFunction::Always,
                        fail_op: wgpu::StencilOperation::Keep,
                        depth_fail_op: wgpu::StencilOperation::Keep,
                        pass_op: wgpu::StencilOperation::Replace,
                    },
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                },
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        let vertex_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player_vertex_buffer"),
            size: (MAX_VERTICES * std::mem::size_of::<CapVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player_index_buffer"),
            size: (MAX_INDICES * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            silhouette_mark_pipeline,
            silhouette_pipeline,
            vertex_buffer,
            index_buffer,
            index_count: 0,
            body_index_count: 0,
            visible: false,
            uploaded: Uploaded::default(),
        }
    }

    /// Upload the avatar mesh when it moved since the last call; `None` hides
    /// the avatar.
    pub fn upload(&mut self, queue: &wgpu::Queue, source: &Versioned<Option<PlayerMeshData>>) {
        let Some(mesh) = self.uploaded.changed(source) else { return };
        let Some(mesh) = mesh else {
            self.visible = false;
            return;
        };
        debug_assert!(mesh.vertices.len() <= MAX_VERTICES && mesh.indices.len() <= MAX_INDICES);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&mesh.vertices));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&mesh.indices));
        self.index_count = mesh.indices.len() as u32;
        self.body_index_count = mesh.body_index_count;
        self.visible = true;
    }
}

// CapVertex must be Pod for write_buffer; it already derives Pod/Zeroable in cap_pass.
const _: fn() = || {
    fn _assert_pod<T: Pod + Zeroable>() {}
    _assert_pod::<CapVertex>();
};
