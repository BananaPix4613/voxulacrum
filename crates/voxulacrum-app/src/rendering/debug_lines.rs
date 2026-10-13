use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};
use wgpu::util::DeviceExt;
use bevy_ecs::prelude::Resource;

use crate::rendering::render_context::RenderContext;
use crate::rendering::frustum::Frustum;
use crate::world::chunk::{CHUNK_WORLD_SIZE, VOXEL_SCALE};

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

/// 12-edge wireframe box (24 verts) spanning `[min, max]` in one color.
fn box_edges(min: Vec3, max: Vec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let corners = [
        [min.x, min.y, min.z],
        [max.x, min.y, min.z],
        [max.x, min.y, max.z],
        [min.x, min.y, max.z],
        [min.x, max.y, min.z],
        [max.x, max.y, min.z],
        [max.x, max.y, max.z],
        [min.x, max.y, max.z],
    ];
    let edges: [(usize, usize); 12] = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    let mut lines = Vec::with_capacity(24);
    for (a, b) in edges {
        lines.push(DebugLineVertex { position: corners[a], color });
        lines.push(DebugLineVertex { position: corners[b], color });
    }
    lines
}

/// Wireframe box hugging a single world voxel, slightly outset to avoid z-fighting.
pub fn voxel_box_lines(v: IVec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let s = VOXEL_SCALE;
    let eps = s * 0.03;
    let min = Vec3::new(v.x as f32 * s - eps, v.y as f32 * s - eps, v.z as f32 * s - eps);
    let max = Vec3::new(
        (v.x as f32 + 1.0) * s + eps,
        (v.y as f32 + 1.0) * s + eps,
        (v.z as f32 + 1.0) * s + eps,
    );
    box_edges(min, max, color)
}

/// Inclusive world-voxel box outline, expanded to cover `max`'s full cell.
fn region_box_lines(a: IVec3, b: IVec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let s = VOXEL_SCALE;
    // Slightly larger than the pick highlight's epsilon, so a one-voxel
    // selection reads as a distinct box rather than z-fighting with it.
    let eps = s * 0.06;
    let (lo, hi) = (a.min(b), a.max(b));
    let min = Vec3::new(lo.x as f32 * s - eps, lo.y as f32 * s - eps, lo.z as f32 * s - eps);
    let max = Vec3::new(
        (hi.x as f32 + 1.0) * s + eps,
        (hi.y as f32 + 1.0) * s + eps,
        (hi.z as f32 + 1.0) * s + eps,
    );
    box_edges(min, max, color)
}

#[derive(Resource)]
pub struct DebugLinePass {
    pub pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: Option<wgpu::Buffer>,
    pub vertex_count: u32,
    /// Optional single-voxel pick highlight, drawn independently of the chunk
    /// boundary lines (and of `show_debug_lines`).
    pub highlight_buffer: Option<wgpu::Buffer>,
    pub highlight_count: u32,
    /// Optional authoring-region outline (blueprint capture selection).
    pub selection_buffer: Option<wgpu::Buffer>,
    pub selection_count: u32,
    /// The corners the current `selection_buffer` was built from, so a frame
    /// that changes nothing rebuilds nothing. Unlike the pick highlight, which
    /// moves with the cursor, a selection changes on a click.
    selection_key: Option<(IVec3, IVec3)>,
    /// Optional free-form line overlay for debug views, in world space.
    pub overlay_buffer: Option<wgpu::Buffer>,
    pub overlay_count: u32,
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
                format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
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
            selection_key: None,
            overlay_buffer: None,
            overlay_count: 0,
        }
    }

    pub fn update(
        &mut self,
        ctx: &RenderContext,
        chunk_positions: &[IVec3],
        frustum: &Frustum,
    ) {
        let lines = generate_chunk_boundary_lines(chunk_positions, frustum);
        self.vertex_count = lines.len() as u32;

        if lines.is_empty() {
            self.vertex_buffer = None;
            return;
        }

        self.vertex_buffer = Some(ctx.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("debug_lines_vertex_buffer"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::VERTEX,
            },
        ));
    }

    /// Set (or clear) the pick highlight box around a single world voxel.
    pub fn set_highlight(&mut self, ctx: &RenderContext, anchor: Option<IVec3>, color: [f32; 3]) {
        match anchor {
            Some(v) => {
                let lines = voxel_box_lines(v, color);
                self.highlight_count = lines.len() as u32;
                self.highlight_buffer = Some(ctx.device.create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("pick_highlight_vertex_buffer"),
                        contents: bytemuck::cast_slice(&lines),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                ));
            }
            None => {
                self.highlight_buffer = None;
                self.highlight_count = 0;
            }
        }
    }

    /// Set (or clear) the authoring-region outline spanning two world voxels,
    /// inclusive. A no-op when the corners are unchanged since the last call.
    pub fn set_selection(
        &mut self,
        ctx: &RenderContext,
        corners: Option<(IVec3, IVec3)>,
        color: [f32; 3],
    ) {
        if self.selection_key == corners {
            return;
        }
        self.selection_key = corners;
        match corners {
            Some((a, b)) => {
                let lines = region_box_lines(a, b, color);
                self.selection_count = lines.len() as u32;
                self.selection_buffer = Some(ctx.device.create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("selection_region_vertex_buffer"),
                        contents: bytemuck::cast_slice(&lines),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                ));
            }
            None => {
                self.selection_buffer = None;
                self.selection_count = 0;
            }
        }
    }

    /// Set (or clear) a free-form world-space line overlay.
    ///
    /// Unlike `set_selection` this holds no key and rebuilds on every call:
    /// the geometry is a whole vertex list rather than two corners, so
    /// comparing it would cost more than rebuilding it. **The caller owns the
    /// change detection** and is expected to call only when its own inputs
    /// moved.
    pub fn set_overlay(&mut self, ctx: &RenderContext, lines: Option<&[DebugLineVertex]>) {
        match lines {
            Some(lines) if !lines.is_empty() => {
                self.overlay_count = lines.len() as u32;
                self.overlay_buffer = Some(ctx.device.create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("debug_overlay_vertex_buffer"),
                        contents: bytemuck::cast_slice(lines),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                ));
            }
            _ => {
                self.overlay_buffer = None;
                self.overlay_count = 0;
            }
        }
    }
}
