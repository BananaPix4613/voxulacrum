use bytemuck::{Pod, Zeroable};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::Path;
use bevy_ecs::prelude::Resource;
use crate::rendering::render_context::RenderContext;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Serialize, Deserialize)]
pub struct FaceVertex {
    pub position: [f32; 3],     // 0..12
    pub normal: [i8; 4],        // 12..16 - Snorm8x4: xyz cardinal (+/-127), w unused
    pub uv: [u16; 2],           // 16..20 - unused in Phase 9 (written 0)
    pub material_id: u16,       // 20..22
    pub biome_tint_index: u8,   // 22 - §11 future (0)
    pub variant_index: u8,      // 23 - §11 future (0)
    pub face_axis: u8,          // 24 - 0..5 (+X,-X,+Y,-Y,+Z,-Z); read by Substep 8
    pub occlusion_class: u8,    // 25 - §11 future (0)
    pub light_level_index: u8,  // 26 - §11 future (0)
    pub enclosure_factor: u8,   // 27 - filled by Substep 9 (0 now)
    pub edge_flag: u8,          // 28 - §11 future (0)
    pub sway_weight: u8,        // 29 - foliage sway (0 for terrain)
    pub ao_factor: u8,          // 30 - baked AO; 255 = unoccluded
    pub _padding: u8,           // 31
}

impl FaceVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FaceVertex>() as wgpu::BufferAddress, // 32
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Snorm8x4 },
                wgpu::VertexAttribute { offset: 16, shader_location: 2, format: wgpu::VertexFormat::Uint16x2 },
                wgpu::VertexAttribute { offset: 20, shader_location: 3, format: wgpu::VertexFormat::Uint16x2 },
                wgpu::VertexAttribute { offset: 24, shader_location: 4, format: wgpu::VertexFormat::Uint8x4 },
                wgpu::VertexAttribute { offset: 28, shader_location: 5, format: wgpu::VertexFormat::Uint8x4 },
            ],
        }
    }
}

pub fn create_terrain_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terrain_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[FaceVertex::layout()],
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
            cull_mode,
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
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

pub fn create_terrain_wireframe_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain_wireframe_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain_wireframe_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("terrain_wireframe_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[FaceVertex::layout()],
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
            cull_mode: Some(wgpu::Face::Back),
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Line,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
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

pub fn create_shadow_pipeline(
    device: &wgpu::Device,
    shadow_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow_pipeline_layout"),
        bind_group_layouts: &[shadow_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[FaceVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader_module,
            entry_point: Some("fs_main"),
            targets: &[],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            // Normal-offset bias (shaders/terrain.wgsl) handles self-shadowing
            // acne across most of the angle range, decoupled from face-to-light
            // slope. Its own bias is capped well under half a voxel so it can't
            // overshoot into neighboring geometry at concave/interior corners;
            // slope_scale here just picks up the residual at grazing sun angles
            // beyond that cap. Keep this modest - the normal-offset cap was
            // tightened specifically (0.04 base / 0.1 max in terrain.wgsl) so
            // this bias wouldn't need to be large; pushing slope_scale much
            // higher reintroduces peter-panning as a visible hard seam/step in
            // the shadow, since there's no PCF blur left to mask it (see
            // compute_shadow - PCF was dropped to a single hard sample for
            // crisp voxel-style shadow edges).
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 1.0,
                clamp: 0.0,
            },
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GrassVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ScatterVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// One tapered capsule, world space. The whole per-instance payload: the quad
/// is generated from `@builtin(vertex_index)`, so this pass binds no vertex
/// buffer at all.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct CapsuleInstanceGpu {
    /// Thick end.
    pub a: [f32; 3],
    /// Radius at `a`.
    pub ra: f32,
    /// Thin end.
    pub b: [f32; 3],
    /// Radius at `b`.
    pub rb: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ScatterInstanceGpu {
    pub position: [f32; 3],
    pub rotation_y: f32,
    pub color: [f32; 3],
    pub scale: f32,
}

pub fn create_detail_paint_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("detail_paint_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let chunk_layout =
        crate::rendering::detail_paint_pass::create_detail_chunk_layout(device);

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("detail_paint_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout, &chunk_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("detail_paint_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GrassVertex>() as wgpu::BufferAddress,
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
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                },
            ],
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
            format: crate::rendering::render_targets::SCENE_DEPTH_FORMAT,
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct WaterVertex {
    pub position: [f32; 3], // 0..12
    pub normal: [f32; 3],   // 12..24 - smoothed surface normal from the height field
    pub flow: [f32; 2],     // 24..32 - world-XZ downhill flow (0 on flat water)
    pub depth: f32,         // 32..36 - contiguous water depth below the surface, voxels
}

impl WaterVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WaterVertex>() as wgpu::BufferAddress, // 36
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 32, shader_location: 3, format: wgpu::VertexFormat::Float32 },
            ],
        }
    }
}

pub fn create_water_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    water_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("water_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout, water_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("water_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[WaterVertex::layout()],
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
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None, // manual depth occlusion via the sampled scene depth
        multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
        multiview: None,
        cache: None,
    })
}

pub fn create_scatter_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scatter_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("scatter_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("scatter_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ScatterVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                        wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
                    ],
                },
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ScatterInstanceGpu>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute { offset: 0,  shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
                        wgpu::VertexAttribute { offset: 12, shader_location: 3, format: wgpu::VertexFormat::Float32 },
                        wgpu::VertexAttribute { offset: 16, shader_location: 4, format: wgpu::VertexFormat::Float32x3 },
                        wgpu::VertexAttribute { offset: 28, shader_location: 5, format: wgpu::VertexFormat::Float32 },
                    ],
                },
            ],
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
            cull_mode: None, // prefab meshes include double-sided crossed quads
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
    })
}

/// Analytic tapered-capsule impostors for wood (spec §5).
///
/// No vertex buffer: the covering quad comes from `@builtin(vertex_index)`, so
/// slot 0 is the instance stream. Depth writes come from the fragment shader,
/// which is why `depth_compare` still tests but the geometry drawn is a flat
/// billboard - the rasterizer's own depth is never used.
pub fn create_capsule_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("capsule_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("capsule_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("capsule_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<CapsuleInstanceGpu>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                    wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32 },
                    wgpu::VertexAttribute { offset: 16, shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
                    wgpu::VertexAttribute { offset: 28, shader_location: 3, format: wgpu::VertexFormat::Float32 },
                ],
            }],
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
            // The quad is built facing the camera, but winding depends on the
            // yaw, so culling it would drop half the tree on two of the four
            // camera orientations.
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
    })
}

/// Wood impostors as shadow casters. Depth-only, no color targets, solving
/// against the light basis rather than the view basis.
pub fn create_capsule_shadow_pipeline(
    device: &wgpu::Device,
    shadow_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("capsule_shadow_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("capsule_shadow_pipeline_layout"),
        bind_group_layouts: &[shadow_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("capsule_shadow_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<CapsuleInstanceGpu>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                    wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32 },
                    wgpu::VertexAttribute { offset: 16, shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
                    wgpu::VertexAttribute { offset: 28, shader_location: 3, format: wgpu::VertexFormat::Float32 },
                ],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader_module,
            entry_point: Some("fs_main"),
            targets: &[],
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
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            // No slope-scale bias: the terrain pipeline's exists for flat faces
            // meeting the light at a grazing angle, and a capsule writes true
            // curved depth. `terrain.wgsl`'s normal-offset handles the receiver
            // side either way.
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
        multiview: None,
        cache: None,
    })
}

pub fn create_post_process_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    pp_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("post_process_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post_process_pipeline_layout"),
        bind_group_layouts: &[pp_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("post_process_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[], // Fullscreen triangle via vertex_index
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
        depth_stencil: None, // No depth needed for fullscreen quad
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

// =============================================================================
// Pipeline Registry for hot-reloading
// =============================================================================

/// Identifies which pipeline a shader file maps to.
#[derive(Clone, Copy, Hash, Eq, PartialEq, Debug)]
pub enum PipelineId {
    Terrain,
    Shadow,
    DetailPaint,
    Scatter,
    Capsule,
    CapsuleShadow,
    Water,
    #[allow(dead_code)] // reserved; post-process currently runs via PostProcessPass
    PostProcess,
}

/// Resources needed to rebuild pipelines (bind group layouts, surface format).
#[derive(Resource)]
pub struct PipelineResources {
    pub surface_format: wgpu::TextureFormat,
    pub global_bind_group_layout: wgpu::BindGroupLayout,
    pub water_bind_group_layout: wgpu::BindGroupLayout,
    pub shadow_bind_group_layout: wgpu::BindGroupLayout,
    #[allow(dead_code)] // reserved; post-process currently runs via PostProcessPass
    pub post_process_bind_group_layout: wgpu::BindGroupLayout,
}

/// Entry in the pipeline registry tracking one shader->pipeline mapping.
struct PipelineEntry {
    pipeline_id: PipelineId,
    last_good_source: String,
}

/// Registry mapping shader filenames to the pipelines they affect.
/// Also holds the live pipeline handles.
#[derive(Resource)]
pub struct PipelineRegistry {
    entries: HashMap<String, PipelineEntry>,
    pub terrain_pipeline: wgpu::RenderPipeline,
    pub terrain_reflection_pipeline: wgpu::RenderPipeline,
    pub terrain_wireframe_pipeline: wgpu::RenderPipeline,
    pub shadow_pipeline: wgpu::RenderPipeline,
    pub detail_paint_pipeline: wgpu::RenderPipeline,
    pub scatter_pipeline: wgpu::RenderPipeline,
    pub capsule_pipeline: wgpu::RenderPipeline,
    pub capsule_shadow_pipeline: wgpu::RenderPipeline,
    pub water_pipeline: wgpu::RenderPipeline,
}

/// Result of a hot-reload attempt for a single shader.
pub struct ReloadResult {
    pub filename: String,
    pub success: bool,
    pub message: String,
}

impl PipelineRegistry {
    /// Create the registry, loading all shaders from disk at runtime.
    pub fn new(
        ctx: &RenderContext,
        resources: &PipelineResources,
        shader_dir: &Path,
    ) -> Self {
        let terrain_source = read_shader(shader_dir, "terrain.wgsl");
        let shadow_source = read_shader(shader_dir, "shadow.wgsl");
        let detail_paint_source = read_shader(shader_dir, "detail_paint.wgsl");
        let scatter_source = read_shader(shader_dir, "scatter.wgsl");
        let capsule_source = read_shader(shader_dir, "capsule.wgsl");
        let capsule_shadow_source = read_shader(shader_dir, "capsule_shadow.wgsl");
        let water_source = read_shader(shader_dir, "water.wgsl");

        let terrain_pipeline = create_terrain_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &terrain_source, Some(wgpu::Face::Back),
        );
        let terrain_reflection_pipeline = create_terrain_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &terrain_source, None,
        );
        let terrain_wireframe_pipeline = create_terrain_wireframe_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &terrain_source,
        );
        let shadow_pipeline = create_shadow_pipeline(
            &ctx.device, &resources.shadow_bind_group_layout,
            &shadow_source,
        );
        let detail_paint_pipeline = create_detail_paint_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &detail_paint_source,
        );
        let scatter_pipeline = create_scatter_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &scatter_source,
        );
        let capsule_pipeline = create_capsule_pipeline(
            &ctx.device, resources.surface_format, &resources.global_bind_group_layout,
            &capsule_source,
        );
        let capsule_shadow_pipeline = create_capsule_shadow_pipeline(
            &ctx.device, &resources.shadow_bind_group_layout, &capsule_shadow_source,
        );
        let water_pipeline = create_water_pipeline(
            &ctx.device, resources.surface_format,
            &resources.global_bind_group_layout, &resources.water_bind_group_layout,
            &water_source,
        );

        let mut entries = HashMap::new();
        entries.insert("terrain.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Terrain,
            last_good_source: terrain_source,
        });
        entries.insert("shadow.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Shadow,
            last_good_source: shadow_source,
        });
        entries.insert("detail_paint.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::DetailPaint,
            last_good_source: detail_paint_source,
        });
        entries.insert("scatter.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Scatter,
            last_good_source: scatter_source,
        });
        entries.insert("capsule.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Capsule,
            last_good_source: capsule_source,
        });
        entries.insert("capsule_shadow.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::CapsuleShadow,
            last_good_source: capsule_shadow_source,
        });
        entries.insert("water.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Water,
            last_good_source: water_source,
        });

        Self {
            entries,
            terrain_pipeline,
            terrain_reflection_pipeline,
            terrain_wireframe_pipeline,
            shadow_pipeline,
            detail_paint_pipeline,
            scatter_pipeline,
            capsule_pipeline,
            capsule_shadow_pipeline,
            water_pipeline,
        }
    }

    /// Attempt to hot-reload a shader. Returns a result describing success or failure.
    /// On failure, the old pipeline is retained.
    pub fn try_reload(
        &mut self,
        ctx: &RenderContext,
        resources: &PipelineResources,
        shader_path: &Path,
    ) -> Option<ReloadResult> {
        let filename = shader_path.file_name()?.to_str()?.to_string();

        let entry = self.entries.get(&filename)?;
        let pipeline_id = entry.pipeline_id;

        // Read new source
        let source = match std::fs::read_to_string(shader_path) {
            Ok(s) => s,
            Err(e) => {
                return Some(ReloadResult {
                    filename,
                    success: false,
                    message: format!("Failed to read file: {}", e),
                });
            }
        };

        // Use error scopes to catch shader compilation errors
        ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);

        let result = match pipeline_id {
            PipelineId::Terrain => {
                let p = create_terrain_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source, Some(wgpu::Face::Back),
                );
                let wf = create_terrain_wireframe_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                self.terrain_wireframe_pipeline = wf;
                self.terrain_reflection_pipeline = create_terrain_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source, None,
                );
                Some(p)
            }
            PipelineId::Shadow => {
                let p = create_shadow_pipeline(
                    &ctx.device, &resources.shadow_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::DetailPaint => {
                let p = create_detail_paint_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::Scatter => {
                let p = create_scatter_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::Capsule => {
                let p = create_capsule_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::CapsuleShadow => {
                let p = create_capsule_shadow_pipeline(
                    &ctx.device, &resources.shadow_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::Water => {
                let p = create_water_pipeline(
                    &ctx.device, resources.surface_format,
                    &resources.global_bind_group_layout, &resources.water_bind_group_layout,
                    &source,
                );
                Some(p)
            }
            PipelineId::PostProcess => None, // Handled separately via PostProcessPass
        };

        // Check for compilation errors
        let error = pollster::block_on(ctx.device.pop_error_scope());

        if let Some(err) = error {
            return Some(ReloadResult {
                filename,
                success: false,
                message: format!("Shader compile error: {}", err),
            });
        }

        if let Some(new_pipeline) = result {
            // Swap in the new pipeline
            match pipeline_id {
                PipelineId::Terrain => self.terrain_pipeline = new_pipeline,
                PipelineId::Shadow => self.shadow_pipeline = new_pipeline,
                PipelineId::DetailPaint => self.detail_paint_pipeline = new_pipeline,
                PipelineId::Scatter => self.scatter_pipeline = new_pipeline,
                PipelineId::Capsule => self.capsule_pipeline = new_pipeline,
                PipelineId::CapsuleShadow => self.capsule_shadow_pipeline = new_pipeline,
                PipelineId::Water => self.water_pipeline = new_pipeline,
                PipelineId::PostProcess => {}
            }

            // Update the last good source
            if let Some(entry) = self.entries.get_mut(&filename) {
                entry.last_good_source = source;
            }

            Some(ReloadResult {
                filename,
                success: true,
                message: "Reloaded successfully".to_string(),
            })
        } else {
            None
        }
    }
}

/// Read a shader file from the shader directory. Panics if the file cannot be read
/// (only used during initial startup where failure is unrecoverable).
fn read_shader(shader_dir: &Path, filename: &str) -> String {
    let path = shader_dir.join(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read shader {}: {}", path.display(), e))
}