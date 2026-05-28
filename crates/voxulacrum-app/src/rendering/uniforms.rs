use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlobalUniforms {
    pub view_proj: [[f32; 4]; 4],          // 64 bytes, offset 0
    pub light_space_matrix: [[f32; 4]; 4], // 64 bytes, offset 64
    pub sun_direction: [f32; 3],           // 12 bytes, offset 128
    pub _pad0: f32,                        //  4 bytes, offset 140
    pub sun_color: [f32; 3],               // 12 bytes, offset 144
    pub _pad1: f32,                        //  4 bytes, offset 156
    pub ambient_color: [f32; 3],           // 12 bytes, offset 160
    pub _pad2: f32,                        //  4 bytes, offset 172
    pub wind_vector: [f32; 2],             //  8 bytes, offset 176
    pub time: f32,                         //  4 bytes, offset 184
    pub debug_mode: u32,                   //  4 bytes, offset 188
    pub cloud_shadow_offset: [f32; 2],     //  8 bytes, offset 192
    pub cloud_coverage: f32,               //  4 bytes, offset 200
    pub edge_strength: f32,                //  4 bytes, offset 204
    pub ortho_ao_strength: f32,            //  4 bytes, offset 208
    pub clip_enabled: u32,                 //  4 bytes, offset 212
    pub _pad_a: [f32; 2],                  //  8 bytes, offset 216 (aligns clip_min to 224)
    pub clip_min: [f32; 3],                // 12 bytes, offset 224
    pub _pad3: f32,                        //  4 bytes, offset 236
    pub clip_max: [f32; 3],                // 12 bytes, offset 240
    pub _pad4: f32,                        //  4 bytes, offset 252
}
// Total: 256 bytes (16 * 16). WGSL vec3 alignment satisfied: clip_min at 224, clip_max at 240.

impl Default for GlobalUniforms {
    fn default() -> Self {
        Self {
            view_proj: [[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]],
            light_space_matrix: [[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]],
            sun_direction: [0.5, 0.8, 0.3],
            _pad0: 0.0,
            sun_color: [1.0, 0.98, 0.92],
            _pad1: 0.0,
            ambient_color: [0.45, 0.45, 0.5],
            _pad2: 0.0,
            wind_vector: [0.0, 0.0],
            time: 0.0,
            debug_mode: 0,
            cloud_shadow_offset: [0.0, 0.0],
            cloud_coverage: 0.55,
            edge_strength: 0.15,
            ortho_ao_strength: 0.3,
            clip_enabled: 0,
            _pad_a: [0.0; 2],
            clip_min: [-10000.0; 3],
            _pad3: 0.0,
            clip_max: [10000.0; 3],
            _pad4: 0.0,
        }
    }
}

/// Shadow pass uniform
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ShadowUniforms {
    pub light_space_matrix: [[f32; 4]; 4], // 64 bytes, offset 0
    pub clip_min: [f32; 3],                // 12 bytes, offset 64
    pub clip_enabled: u32,                 //  4 bytes, offset 76
    pub clip_max: [f32; 3],                // 12 bytes, offset 80
    pub _pad: f32,                         //  4 bytes, offset 92
}
// Total: 96 bytes (6 * 16)

/// Post-processing uniforms
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PostProcessUniforms {
    pub warm_tint: [f32; 3],               // 12 bytes, offset  0
    pub warm_tint_strength: f32,           //  4 bytes, offset 12
    pub desaturation: f32,                 //  4 bytes, offset 16
    pub vignette_strength: f32,            //  4 bytes, offset 20
    pub exposure: f32,                     //  4 bytes, offset 24
    pub clip_fog_enabled: u32,             //  4 bytes, offset 28
    pub fog_color: [f32; 3],               // 12 bytes, offset 32
    pub fog_density: f32,                  //  4 bytes, offset 44
    pub clip_max: [f32; 3],                // 12 bytes, offset 48
    pub _pad0: f32,                        //  4 bytes, offset 60
    pub clip_min: [f32; 3],                // 12 bytes, offset 64
    pub _pad1: f32,                        //  4 bytes, offset 76
    pub inv_view_proj: [[f32; 4]; 4],      // 64 bytes, offset 80
    pub render_resolution: [f32; 2],       //  8 bytes, offset 144
    pub _pad2: [f32; 2],                   //  8 bytes, offset 152
}
// Total: 160 bytes (10 * 16)

impl Default for PostProcessUniforms {
    fn default() -> Self {
        Self {
            warm_tint: [1.0, 1.0, 1.0],
            warm_tint_strength: 0.0,
            desaturation: 0.0,
            vignette_strength: 0.35,
            exposure: 1.1,
            clip_fog_enabled: 0,
            fog_color: [0.4, 0.35, 0.3],
            fog_density: 2.0,
            clip_max: [10000.0; 3],
            _pad0: 0.0,
            clip_min: [-10000.0; 3],
            _pad1: 0.0,
            inv_view_proj: [[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]],
            render_resolution: [1.0, 1.0],
            _pad2: [0.0; 2],
        }
    }
}

/// Outline pass uniforms (48 bytes, 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct OutlineUniforms {
    pub texel_size: [f32; 2],          //  8 bytes, offset  0
    pub depth_threshold: f32,          //  4 bytes, offset  8
    pub depth_strength: f32,           //  4 bytes, offset 12
    pub normal_threshold: f32,         //  4 bytes, offset 16
    pub normal_strength: f32,          //  4 bytes, offset 20
    pub darken_strength: f32,          //  4 bytes, offset 24
    pub brighten_strength: f32,        //  4 bytes, offset 28
    pub enabled: u32,                  //  4 bytes, offset 32
    pub _pad: [u32; 3],                // 12 bytes, offset 36
}
// Total: 48 bytes = 3 × 16

pub fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("global_uniforms_bind_group_layout"),
        entries: &[
            // binding 0: global uniforms
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // binding 1: cloud shadow texture
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // binding 2: cloud shadow sampler
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // binding 3: shadow depth texture
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // binding 4: shadow comparison sampler
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}

pub fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    cloud_texture_view: &wgpu::TextureView,
    cloud_sampler: &wgpu::Sampler,
    shadow_texture_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("global_uniforms_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(cloud_texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(cloud_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(shadow_texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
        ],
    })
}

/// Bind group layout for the shadow depth pass (uniform buffer, visible to vertex + fragment
/// so the fragment stage can read clip bounds for cross-section discard)
pub fn create_shadow_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shadow_bind_group_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

pub fn create_shadow_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadow_bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}

/// Upscale pass uniforms (32 bytes, 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct UpscaleUniforms {
    pub subpixel_offset: [f32; 2],
    pub render_resolution: [f32; 2],
    pub window_resolution: [f32; 2],
    pub tex_resolution: [f32; 2],
}
// Total: 32 bytes = 2 × 16

pub fn create_post_process_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post_process_bind_group_layout"),
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
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            // binding 3: depth texture for cross-section fog world-position reconstruction
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

pub fn create_post_process_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    scene_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    depth_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post_process_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(scene_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
        ],
    })
}