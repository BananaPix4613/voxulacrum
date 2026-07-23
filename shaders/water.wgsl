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

@group(0) @binding(1)
var cloud_texture: texture_2d<f32>;

@group(0) @binding(2)
var cloud_sampler: sampler;

@group(0) @binding(3)
var shadow_map: texture_depth_2d;

@group(0) @binding(4)
var shadow_sampler: sampler_comparison;

@group(0) @binding(7)
var cloud_env_texture: texture_2d<f32>;

@group(0) @binding(8)
var cloud_env_sampler: sampler;

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

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    // Cross-section clipping
    if globals.clip_enabled != 0u {
        let wp = in.world_position;
        if wp.x > globals.clip_max.x || wp.y > globals.clip_max.y || wp.z > globals.clip_max.z
        || wp.x < globals.clip_min.x || wp.y < globals.clip_min.y || wp.z < globals.clip_min.z {
            discard;
        }
    }

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

    let env_uv_0 = (in.world_position.xz - globals.env_origin) / globals.env_extent;
    var env_0 = globals.cloud_defaults.x;
    if env_uv_0.x >= 0.0 && env_uv_0.x <= 1.0 && env_uv_0.y >= 0.0 && env_uv_0.y <= 1.0 {
        env_0 = textureSample(cloud_env_texture, cloud_env_sampler, env_uv_0).r;
    }
    let cloud_uv_0 = in.world_position.xz * globals.cloud_uv_scale_0 + globals.cloud_offset_0;
    let detail_0 = textureSample(cloud_texture, cloud_sampler, cloud_uv_0).r;
    let density_0 = env_0 * detail_0;
    let cloud_threshold_0 = smoothstep(globals.cloud_coverage_0 - 0.015, globals.cloud_coverage_0 + 0.015, density_0);
    let cloud_depth_0 = clamp(env_0 / max(globals.env_typical.x, 0.05), 0.0, 2.0);
    let storm_dark_0 = clamp(mix(1.0, 0.55, cloud_depth_0), 0.0, 1.0);
    let cloud_factor_0 = mix(1.0, storm_dark_0, cloud_threshold_0);

    let env_uv_1 = (in.world_position.xz - globals.env_origin) / globals.env_extent;
    var env_1 = globals.cloud_defaults.y;
    if env_uv_1.x >= 0.0 && env_uv_1.x <= 1.0 && env_uv_1.y >= 0.0 && env_uv_1.y <= 1.0 {
        env_1 = textureSample(cloud_env_texture, cloud_env_sampler, env_uv_1).r;
    }
    let cloud_uv_1 = in.world_position.xz * globals.cloud_uv_scale_1 + globals.cloud_offset_1;
    let detail_1 = textureSample(cloud_texture, cloud_sampler, cloud_uv_1).g;
    let density_1 = env_1 * detail_1;
    let cloud_threshold_1 = smoothstep(globals.cloud_coverage_1 - 0.015, globals.cloud_coverage_1 + 0.015, density_1);
    let cloud_depth_1 = clamp(env_1 / max(globals.env_typical.y, 0.05), 0.0, 2.0);
    let storm_dark_1 = clamp(mix(1.0, 0.68, cloud_depth_1), 0.0, 1.0);
    let cloud_factor_1 = mix(1.0, storm_dark_1, cloud_threshold_1);

    let env_uv_2 = (in.world_position.xz - globals.env_origin) / globals.env_extent;
    var env_2 = globals.cloud_defaults.z;
    if env_uv_2.x >= 0.0 && env_uv_2.x <= 1.0 && env_uv_2.y >= 0.0 && env_uv_2.y <= 1.0 {
        env_2 = textureSample(cloud_env_texture, cloud_env_sampler, env_uv_2).r;
    }
    let cloud_uv_2 = in.world_position.xz * globals.cloud_uv_scale_2 + globals.cloud_offset_2;
    let detail_2 = textureSample(cloud_texture, cloud_sampler, cloud_uv_2).b;
    let density_2 = env_2 * detail_2;
    let cloud_threshold_2 = smoothstep(globals.cloud_coverage_2 - 0.015, globals.cloud_coverage_2 + 0.015, density_2);
    let cloud_depth_2 = clamp(env_2 / max(globals.env_typical.z, 0.05), 0.0, 2.0);
    let storm_dark_2 = clamp(mix(1.0, 0.85, cloud_depth_2), 0.0, 1.0);
    let cloud_factor_2 = mix(1.0, storm_dark_2, cloud_threshold_2);

    let env_uv_3 = (in.world_position.xz - globals.env_origin) / globals.env_extent;
    var env_3 = globals.cloud_defaults.w;
    if env_uv_3.x >= 0.0 && env_uv_3.x <= 1.0 && env_uv_3.y >= 0.0 && env_uv_3.y <= 1.0 {
        env_3 = textureSample(cloud_env_texture, cloud_env_sampler, env_uv_3).r;
    }
    let cloud_uv_3 = in.world_position.xz * globals.cloud_uv_scale_3 + globals.cloud_offset_3;
    let detail_3 = textureSample(cloud_texture, cloud_sampler, cloud_uv_3).a;
    let density_3 = env_3 * detail_3;
    let cloud_threshold_3 = smoothstep(globals.cloud_coverage_3 - 0.015, globals.cloud_coverage_3 + 0.015, density_3);
    let cloud_depth_3 = clamp(env_3 / max(globals.env_typical.w, 0.05), 0.0, 2.0);
    let storm_dark_3 = clamp(mix(1.0, 0.94, cloud_depth_3), 0.0, 1.0);
    let cloud_factor_3 = mix(1.0, storm_dark_3, cloud_threshold_3);

    let cloud_factor = cloud_factor_0 * cloud_factor_1 * cloud_factor_2 * cloud_factor_3;

    let lit_color = base_color * (diffuse * shadow * cloud_factor + ambient) + specular * shadow * cloud_factor;
    let final_alpha = mix(0.4, 0.85, depth_t);

    return FragmentOutput(vec4<f32>(lit_color, final_alpha), vec4<f32>(perturbed_normal, final_alpha));
}