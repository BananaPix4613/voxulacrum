use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use bevy_ecs::prelude::Resource;

use crate::rendering::cap_pass::CapVertex;
use crate::rendering::render_context::RenderContext;

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

        Self { pipeline, silhouette_mark_pipeline, silhouette_pipeline, vertex_buffer, index_buffer, index_count: 0, body_index_count: 0, visible: false }
    }

    /// Upload a freshly built avatar mesh.
    pub fn update(&mut self, queue: &wgpu::Queue, verts: &[CapVertex], indices: &[u32], body_index_count: u32) {
        debug_assert!(verts.len() <= MAX_VERTICES && indices.len() <= MAX_INDICES);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(verts));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(indices));
        self.index_count = indices.len() as u32;
        self.body_index_count = body_index_count;
    }
}

/// Build the avatar mesh: a capsule (the standard prototype body), a facing
/// visor on the front, and a circular hard drop shadow at `ground_y` (the first
/// solid surface below the player) when present.
pub fn build_player_mesh(feet: Vec3, facing: f32, ground_y: Option<f32>) -> (Vec<CapVertex>, Vec<u32>, u32) {
    let mut verts: Vec<CapVertex> = Vec::with_capacity(256);
    let mut indices: Vec<u32> = Vec::with_capacity(1024);

    let radius = 0.3;
    let total_height = 1.5;
    let cyl_height = total_height - 2.0 * radius; // 0.9
    build_capsule(&mut verts, &mut indices, feet, radius, cyl_height, 12, 5, [0.30, 0.55, 0.75]);

    // Facing visor ("eyes") on the front, oriented to the look direction.
    let facing_dir = Vec2::new(facing.cos(), facing.sin());
    let eye_center = feet + Vec3::new(
        facing_dir.x * radius * 0.96,
        total_height * 0.72,
        facing_dir.y * radius * 0.96,
    );
    push_visor(&mut verts, &mut indices, eye_center, facing_dir, 0.40, 0.18, [0.05, 0.05, 0.08]);

    // Index count for the body alone (capsule + visor). The drop shadow is appended
    // after this so the silhouette pass can draw the body without it.
    let body_index_count = indices.len() as u32;
    if let Some(gy) = ground_y {
        push_disc(&mut verts, &mut indices, feet.x, gy + 0.02, feet.z, 0.42, 20, [0.04, 0.04, 0.05]);
    }

    (verts, indices, body_index_count)
}

/// A capsule (two hemispheres joined by a cylinder) whose base (feet) is at
/// `base`, spanning `base.y .. base.y + 2*radius + cyl_height`. Smooth normals.
fn build_capsule(
    verts: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
    base: Vec3,
    radius: f32,
    cyl_height: f32,
    segments: usize,
    rings: usize,
    color: [f32; 3],
) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let bottom_c = base.y + radius;
    let top_c = base.y + radius + cyl_height;

    // Ring latitudes: bottom hemisphere (-π/2..0), then top hemisphere (0..π/2).
    // The two φ=0 equator rings sit at bottom_c and top_c -> the cylinder wall.
    let mut ring_lat: Vec<(f32, f32)> = Vec::with_capacity(2 * (rings + 1)); // (y, ring_radius)
    let mut ring_ny: Vec<f32> = Vec::with_capacity(2 * (rings + 1));
    for i in 0..=rings {
        let phi = -FRAC_PI_2 + (i as f32 / rings as f32) * FRAC_PI_2;
        ring_lat.push((bottom_c + radius * phi.sin(), radius * phi.cos()));
        ring_ny.push(phi.sin());
    }
    for i in 0..=rings {
        let phi = (i as f32 / rings as f32) * FRAC_PI_2;
        ring_lat.push((top_c + radius * phi.sin(), radius * phi.cos()));
        ring_ny.push(phi.sin());
    }

    let start = verts.len() as u32;
    let stride = (segments + 1) as u32;
    for (ri, &(y, rr)) in ring_lat.iter().enumerate() {
        let ny = ring_ny[ri];
        let horiz = (1.0 - ny * ny).max(0.0).sqrt(); // |cos φ| for the normal's xz scale
        for j in 0..=segments {
            let theta = j as f32 / segments as f32 * TAU;
            let (st, ct) = theta.sin_cos();
            verts.push(CapVertex {
                position: [base.x + rr * ct, y, base.z + rr * st],
                normal: [horiz * ct, ny, horiz * st],
                color,
            });
        }
    }
    let ring_count = ring_lat.len() as u32;
    for i in 0..ring_count - 1 {
        for j in 0..segments as u32 {
            let a = start + i * stride + j;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            indices.extend(&[a, c, b, b, c, d]);
        }
    }
}

/// A small quad facing outward along `facing_dir` (the player's eyes/visor).
fn push_visor(verts: &mut Vec<CapVertex>, indices: &mut Vec<u32>, center: Vec3, facing_dir: Vec2, width: f32, height: f32, color: [f32; 3]) {
    let right = Vec3::new(-facing_dir.y, 0.0, facing_dir.x) * (width * 0.5);
    let up = Vec3::new(0.0, height * 0.5, 0.0);
    let n = [facing_dir.x, 0.0, facing_dir.y];
    let base = verts.len() as u32;
    for corner in [center - right - up, center - right + up, center + right + up, center + right - up] {
        verts.push(CapVertex { position: corner.to_array(), normal: n, color });
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A horizontal filled disc (triangle fan) - the circular drop shadow.
fn push_disc(verts: &mut Vec<CapVertex>, indices: &mut Vec<u32>, cx: f32, y: f32, cz: f32, r: f32, segments: usize, color: [f32; 3]) {
    use std::f32::consts::TAU;
    let n = [0.0, 1.0, 0.0];
    let center = verts.len() as u32;
    verts.push(CapVertex { position: [cx, y, cz], normal: n, color });
    for j in 0..=segments {
        let a = j as f32 / segments as f32 * TAU;
        verts.push(CapVertex { position: [cx + r * a.cos(), y, cz + r * a.sin()], normal: n, color });
    }
    for j in 0..segments as u32 {
        indices.extend_from_slice(&[center, center + 1 + j, center + 2 + j]);
    }
}

// CapVertex must be Pod for write_buffer; it already derives Pod/Zeroable in cap_pass.
const _: fn() = || {
    fn _assert_pod<T: Pod + Zeroable>() {}
    _assert_pod::<CapVertex>();
};
