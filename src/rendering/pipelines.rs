use bytemuck::{Pod, Zeroable};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Serialize, Deserialize)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub ao: f32,
    pub material_id: u32,
    pub cell_flags: u32,
    pub _pad_vert: [u32; 2], // Pad to 64 bytes (16-byte alignment for GPU)
}

impl TerrainVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0,  shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x3 },
                wgpu::VertexAttribute { offset: 36, shader_location: 3, format: wgpu::VertexFormat::Float32 },
                wgpu::VertexAttribute { offset: 40, shader_location: 4, format: wgpu::VertexFormat::Uint32 },
                wgpu::VertexAttribute { offset: 44, shader_location: 5, format: wgpu::VertexFormat::Uint32 },
            ],
        }
    }
}

pub fn create_terrain_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
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
            buffers: &[TerrainVertex::layout()],
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
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
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
            buffers: &[TerrainVertex::layout()],
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
            format: wgpu::TextureFormat::Depth32Float,
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
            buffers: &[TerrainVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: None,
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
            bias: wgpu::DepthBiasState {
                constant: 2,
                slope_scale: 2.0,
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
pub struct GrassInstance {
    pub position: [f32; 3],
    pub scale: f32,
    pub rotation: f32,
    pub blade_phase: f32,
    pub terrain_color: [f32; 3],
    pub _pad1: f32,
}

pub fn create_vegetation_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vegetation_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("vegetation_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("vegetation_pipeline"),
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
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GrassInstance>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 5,
                            format: wgpu::VertexFormat::Float32,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 6,
                            format: wgpu::VertexFormat::Float32,
                        },
                        wgpu::VertexAttribute {
                            offset: 20,
                            shader_location: 7,
                            format: wgpu::VertexFormat::Float32,
                        },
                        wgpu::VertexAttribute {
                            offset: 24,
                            shader_location: 8,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 36,
                            shader_location: 9,
                            format: wgpu::VertexFormat::Float32,
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
            format: wgpu::TextureFormat::Depth32Float,
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
    pub position: [f32; 3],
    pub flow: [f32; 2],
    pub depth: f32,
}

impl WaterVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WaterVertex>() as wgpu::BufferAddress,
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
                wgpu::VertexAttribute {
                    offset: 20,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        }
    }
}

pub fn create_water_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    global_bind_group_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("water_shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water_pipeline_layout"),
        bind_group_layouts: &[global_bind_group_layout],
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
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
    Vegetation,
    Water,
    PostProcess,
}

/// Resources needed to rebuild pipelines (bind group layouts, surface format).
pub struct PipelineResources {
    pub surface_format: wgpu::TextureFormat,
    pub global_bind_group_layout: wgpu::BindGroupLayout,
    pub shadow_bind_group_layout: wgpu::BindGroupLayout,
    pub post_process_bind_group_layout: wgpu::BindGroupLayout,
}

/// Entry in the pipeline registry tracking one shader->pipeline mapping.
struct PipelineEntry {
    pipeline_id: PipelineId,
    last_good_source: String,
}

/// Registry mapping shader filenames to the pipelines they affect.
/// Also holds the live pipeline handles.
pub struct PipelineRegistry {
    entries: HashMap<String, PipelineEntry>,
    pub terrain_pipeline: wgpu::RenderPipeline,
    pub terrain_wireframe_pipeline: wgpu::RenderPipeline,
    pub shadow_pipeline: wgpu::RenderPipeline,
    pub vegetation_pipeline: wgpu::RenderPipeline,
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
        device: &wgpu::Device,
        resources: &PipelineResources,
        shader_dir: &Path,
    ) -> Self {
        let terrain_source = read_shader(shader_dir, "terrain.wgsl");
        let shadow_source = read_shader(shader_dir, "shadow.wgsl");
        let vegetation_source = read_shader(shader_dir, "vegetation.wgsl");
        let water_source = read_shader(shader_dir, "water.wgsl");

        let terrain_pipeline = create_terrain_pipeline(
            device, resources.surface_format, &resources.global_bind_group_layout,
            &terrain_source,
        );
        let terrain_wireframe_pipeline = create_terrain_wireframe_pipeline(
            device, resources.surface_format, &resources.global_bind_group_layout,
            &terrain_source,
        );
        let shadow_pipeline = create_shadow_pipeline(
            device, &resources.shadow_bind_group_layout,
            &shadow_source,
        );
        let vegetation_pipeline = create_vegetation_pipeline(
            device, resources.surface_format, &resources.global_bind_group_layout,
            &vegetation_source,
        );
        let water_pipeline = create_water_pipeline(
            device, resources.surface_format, &resources.global_bind_group_layout,
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
        entries.insert("vegetation.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Vegetation,
            last_good_source: vegetation_source,
        });
        entries.insert("water.wgsl".to_string(), PipelineEntry {
            pipeline_id: PipelineId::Water,
            last_good_source: water_source,
        });

        Self {
            entries,
            terrain_pipeline,
            terrain_wireframe_pipeline,
            shadow_pipeline,
            vegetation_pipeline,
            water_pipeline,
        }
    }

    /// Attempt to hot-reload a shader. Returns a result describing success or failure.
    /// On failure, the old pipeline is retained.
    pub fn try_reload(
        &mut self,
        device: &wgpu::Device,
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
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let result = match pipeline_id {
            PipelineId::Terrain => {
                let p = create_terrain_pipeline(
                    device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                // Also rebuild wireframe variant
                let wf = create_terrain_wireframe_pipeline(
                    device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                self.terrain_wireframe_pipeline = wf;
                Some(p)
            }
            PipelineId::Shadow => {
                let p = create_shadow_pipeline(
                    device, &resources.shadow_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::Vegetation => {
                let p = create_vegetation_pipeline(
                    device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::Water => {
                let p = create_water_pipeline(
                    device, resources.surface_format,
                    &resources.global_bind_group_layout, &source,
                );
                Some(p)
            }
            PipelineId::PostProcess => None, // Handled separately via PostProcessPass
        };

        // Check for compilation errors
        let error = pollster::block_on(device.pop_error_scope());

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
                PipelineId::Vegetation => self.vegetation_pipeline = new_pipeline,
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