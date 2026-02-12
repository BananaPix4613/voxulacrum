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
    _pad_time: f32,
    cloud_shadow_offset: vec2<f32>,
    cloud_coverage: f32,
    _pad3: f32,
};

@group(0) @binding(0)
var<uniform> globals: GlobalUniforms;

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

fn compute_shadow(world_pos: vec3<f32>) -> f32 {
    // Transform world position to light space
    let light_space_pos = globals.light_space_matrix * vec4<f32>(world_pos, 1.0);

    // Perspective divide (no-op for ortho, but correct regardless)
    let proj_coords = light_space_pos.xyz / light_space_pos.w;

    // Convert from NDC [-1,1] to texture coordinates [0,1]
    let shadow_uv = vec2<f32>(
        proj_coords.x * 0.5 + 0.5,
        proj_coords.y * -0.5 + 0.5,
    );

    // Depth in light space (already in [0,1] for orthographic RH)
    let current_depth = proj_coords.z;

    // Out of shadow map bounds = fully lit
    if shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0 {
        return 1.0;
    }
    if current_depth > 1.0 || current_depth < 0.0 {
        return 1.0;
    }

    // PCF (Percentage Closer Filtering) — 3x3 kernel for soft shadow edges
    // Apply small bias to prevent shadow acne
    let bias = 0.002;
    let biased_depth = current_depth - bias;
    let texel_size = 1.0 / 2048.0;
    var shadow_sum = 0.0;
    for (var x: i32 = -1; x <= 1; x++) {
        for (var y: i32 = -1; y <= 1; y++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            shadow_sum += textureSampleCompare(
                shadow_map,
                shadow_sampler,
                shadow_uv + offset,
                biased_depth
            );
        }
    }
    return shadow_sum / 9.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);

    // Sun color is already faded to zero at horizon by the CPU
    let diffuse = globals.sun_color * n_dot_l;

    let ambient = globals.ambient_color * 0.4;
    let ao_factor = mix(0.3, 1.0, in.ao);

    // Cloud shadow sampling
    let cloud_uv = in.world_position.xz * 0.03 + globals.cloud_shadow_offset;
    let cloud_sample = textureSample(cloud_texture, cloud_sampler, cloud_uv).r;
    let cloud_threshold = smoothstep(
        globals.cloud_coverage - 0.15,
        globals.cloud_coverage + 0.15,
        cloud_sample
    );
    let cloud_factor = mix(0.45, 1.0, cloud_threshold);

    // Shadow mapping — only affects direct sunlight
    let shadow = compute_shadow(in.world_position);

    let lit_color = in.color * (diffuse * shadow * cloud_factor + ambient) * ao_factor;
    return vec4<f32>(lit_color, 1.0);
}