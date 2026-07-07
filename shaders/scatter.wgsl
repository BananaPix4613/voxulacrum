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

@group(0) @binding(0) var<uniform> globals: GlobalUniforms;
@group(0) @binding(1) var cloud_texture: texture_2d<f32>;
@group(0) @binding(2) var cloud_sampler: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;

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

    let cloud_uv = in.world_position.xz * 0.015 + globals.cloud_shadow_offset;
    let cloud_sample = textureSample(cloud_texture, cloud_sampler, cloud_uv).r;
    let cloud_threshold = smoothstep(globals.cloud_coverage - 0.15, globals.cloud_coverage + 0.15, cloud_sample);
    let cloud_factor = mix(0.45, 1.0, cloud_threshold);

    let shadow = compute_shadow(in.world_position);
    let lit_color = in.color * (diffuse * shadow * cloud_factor + ambient);

    // normal.w = 0.5 flags this as foliage so the outline pass skips it.
    return FragmentOutput(vec4<f32>(lit_color, 1.0), vec4<f32>(n, 0.5));
}
