use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlobalUniforms {
    pub view_proj: [[f32; 4]; 4],        // 64 bytes, offset 0
    pub sun_direction: [f32; 3],         // 12 bytes, offset 64
    pub _pad0: f32,                      //  4 bytes, offset 76
    pub sun_color: [f32; 3],             // 12 bytes, offset 80
    pub _pad1: f32,                      //  4 bytes, offset 92
    pub ambient_color: [f32; 3],         // 12 bytes, offset 96
    pub _pad2: f32,                      //  4 bytes, offset 108
    pub wind_vector: [f32; 2],           //  8 bytes, offset 112
    pub time: f32,                       //  4 bytes, offset 120
    pub _pad_time: f32,                  //  4 bytes, offset 124 (align next vec2)
    pub cloud_shadow_offset: [f32; 2],   //  8 bytes, offset 128
    pub _pad3: [f32; 2],                 //  8 bytes, offset 136 (pad to 144 = 16*9)
}
// Total: 144 bytes - matches WGSL uniform layout exactly

impl Default for GlobalUniforms {
    fn default() -> Self {
        Self {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            sun_direction: [0.5, 0.8, 0.3],
            _pad0: 0.0,
            sun_color: [1.0, 0.98, 0.92],
            _pad1: 0.0,
            ambient_color: [0.45, 0.45, 0.5],
            _pad2: 0.0,
            wind_vector: [0.0, 0.0],
            time: 0.0,
            _pad_time: 0.0,
            cloud_shadow_offset: [0.0, 0.0],
            _pad3: [0.0, 0.0],
        }
    }
}

pub fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("global_uniforms_bind_group_layout"),
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

pub fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("global_uniforms_bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}