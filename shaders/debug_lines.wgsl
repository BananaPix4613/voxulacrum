struct GlobalUniforms {
    view_proj: mat4x4<f32>,
    light_space_matrix: mat4x4<f32>,
    sun_direction: vec3<f32>,
    _pad0: f32,
    sun_color: vec3<f32>,
    _pad1: f32,
    ambient_color: vec3<f32>,
    _pad2: f32,
    wind_vector: vec2<f32>,
    time: f32,
    debug_mode: u32,
    cloud_offset_0: vec2<f32>,
    cloud_coverage_0: f32,
    cloud_uv_scale_0: f32,
    cloud_offset_1: vec2<f32>,
    cloud_coverage_1: f32,
    cloud_uv_scale_1: f32,
    cloud_offset_2: vec2<f32>,
    cloud_coverage_2: f32,
    cloud_uv_scale_2: f32,
    cloud_offset_3: vec2<f32>,
    cloud_coverage_3: f32,
    cloud_uv_scale_3: f32,
    env_origin: vec2<f32>,
    env_extent: f32,
    _pad_env: f32,
    cloud_defaults: vec4<f32>,
    env_typical: vec4<f32>,
    edge_strength: f32,
    ortho_ao_strength: f32,
    clip_enabled: u32,
    _pad_a: f32,
    clip_min: vec3<f32>,
    _pad3: f32,
    clip_max: vec3<f32>,
    _pad4: f32,
    mask_origin: vec3<f32>,
    mask_enabled: u32,
    view_dir: vec3<f32>,
    _pad5: f32,
    render_size: vec2<f32>,
    volume_radius: f32,
    _pad6: f32,
};

@group(0) @binding(0)
var<uniform> globals: GlobalUniforms;

// Unused bindings required by shared bind group layout
@group(0) @binding(1)
var cloud_texture: texture_2d<f32>;
@group(0) @binding(2)
var cloud_sampler: sampler;
@group(0) @binding(3)
var shadow_map: texture_depth_2d;
@group(0) @binding(4)
var shadow_sampler: sampler_comparison;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    var out: FragmentOutput;
    out.color = vec4<f32>(in.color, 0.8);
    out.normal = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    return out;
}