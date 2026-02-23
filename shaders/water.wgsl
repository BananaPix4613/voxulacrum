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
    _pad3: f32,
    _pad4: vec2<f32>,
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
    @location(1) flow: vec2<f32>,
    @location(2) depth: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) flow: vec2<f32>,
    @location(2) depth: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Small wave displacement on the water surface
    let wave_phase = dot(in.position.xz, vec2<f32>(0.8, 0.6)) * 0.5 + globals.time * 1.5;
    var pos = in.position;
    pos.y += sin(wave_phase) * 0.04 + sin(wave_phase * 2.3 + 1.0) * 0.02;

    out.clip_position = globals.view_proj * vec4<f32>(pos, 1.0);
    out.world_position = pos;
    out.flow = in.flow;
    out.depth = in.depth;
    return out;
}

fn compute_shadow(world_pos: vec3<f32>) -> f32 {
    let light_space_pos = globals.light_space_matrix * vec4<f32>(world_pos, 1.0);
    let proj_coords = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = vec2<f32>(
        proj_coords.x * 0.5 + 0.5,
        proj_coords.y * -0.5 + 0.5,
    );
    let current_depth = proj_coords.z;

    if shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0 {
        return 1.0;
    }
    if current_depth > 1.0 || current_depth < 0.0 {
        return 1.0;
    }

    let bias = 0.003;
    let biased_depth = current_depth - bias;
    let texel_size = 1.0 / 4096.0;
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
    let deep_color = vec3<f32>(0.12, 0.25, 0.40);
    let shallow_color = vec3<f32>(0.25, 0.45, 0.55);
    let depth_t = clamp((in.depth - 1.0) * 0.2, 0.0, 1.0);
    let base_color = mix(shallow_color, deep_color, depth_t);

    let t = globals.time;
    let wp = in.world_position.xz;
    let ripple1 = sin(dot(wp, vec2<f32>(0.7, 0.5)) * 0.8 + t * 1.2) * 0.08;
    let ripple2 = sin(dot(wp, vec2<f32>(-0.4, 0.9)) * 1.1 + t * 0.9) * 0.06;
    let ripple3 = sin(dot(wp, vec2<f32>(0.6, -0.7)) * 1.5 + t * 1.6) * 0.04;
    let nx = ripple1 + ripple2 * 0.5;
    let nz = ripple2 + ripple3 * 0.5;
    let perturbed_normal = normalize(vec3<f32>(nx, 1.0, nz));

    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(perturbed_normal, sun_dir), 0.0);
    let diffuse = globals.sun_color * n_dot_l * 0.6;
    let ambient = globals.ambient_color * 0.5;

    let view_dir = normalize(vec3<f32>(0.577, 0.577, 0.577));
    let reflect_dir = reflect(-sun_dir, perturbed_normal);
    let spec = pow(max(dot(reflect_dir, view_dir), 0.0), 32.0);
    let spec_cel = step(0.5, spec);
    let specular = globals.sun_color * spec_cel * 0.4;

    let shadow = compute_shadow(in.world_position);

    let cloud_uv = in.world_position.xz * 0.03 + globals.cloud_shadow_offset;
    let cloud_sample = textureSample(cloud_texture, cloud_sampler, cloud_uv).r;
    let cloud_threshold = smoothstep(globals.cloud_coverage - 0.20, globals.cloud_coverage + 0.20, cloud_sample);
    let cloud_factor = mix(0.45, 1.0, cloud_threshold);

    let lit_color = base_color * (diffuse * shadow * cloud_factor + ambient) + specular * shadow * cloud_factor;
    let final_alpha = mix(0.4, 0.85, depth_t);

    return vec4<f32>(lit_color, final_alpha);
}