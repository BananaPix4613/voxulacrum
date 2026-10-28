//! The debug-line pipeline and its GPU line lists. The lists are built on the
//! main side (`meshing::overlay`) and uploaded here when they move.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use bevy_ecs::prelude::Resource;

use crate::render_context::RenderContext;
use crate::versioned::{Uploaded, Versioned};

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

#[derive(Resource)]
pub struct DebugLinePass {
    pub pipeline: wgpu::RenderPipeline,
    /// Chunk boundary lines.
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub vertex_count: u32,
    /// Optional single-voxel pick highlight, drawn independently of the chunk
    /// boundary lines (and of `show_debug_lines`).
    pub highlight_buffer: Option<wgpu::Buffer>,
    pub highlight_count: u32,
    /// Optional authoring-region outline (blueprint capture selection).
    pub selection_buffer: Option<wgpu::Buffer>,
    pub selection_count: u32,
    /// Optional free-form line overlay for debug views, in world space.
    pub overlay_buffer: Option<wgpu::Buffer>,
    pub overlay_count: u32,
    boundaries_uploaded: Uploaded,
    highlight_uploaded: Uploaded,
    selection_uploaded: Uploaded,
    overlay_uploaded: Uploaded,
}

impl DebugLinePass {
    pub fn new(
        ctx: &RenderContext,
        global_bind_group_layout: &wgpu::BindGroupLayout,
        shader_source: &str,
    ) -> Self {
        let shader_module = ctx.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug_lines_shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug_lines_pipeline_layout"),
            bind_group_layouts: &[global_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = ctx.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                        format: ctx.surface_format,
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
                format: crate::render_targets::SCENE_DEPTH_FORMAT,
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
            highlight_buffer: None,
            highlight_count: 0,
            selection_buffer: None,
            selection_count: 0,
            overlay_buffer: None,
            overlay_count: 0,
            boundaries_uploaded: Uploaded::default(),
            highlight_uploaded: Uploaded::default(),
            selection_uploaded: Uploaded::default(),
            overlay_uploaded: Uploaded::default(),
        }
    }

    /// Upload whichever line lists moved since the last call. An empty list
    /// drops its buffer.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        boundaries: &Versioned<Vec<DebugLineVertex>>,
        highlight: &Versioned<Vec<DebugLineVertex>>,
        selection: &Versioned<Vec<DebugLineVertex>>,
        overlay: &Versioned<Vec<DebugLineVertex>>,
    ) {
        if let Some(lines) = self.boundaries_uploaded.changed(boundaries) {
            (self.vertex_buffer, self.vertex_count) =
                line_buffer(device, "debug_lines_vertex_buffer", lines);
        }
        if let Some(lines) = self.highlight_uploaded.changed(highlight) {
            (self.highlight_buffer, self.highlight_count) =
                line_buffer(device, "pick_highlight_vertex_buffer", lines);
        }
        if let Some(lines) = self.selection_uploaded.changed(selection) {
            (self.selection_buffer, self.selection_count) =
                line_buffer(device, "selection_region_vertex_buffer", lines);
        }
        if let Some(lines) = self.overlay_uploaded.changed(overlay) {
            (self.overlay_buffer, self.overlay_count) =
                line_buffer(device, "debug_overlay_vertex_buffer", lines);
        }
    }
}

/// A vertex buffer holding `lines`, with its vertex count; no buffer when empty.
fn line_buffer(
    device: &wgpu::Device,
    label: &str,
    lines: &[DebugLineVertex],
) -> (Option<wgpu::Buffer>, u32) {
    if lines.is_empty() {
        return (None, 0);
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(lines),
        usage: wgpu::BufferUsages::VERTEX,
    });
    (Some(buffer), lines.len() as u32)
}
