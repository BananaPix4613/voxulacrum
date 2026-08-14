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

@group(0) @binding(0) var<uniform> globals: GlobalUniforms;
@group(0) @binding(1) var cloud_texture: texture_2d<f32>;
@group(0) @binding(2) var cloud_sampler: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var visibility_mask: texture_3d<u32>;
@group(0) @binding(7) var cloud_env_texture: texture_2d<f32>;
@group(0) @binding(8) var cloud_env_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct InstanceInput {
    @location(2) inst_position: vec3<f32>,
    @location(3) rotation_y: f32,
    @location(4) color: vec3<f32>,
    @location(5) scale: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

/// Flood cost past which a cell is outside the readable pocket. Must match
/// `terrain.wgsl` and `detail_paint.wgsl` - three copies because these shaders
/// have no include mechanism, and the passes disagreeing shows up as props
/// surviving where their ground went dark.
const CLARITY_RADIUS: u32 = 24u;

/// Whether the cell containing `sample_pos` is inside the player's visible
/// volume, read from the same 64-cube mask the terrain pass reads. True whenever
/// the mask is off, so nothing changes above ground.
fn in_visible_volume(sample_pos: vec3<f32>) -> bool {
    if globals.mask_enabled != 1u {
        return true;
    }
    let cell = floor(sample_pos);
    let ai = vec3<i32>(cell) - vec3<i32>(globals.mask_origin);
    if ai.x < 0 || ai.x >= 64 || ai.y < 0 || ai.y >= 64 || ai.z < 0 || ai.z >= 64 {
        return false;
    }
    let s = textureLoad(visibility_mask, ai, 0).r & 0x7Fu;
    if s == 0u || s > CLARITY_RADIUS + 1u {
        return false;
    }

    // Cost-priority march.
    let base = cell + vec3<f32>(0.5, 0.5, 0.5);
    for (var i = 1; i <= 64; i = i + 1) {
        let p = base + globals.view_dir * (f32(i) * 0.5);
        let c = vec3<i32>(floor(p)) - vec3<i32>(globals.mask_origin);
        if c.x < 0 || c.x >= 64 || c.y < 0 || c.y >= 64 || c.z < 0 || c.z >= 64 {
            break;
        }
        let raw = textureLoad(visibility_mask, c, 0).r;
        if (raw & 0x80u) != 0u {
            continue;
        }
        let behind = raw & 0x7Fu;
        if behind != 0u && behind < s {
            return false;
        }
    }
    return true;
}

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    var out: VertexOutput;
    let cos_r = cos(instance.rotation_y);
    let sin_r = sin(instance.rotation_y);

    let scaled = vertex.position * instance.scale;
    let rotated = vec3<f32>(
        scaled.x * cos_r - scaled.z * sin_r,
        scaled.y,
        scaled.x * sin_r + scaled.z * cos_r,
    );
    let world_pos = instance.inst_position + rotated;

    // Cull at the anchor, not the vertex: a prop culls as a whole thing, and its
    // anchor is the cell the design (§ 8) says owns it.
    if !in_visible_volume(instance.inst_position + vec3<f32>(0.0, 0.25, 0.0)) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.world_position = vec3<f32>(0.0);
        out.normal = vec3<f32>(0.0, 1.0, 0.0);
        out.color = vec3<f32>(0.0);
        return out;
    }

    // Uniform scale -> normal only needs the same Y rotation.
    let rnormal = vec3<f32>(
        vertex.normal.x * cos_r - vertex.normal.z * sin_r,
        vertex.normal.y,
        vertex.normal.x * sin_r + vertex.normal.z * cos_r,
    );

    out.clip_position = globals.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_position = world_pos;
    out.normal = rnormal;
    out.color = instance.color;
    return out;
}

fn compute_shadow(world_pos: vec3<f32>) -> f32 {
    let light_space_pos = globals.light_space_matrix * vec4<f32>(world_pos, 1.0);
    let proj_coords = light_space_pos.xyz / light_space_pos.w;
    let shadow_uv = vec2<f32>(proj_coords.x * 0.5 + 0.5, proj_coords.y * -0.5 + 0.5);
    let current_depth = proj_coords.z;
    if shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0 {
        return 1.0;
    }
    if current_depth > 1.0 || current_depth < 0.0 {
        return 1.0;
    }
    let bias = 0.004;
    let biased_depth = current_depth - bias;
    let texel_size = 1.0 / 4096.0;
    var shadow_sum = 0.0;
    for (var x: i32 = -1; x <= 1; x++) {
        for (var y: i32 = -1; y <= 1; y++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            shadow_sum += textureSampleCompare(shadow_map, shadow_sampler, shadow_uv + offset, biased_depth);
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
    if globals.clip_enabled != 0u {
        let wp = in.world_position;
        if wp.x > globals.clip_max.x || wp.y > globals.clip_max.y || wp.z > globals.clip_max.z
        || wp.x < globals.clip_min.x || wp.y < globals.clip_min.y || wp.z < globals.clip_min.z {
            discard;
        }
    }

    let n = normalize(in.normal);
    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);
    let diffuse = globals.sun_color * n_dot_l;
    let ambient = globals.ambient_color * 0.6;

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

    let shadow = compute_shadow(in.world_position);
    let lit_color = in.color * (diffuse * shadow * cloud_factor + ambient);

    // normal.w = 0.5 flags this as foliage so the outline pass skips it.
    return FragmentOutput(vec4<f32>(lit_color, 1.0), vec4<f32>(n, 0.5));
}
