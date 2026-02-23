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

// Per-vertex grass blade geometry
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

// Per-instance data
struct InstanceInput {
    @location(4) inst_position: vec3<f32>,
    @location(5) scale: f32,
    @location(6) rotation: f32,
    @location(7) blade_phase: f32,
    @location(8) terrain_color: vec3<f32>,
    @location(9) _pad1: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) terrain_color: vec3<f32>,
};

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    var out: VertexOutput;

    let cos_r = cos(instance.rotation);
    let sin_r = sin(instance.rotation);
    var local_pos = vec3<f32>(
        vertex.position.x * cos_r - vertex.position.z * sin_r,
        vertex.position.y * instance.scale,
        vertex.position.x * sin_r + vertex.position.z * cos_r,
    );

    let vertex_height = vertex.uv.y;
    let raw_wind_strength = length(globals.wind_vector);
    let wind_strength = min(raw_wind_strength, 3.0);

    var wind_dir = vec2<f32>(1.0, 0.0);
    if raw_wind_strength > 0.01 {
        wind_dir = globals.wind_vector / raw_wind_strength;
    }

    // Group sway: low spatial frequency so nearby blades sway together
    let group_phase = dot(instance.inst_position.xz, vec2<f32>(0.07, 0.03));
    let phase = group_phase + instance.blade_phase * 0.3;

    // Gentle sway
    let sway = sin(globals.time * 2.0 + phase) * 0.08
             + sin(globals.time * 4.5 + phase * 2.1) * 0.03;

    let height_factor = vertex_height * vertex_height;
    let displacement = wind_dir * wind_strength * sway * height_factor;

    local_pos.x += displacement.x;
    local_pos.z += displacement.y;
    local_pos.y -= length(displacement) * 0.15 * vertex_height;
    local_pos.y = max(local_pos.y, 0.0);

    let world_pos = instance.inst_position + local_pos;
    out.world_position = world_pos;
    out.clip_position = globals.view_proj * vec4<f32>(world_pos, 1.0);
    out.normal = normalize(vec3<f32>(-displacement.x * 0.2, 1.0, -displacement.y * 0.2));
    out.uv = vertex.uv;
    out.terrain_color = instance.terrain_color;

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

    let bias = 0.004;
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
    let base_color = mix(in.terrain_color, in.terrain_color * 1.3, in.uv.y * 0.5);

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

    let lit_color = base_color * (diffuse * shadow * cloud_factor + ambient);

    return vec4<f32>(lit_color, 1.0);
}