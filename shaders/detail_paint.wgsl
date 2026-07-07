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

// Per-column paint data for this chunk.
struct Column {
    packed: u32,      // density (0-7) | species (8-15) | tint (16-23)
    surface_top: f32, // top-face height in voxel units
};

@group(1) @binding(0) var<storage, read> columns: array<Column>;

struct ChunkUniform {
    origin: vec3<f32>,
    voxel_scale: f32,
};

@group(1) @binding(1) var<uniform> chunk: ChunkUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) height: f32,
    @location(3) color: vec3<f32>,
};

const CHUNK_DIM: u32 = 32u;
const MAX_BLADES: u32 = 8u; // must match MAX_BLADES_PER_COLUMN in detail_paint_pass.rs
const TAU: f32 = 6.28318530718;

fn hash_u32(h_in: u32) -> u32 {
    var h = h_in;
    h ^= h >> 16u;
    h = h * 0x45d9f3bu;
    h ^= h >> 16u;
    h = h * 0x45d9f3bu;
    h ^= h >> 16u;
    return h;
}

fn hash_pos(x: i32, z: i32, seed: u32) -> u32 {
    var h = u32(x) * 374761393u;
    h = h + u32(z) * 668265263u;
    h = h + seed * 1911520717u;
    return hash_u32(h);
}

fn hash01(h: u32) -> f32 {
    return f32(h & 0x00FFFFFFu) / 16777216.0;
}

fn hash_signed(h: u32) -> f32 {
    return hash01(h) * 2.0 - 1.0;
}

fn base_color_for(species: u32) -> vec3<f32> {
    switch (species) {
        case 2u: { return vec3<f32>(0.45, 0.52, 0.20); } // dry / golden
        default: { return vec3<f32>(0.20, 0.55, 0.22); } // meadows green
    }
}

@vertex
fn vs_main(vertex: VertexInput, @builtin(instance_index) inst: u32) -> VertexOutput {
    var out: VertexOutput;

    let col_index = inst / MAX_BLADES;
    let blade_index = inst % MAX_BLADES;

    // Guard against out-of-range column reads (e.g. an instance-count vs
    // MAX_BLADES mismatch), which would scatter blades on garbage data.
    if col_index >= arrayLength(&columns) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.world_position = vec3<f32>(0.0);
        out.normal = vec3<f32>(0.0, 1.0, 0.0);
        out.height = 0.0;
        out.color = vec3<f32>(0.0);
        return out;
    }

    let col = columns[col_index];
    let density = col.packed & 0xFFu;
    let species = (col.packed >> 8u) & 0xFFu;
    let tint = (col.packed >> 16u) & 0xFFu;

    // Density-driven blade count: cull blades beyond the active count.
    let active_count = max(1u, u32(floor(f32(density) / 255.0 * f32(MAX_BLADES) + 0.5)));
    if density == 0u || blade_index >= active_count {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0); // off-screen / culled
        out.world_position = vec3<f32>(0.0);
        out.normal = vec3<f32>(0.0, 1.0, 0.0);
        out.height = 0.0;
        out.color = vec3<f32>(0.0);
        return out;
    }

    let lx = col_index % CHUNK_DIM;
    let lz = col_index / CHUNK_DIM;
    let vs = chunk.voxel_scale;
    let base_vx = i32(round(chunk.origin.x / vs)) + i32(lx);
    let base_vz = i32(round(chunk.origin.z / vs)) + i32(lz);

    // Per-blade deterministic variation from world column + blade index.
    let h1 = hash_pos(base_vx, base_vz, blade_index * 3u + 1u);
    let h2 = hash_pos(base_vx, base_vz, blade_index * 3u + 2u);
    let h3 = hash_pos(base_vx, base_vz, blade_index * 3u + 3u);
    let h4 = hash_pos(base_vx, base_vz, blade_index * 7u + 11u);

    let scale = 0.7 + hash01(h1) * 0.6;
    let rotation = hash01(h2) * TAU;
    let jitter_x = hash_signed(h3) * 0.35;
    let jitter_z = hash_signed(hash_u32(h3)) * 0.35;
    let blade_phase = hash01(h4) * TAU;

    // Rotate the blade plane around Y, scale height.
    let cos_r = cos(rotation);
    let sin_r = sin(rotation);
    var local = vec3<f32>(
        vertex.position.x * cos_r - vertex.position.z * sin_r,
        vertex.position.y * scale,
        vertex.position.x * sin_r + vertex.position.z * cos_r,
    );

    // Wind sway: low-frequency group phase from world position so neighbors
    // sway together (mirrors the retired grass shader)
    let height_norm = vertex.uv.y;
    let raw_wind = length(globals.wind_vector);
    let wind_strength = min(raw_wind, 3.0);
    var wind_dir = vec2<f32>(1.0, 0.0);
    if raw_wind > 0.01 {
        wind_dir = globals.wind_vector / raw_wind;
    }
    let col_world = vec2<f32>(f32(base_vx), f32(base_vz)) * vs;
    let group_phase = dot(col_world, vec2<f32>(0.07, 0.03));
    let phase = group_phase + blade_phase * 0.3;
    let sway = sin(globals.time * 2.0 + phase) * 0.08
             + sin(globals.time * 4.5 + phase * 2.1) * 0.03;
    let height_factor = height_norm * height_norm;
    let displacement = wind_dir * wind_strength * sway * height_factor;
    local.x += displacement.x;
    local.z += displacement.y;
    local.y -= length(displacement) * 0.15 * height_norm;
    local.y = max(local.y, 0.0);

    // Column-centered base on the surface top face, plus per-blade jitter.
    let base = chunk.origin + vec3<f32>(
        (f32(lx) + 0.5) * vs + jitter_x * vs,
        col.surface_top * vs,
        (f32(lz) + 0.5) * vs + jitter_z * vs,
    );
    let world_pos = base + local;

    // Color: species base, tint brightness nudge, per-blade hash variation.
    var color = base_color_for(species);
    let bright = 0.85 + f32(tint) / 255.0 * 0.3;
    let cv = hash01(hash_u32(h1)) * 0.15 - 0.075;
    color = clamp(color * bright + vec3<f32>(cv, cv, cv * 0.5), vec3<f32>(0.0), vec3<f32>(1.0));

    out.clip_position = globals.view_proj * vec4<f32>(world_pos, 1.0);
    out.world_position = world_pos;
    out.normal = normalize(vec3<f32>(-displacement.x * 0.2, 1.0, -displacement.y * 0.2));
    out.height = height_norm;
    out.color = color;
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

    let base_color = mix(in.color, in.color * 1.3, in.height * 0.5);

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

    // normal.w = 0.5 flags vegetation so the outline pass skips it.
    return FragmentOutput(vec4<f32>(lit_color, 1.0), vec4<f32>(n, 0.5));
}
