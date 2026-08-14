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

/// Flood cost past which a cell is outside the readable pocket. Must match
/// `terrain.wgsl` - the two passes disagreeing means foliage survives where the
/// ground under it went dark, or the reverse.
const CLARITY_RADIUS: u32 = 24u;

/// Whether the cell containing `sample_pos` is inside the player's visible
/// volume, read from the same 64-cube mask the terrain pass reads.
///
/// True whenever the mask is off, so nothing changes above ground. A position
/// outside the window is not visible: the window is centered on the player, so
/// anything beyond it is far enough to be occluded by definition.
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

    // Cull against the air cell the blades stand in, at the column center rather
    // than per blade: a column has to cull as a unit, or half a tuft vanishes
    // and reads as a glitch. The 0.25 lifts the sample off the exact top face,
    // which lands on a cell boundary for a full cube.
    let surface_cell = chunk.origin + vec3<f32>(
        (f32(lx) + 0.5) * vs,
        col.surface_top * vs + 0.25,
        (f32(lz) + 0.5) * vs,
    );
    if !in_visible_volume(surface_cell) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.world_position = vec3<f32>(0.0);
        out.normal = vec3<f32>(0.0, 1.0, 0.0);
        out.height = 0.0;
        out.color = vec3<f32>(0.0);
        return out;
    }

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
    let lit_color = base_color * (diffuse * shadow * cloud_factor + ambient);

    // normal.w = 0.5 flags vegetation so the outline pass skips it.
    return FragmentOutput(vec4<f32>(lit_color, 1.0), vec4<f32>(n, 0.5));
}
