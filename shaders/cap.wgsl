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
    cloud_shadow_offset: vec2<f32>,
    cloud_coverage: f32,
    edge_strength: f32,
    ortho_ao_strength: f32,
    clip_enabled: u32,
    _pad_a: vec2<f32>,
    clip_min: vec3<f32>,
    _pad3: f32,
    clip_max: vec3<f32>,
    _pad4: f32,
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
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.normal = in.normal;
    out.color = in.color;
    return out;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    // Cross-section clipping: prevent cap from bleeding past other active clip planes
    if globals.clip_enabled != 0u {
        let wp = in.world_position;
        if wp.x > globals.clip_max.x || wp.y > globals.clip_max.y || wp.z > globals.clip_max.z
        || wp.x < globals.clip_min.x || wp.y < globals.clip_min.y || wp.z < globals.clip_min.z {
            discard;
        }
    }

    let n = normalize(in.normal);

    // Simple ambient + directional lighting for the cap surface
    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);
    let lit = in.color * (globals.ambient_color * 0.6 + globals.sun_color * n_dot_l * 0.4);

    return FragmentOutput(
        vec4<f32>(lit, 1.0),
        vec4<f32>(n, 1.0),
    );
}
