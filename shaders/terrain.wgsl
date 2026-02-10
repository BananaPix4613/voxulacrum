struct GlobalUniforms {
    view_proj: mat4x4<f32>,
    sun_direction: vec3<f32>,
    _pad0: f32,
    sun_color: vec3<f32>,
    _pad1: f32,
    ambient_color: vec3<f32>,
    _pad2: f32,
    wind_vector: vec2<f32>,
    time: f32,
    _pad_time: f32,
    cloud_shadow_offset: vec2<f32>,
    _pad3: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> globals: GlobalUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) ao: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) ao: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.normal = in.normal;
    out.color = in.color;
    out.ao = in.ao;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let n_dot_l = max(dot(n, normalize(globals.sun_direction)), 0.0);
    let diffuse = globals.sun_color * n_dot_l;
    let ambient = globals.ambient_color * 0.4;
    let ao_factor = mix(0.3, 1.0, in.ao);
    // Phase 1: no cloud shadow texture yet
    let cloud_factor = 1.0;
    let lit_color = in.color * (diffuse * cloud_factor + ambient) * ao_factor;
    return vec4<f32>(lit_color, 1.0);
}