//! wgpu pipelines + the per-frame Callback that paints the chunk inside the
//! egui viewport.
//!
//! egui-wgpu's main render pass has no depth attachment, so the 3D scene is
//! drawn into an offscreen color+depth target during `prepare`, then composed
//! into the egui pass during `paint` via a fullscreen-quad sample.
//!
//! Long-lived GPU resources (`ChunkRenderState`) are inserted once into
//! `egui-wgpu`'s `CallbackResources` during app construction and reused
//! every frame.

use bytemuck::{Pod, Zeroable};
use egui_wgpu::CallbackTrait;
use voxel_core::{ChunkBuffer, Voxel};
use wgpu::util::DeviceExt;

use super::camera::OrbitCamera;
use super::mesh::{build_chunk_mesh, FaceVertex};

// Format for the offscreen color target. Chosen to match common surface
// formats so a future direct-to-surface pass is a one-line swap.
const OFFSCREEN_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const TERRAIN_SHADER: &str = r#"
struct CameraUniform {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal:   vec3<f32>,
    @location(2) color:    vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.world_normal = in.normal;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);

    // Minecraft-style face-orientation shading. Reads as four crisp tiers
    // — top, N/S, E/W, bottom — so cubes pop without depending on a
    // particular light direction.
    var face_shade: f32 = 0.78;                       // default (N/S — Z faces)
    if (n.y > 0.5) { face_shade = 1.0; }              // top
    else if (n.y < -0.5) { face_shade = 0.45; }       // bottom
    else if (abs(n.x) > 0.5) { face_shade = 0.62; }   // E/W (X faces)

    // Soft directional Lambert layered on top, in a narrow band, so two
    // faces of the same orientation aren't completely flat.
    let l = normalize(-camera.light_dir.xyz);
    let lambert = max(dot(n, l), 0.0);
    let shade = face_shade * (0.85 + 0.15 * lambert);

    return vec4<f32>(in.color * shade, 1.0);
}
"#;

const COMPOSITE_SHADER: &str = r#"
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Fullscreen triangle (covers [-1..1] in both axes with one big tri).
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_smp, in.uv);
}
"#;

/// Per-mesh GPU buffers. Re-created on every upload.
struct MeshBuffers {
    vertex_buf: wgpu::Buffer,
    vertex_count: u32,
}

/// GPU uniform layout: viewproj + light dir (padded to 16 bytes).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

/// Offscreen color + depth target. Recreated when the viewport size changes.
struct Offscreen {
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    size: [u32; 2],
}

/// Long-lived GPU resources for the 3D viewer.
pub struct ChunkRenderState {
    // 3D scene pass
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    // Composite pass (offscreen → egui)
    composite_pipeline: wgpu::RenderPipeline,
    composite_bg_layout: wgpu::BindGroupLayout,
    composite_sampler: wgpu::Sampler,
    composite_bg: Option<wgpu::BindGroup>,
    offscreen: Option<Offscreen>,
    // Mesh
    mesh: Option<MeshBuffers>,
    pub last_meshed_version: u64,
}

impl ChunkRenderState {
    /// Build pipelines, uniform buffer, and bind groups. Called once during
    /// app startup.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        // --- 3D scene pass ------------------------------------------------
        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel_terrain_shader"),
            source: wgpu::ShaderSource::Wgsl(TERRAIN_SHADER.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("voxel_camera_uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxel_camera_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel_camera_bg"),
            layout: &uniform_bg_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel_terrain_pl"),
            bind_group_layouts: &[&uniform_bg_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FaceVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, // position
                1 => Float32x3, // normal
                2 => Float32x3, // color
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel_terrain_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OFFSCREEN_COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // --- Composite pass (offscreen color → egui surface) --------------
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel_composite_shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });

        let composite_bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("voxel_composite_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("voxel_composite_pl"),
            bind_group_layouts: &[&composite_bg_layout],
            push_constant_ranges: &[],
        });

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel_composite_pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let composite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("voxel_composite_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            pipeline,
            uniform_buf,
            uniform_bg,
            composite_pipeline,
            composite_bg_layout,
            composite_sampler,
            composite_bg: None,
            offscreen: None,
            mesh: None,
            last_meshed_version: u64::MAX,
        }
    }

    /// Mesh + upload the chunk if its `field_version` differs from the
    /// cached `last_meshed_version`. Cheap no-op otherwise.
    pub fn update_mesh(
        &mut self,
        device: &wgpu::Device,
        chunk: &ChunkBuffer<Voxel, 32>,
        version: u64,
    ) {
        if self.last_meshed_version == version && self.mesh.is_some() {
            return;
        }
        let mesh = build_chunk_mesh(chunk);
        let vertex_count = mesh.vertex_count();
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel_chunk_mesh"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.mesh = Some(MeshBuffers { vertex_buf, vertex_count });
        self.last_meshed_version = version;
    }

    fn ensure_offscreen(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let needs_resize = self.offscreen.as_ref().map(|o| o.size != size).unwrap_or(true);
        if !needs_resize {
            return;
        }
        let w = size[0].max(1);
        let h = size[1].max(1);
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxel_offscreen_color"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("voxel_offscreen_depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());

        self.composite_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("voxel_composite_bg"),
            layout: &self.composite_bg_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.composite_sampler),
                },
            ],
        }));
        self.offscreen = Some(Offscreen { color_view, depth_view, size });
    }

    fn update_uniforms(&self, queue: &wgpu::Queue, camera: &OrbitCamera, aspect: f32) {
        let view_proj = camera.view_proj(aspect);
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: [-0.4, -0.8, -0.5, 0.0],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[uniform]));
    }
}

/// Per-frame callback handed to egui-wgpu. `prepare` draws the 3D scene
/// off-screen; `paint` samples that texture into the egui pass.
pub struct RenderCallback {
    pub camera: OrbitCamera,
    pub size: [u32; 2],
}

impl CallbackTrait for RenderCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let state = resources
            .get_mut::<ChunkRenderState>()
            .expect("ChunkRenderState missing from CallbackResources");

        state.ensure_offscreen(device, self.size);
        let aspect = self.size[0].max(1) as f32 / self.size[1].max(1) as f32;
        state.update_uniforms(queue, &self.camera, aspect);

        let (Some(offscreen), Some(mesh)) = (state.offscreen.as_ref(), state.mesh.as_ref()) else {
            return Vec::new();
        };

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("voxel_offscreen_rp"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &offscreen.color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.07, g: 0.08, b: 0.10, a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &offscreen.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&state.pipeline);
        rp.set_bind_group(0, &state.uniform_bg, &[]);
        rp.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
        rp.draw(0..mesh.vertex_count, 0..1);
        drop(rp);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let state = resources
            .get::<ChunkRenderState>()
            .expect("ChunkRenderState missing from CallbackResources");
        let Some(bg) = state.composite_bg.as_ref() else { return };
        render_pass.set_pipeline(&state.composite_pipeline);
        render_pass.set_bind_group(0, bg, &[]);
        // Fullscreen triangle: 3 verts, no vertex buffer.
        render_pass.draw(0..3, 0..1);
    }
}
