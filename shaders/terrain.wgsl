const AO_MIN: f32 = 0.3;
const AO_STRENGTH: f32 = 3.5;

// Debug mode constants
const DEBUG_NONE: u32 = 0u;
const DEBUG_MATERIAL_ID: u32 = 1u;
const DEBUG_AO_ONLY: u32 = 2u;
const DEBUG_NORMALS: u32 = 3u;
const DEBUG_GREEDY: u32 = 4u;

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
    @location(4) material_id: u32,
    @location(5) cell_flags: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) ao: f32,
    @location(4) @interpolate(flat) material_id: u32,
    @location(6) @interpolate(flat) cell_flags: u32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.normal = in.normal;
    out.color = in.color;
    out.ao = in.ao;
    out.material_id = in.material_id;
    out.cell_flags = in.cell_flags;
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

    let bias = 0.002;
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

// Debug palette: 9 high-contrast colors for material IDs 0-8
fn debug_material_color(id: u32) -> vec3<f32> {
    switch (id) {
        case 0u: { return vec3<f32>(0.2, 0.2, 0.2); }   // Air (dark grey)
        case 1u: { return vec3<f32>(1.0, 0.3, 0.3); }   // Limestone (red)
        case 2u: { return vec3<f32>(0.3, 1.0, 0.3); }   // Granite (green)
        case 3u: { return vec3<f32>(0.3, 0.3, 1.0); }   // Soil (blue)
        case 4u: { return vec3<f32>(1.0, 1.0, 0.3); }   // Clay (yellow)
        case 5u: { return vec3<f32>(0.3, 1.0, 1.0); }   // Sand (cyan)
        case 6u: { return vec3<f32>(1.0, 0.3, 1.0); }   // Grass Soil (magenta)
        case 7u: { return vec3<f32>(1.0, 0.6, 0.2); }   // Water (orange)
        case 8u: { return vec3<f32>(0.6, 0.2, 1.0); }   // Gravel (purple)
        default: { return vec3<f32>(1.0, 1.0, 1.0); }   // Unknown (white)
    }
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

    let n = normalize(in.normal);

    // --- Debug modes ---
    if globals.debug_mode == DEBUG_MATERIAL_ID {
        return FragmentOutput(vec4<f32>(debug_material_color(in.material_id), 1.0), vec4<f32>(n, 1.0));
    }
    if globals.debug_mode == DEBUG_AO_ONLY {
        return FragmentOutput(vec4<f32>(vec3<f32>(in.ao), 1.0), vec4<f32>(n, 1.0));
    }
    if globals.debug_mode == DEBUG_NORMALS {
        let normal_color = n * 0.5 + 0.5;
        return FragmentOutput(vec4<f32>(normal_color, 1.0), vec4<f32>(n, 1.0));
    }
    if globals.debug_mode == DEBUG_GREEDY {
        if (in.cell_flags & 1u) == 1u {
            return FragmentOutput(vec4<f32>(0.2, 0.8, 0.2, 1.0), vec4<f32>(n, 1.0));
        } else {
            return FragmentOutput(vec4<f32>(0.2, 0.2, 0.8, 1.0), vec4<f32>(n, 1.0));
        }
    }

    // --- Normal rendering ---
    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);
    let diffuse = globals.sun_color * n_dot_l;
    let ambient = globals.ambient_color * 0.4;
    let ao_factor = mix(AO_MIN, 1.0, in.ao);

    let cloud_uv = in.world_position.xz * 0.015 + globals.cloud_shadow_offset;
    let cloud_sample = textureSample(cloud_texture, cloud_sampler, cloud_uv).r;
    let cloud_threshold = smoothstep(globals.cloud_coverage - 0.15, globals.cloud_coverage + 0.15, cloud_sample);
    let cloud_factor = mix(0.25, 1.0, cloud_threshold);

    let shadow = compute_shadow(in.world_position);

    var final_color = in.color * (diffuse * shadow * cloud_factor + ambient * ao_factor * AO_STRENGTH + ambient * (1.0 - AO_STRENGTH));

    // Directional AO
    // let view_alignment = dot(n, vec3<f32>(0.0, 1.0, 0.0));
    // let ortho_ao = mix(1.0 - globals.ortho_ao_strength, 1.0, view_alignment * 0.5 + 0.5);
    // final_color = final_color * ortho_ao;

    return FragmentOutput(vec4<f32>(final_color, 1.0), vec4<f32>(n, 1.0));
}