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
    cloud_offset_0: vec2<f32>, cloud_coverage_0: f32, cloud_uv_scale_0: f32,
    cloud_offset_1: vec2<f32>, cloud_coverage_1: f32, cloud_uv_scale_1: f32,
    cloud_offset_2: vec2<f32>, cloud_coverage_2: f32, cloud_uv_scale_2: f32,
    cloud_offset_3: vec2<f32>, cloud_coverage_3: f32, cloud_uv_scale_3: f32,
    env_origin: vec2<f32>, env_extent: f32, _pad_env: f32,
    cloud_defaults: vec4<f32>,
    env_typical: vec4<f32>,
    edge_strength: f32,
    ortho_ao_strength: f32,
    clip_enabled: u32,
    _pad_a: f32,
    clip_min: vec3<f32>, _pad3: f32,
    clip_max: vec3<f32>, _pad4: f32,
    mask_origin: vec3<f32>, mask_enabled: u32,
    view_dir: vec3<f32>, _pad5: f32,
    render_size: vec2<f32>, volume_radius: f32, _pad6: f32,
};

struct WaterUniforms {
    inv_view_proj: mat4x4<f32>,
    params: vec4<f32>,  // x=refraction, y=caustic_scale, z=reflection, w=flow_scroll
    params2: vec4<f32>, // x=depth_fade, y=wave_strength, z=foam_width
};

@group(0) @binding(0) var<uniform> globals: GlobalUniforms;
@group(0) @binding(1) var cloud_texture: texture_2d<f32>;
@group(0) @binding(2) var cloud_sampler: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var visibility_mask: texture_3d<u32>;
@group(0) @binding(7) var cloud_env_texture: texture_2d<f32>;
@group(0) @binding(8) var cloud_env_sampler: sampler;

@group(1) @binding(0) var<uniform> water: WaterUniforms;
@group(1) @binding(1) var scene_color: texture_2d<f32>;
@group(1) @binding(2) var scene_sampler: sampler;
@group(1) @binding(3) var scene_depth: texture_depth_2d;
@group(1) @binding(4) var reflection_color: texture_2d<f32>;
@group(1) @binding(5) var reflection_depth: texture_depth_2d;

const DEEP_COLOR: vec3<f32> = vec3<f32>(0.06, 0.20, 0.32);
const SHALLOW_COLOR: vec3<f32> = vec3<f32>(0.22, 0.48, 0.55);
const FOAM_WIDTH: f32 = 0.1;
const FOAM_COLOR: vec3<f32> = vec3<f32>(0.90, 0.96, 1.0);
const WATER_ABSORB: vec3<f32> = vec3<f32>(0.9, 0.45, 0.21); // per-channel absorption/voxel: red strongest

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) flow: vec2<f32>,
    @location(3) depth: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) flow: vec2<f32>,
    @location(3) depth: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    var pos = in.position;
    let phase = dot(pos.xz, vec2<f32>(0.7, 0.5)) + globals.time * 1.3;
    pos.y += (sin(phase) * 0.5 + 0.5) * 0.06 * clamp(in.depth, 0.0, 1.0);
    out.clip_position = globals.view_proj * vec4<f32>(pos, 1.0);
    out.world_position = pos;
    out.normal = normalize(in.normal);
    out.flow = in.flow;
    out.depth = in.depth;
    return out;
}

fn compute_shadow(world_pos: vec3<f32>) -> f32 {
    let lp = globals.light_space_matrix * vec4<f32>(world_pos, 1.0);
    let pc = lp.xyz / lp.w;
    let uv = vec2<f32>(pc.x * 0.5 + 0.5, pc.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || pc.z < 0.0 || pc.z > 1.0 {
        return 1.0;
    }
    return textureSampleCompare(shadow_map, shadow_sampler, uv, pc.z - 0.003);
}

// 4-layer cloud darkening, matching terrain.wgsl so water darkens under storms
// consistently with the ground it sits in.
fn cloud_light(wp: vec3<f32>) -> f32 {
    var factor = 1.0;

    let uv0 = (wp.xz - globals.env_origin) / globals.env_extent;
    var env0 = globals.cloud_defaults.x;
    if uv0.x >= 0.0 && uv0.x <= 1.0 && uv0.y >= 0.0 && uv0.y <= 1.0 {
        env0 = textureSample(cloud_env_texture, cloud_env_sampler, uv0).r;
    }
    let d0 = env0 * textureSample(cloud_texture, cloud_sampler, wp.xz * globals.cloud_uv_scale_0 + globals.cloud_offset_0).r;
    let th0 = smoothstep(globals.cloud_coverage_0 - 0.015, globals.cloud_coverage_0 + 0.015, d0);
    let cd0 = clamp(env0 / max(globals.env_typical.x, 0.05), 0.0, 2.0);
    factor = factor * mix(1.0, clamp(mix(1.0, 0.35, cd0), 0.0, 1.0), th0);

    let uv1 = (wp.xz - globals.env_origin) / globals.env_extent;
    var env1 = globals.cloud_defaults.y;
    if uv1.x >= 0.0 && uv1.x <= 1.0 && uv1.y >= 0.0 && uv1.y <= 1.0 {
        env1 = textureSample(cloud_env_texture, cloud_env_sampler, uv1).r;
    }
    let d1 = env1 * textureSample(cloud_texture, cloud_sampler, wp.xz * globals.cloud_uv_scale_1 + globals.cloud_offset_1).g;
    let th1 = smoothstep(globals.cloud_coverage_1 - 0.015, globals.cloud_coverage_1 + 0.015, d1);
    let cd1 = clamp(env1 / max(globals.env_typical.y, 0.05), 0.0, 2.0);
    factor = factor * mix(1.0, clamp(mix(1.0, 0.5, cd1), 0.0, 1.0), th1);

    let uv2 = (wp.xz - globals.env_origin) / globals.env_extent;
    var env2 = globals.cloud_defaults.z;
    if uv2.x >= 0.0 && uv2.x <= 1.0 && uv2.y >= 0.0 && uv2.y <= 1.0 {
        env2 = textureSample(cloud_env_texture, cloud_env_sampler, uv2).r;
    }
    let d2 = env2 * textureSample(cloud_texture, cloud_sampler, wp.xz * globals.cloud_uv_scale_2 + globals.cloud_offset_2).b;
    let th2 = smoothstep(globals.cloud_coverage_2 - 0.015, globals.cloud_coverage_2 + 0.015, d2);
    let cd2 = clamp(env2 / max(globals.env_typical.z, 0.05), 0.0, 2.0);
    factor = factor * mix(1.0, clamp(mix(1.0, 0.8, cd2), 0.0, 1.0), th2);

    let uv3 = (wp.xz - globals.env_origin) / globals.env_extent;
    var env3 = globals.cloud_defaults.w;
    if uv3.x >= 0.0 && uv3.x <= 1.0 && uv3.y >= 0.0 && uv3.y <= 1.0 {
        env3 = textureSample(cloud_env_texture, cloud_env_sampler, uv3).r;
    }
    let d3 = env3 * textureSample(cloud_texture, cloud_sampler, wp.xz * globals.cloud_uv_scale_3 + globals.cloud_offset_3).a;
    let th3 = smoothstep(globals.cloud_coverage_3 - 0.015, globals.cloud_coverage_3 + 0.015, d3);
    let cd3 = clamp(env3 / max(globals.env_typical.w, 0.05), 0.0, 2.0);
    factor = factor * mix(1.0, clamp(mix(1.0, 0.92, cd3), 0.0, 1.0), th3);

    return factor;
}

fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let w = water.inv_view_proj * ndc;
    return w.xyz / w.w;
}

fn caustic(p: vec2<f32>, dir: vec2<f32>, t: f32) -> f32 {
    let a = sin(dot(p, vec2<f32>(1.3, 0.9)) + t * 1.4 + dot(dir, p) * 0.4);
    let b = sin(dot(p, vec2<f32>(-0.7, 1.6)) - t * 1.1 + dot(dir, p) * 0.4);
    let c = sin(dot(p, vec2<f32>(1.9, -1.2)) + t * 0.8);
    let v = (a + b + c) / 3.0;
    return pow(clamp(v * 0.5 + 0.5, 0.0, 1.0), 3.0);
}

fn quantize(x: f32, steps: f32) -> f32 {
    return floor(x * steps + 0.5) / steps;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

/// The shell colour and tri-tonal axis ramp, matching `terrain.wgsl`. Water
/// outside the readable volume is part of the same dark shell the terrain draws
/// there, so it has to use the same numbers.
const VOID_MATERIAL: vec3<f32> = vec3<f32>(0.032, 0.032, 0.042);
const AXIS_TOP: f32 = 1.0;
const AXIS_SIDE_X: f32 = 0.75;
const AXIS_SIDE_Z: f32 = 0.60;
const AXIS_BOTTOM: f32 = 0.40;

/// Flood cost past which a cell is outside the readable pocket. Must match
/// `terrain.wgsl`, `detail_paint.wgsl` and `scatter.wgsl` - four copies, because
/// these shaders have no include mechanism. The passes disagreeing shows up as
/// water surviving where the ground under it went dark.
const CLARITY_RADIUS: u32 = 24u;

/// Whether the cell containing `sample_pos` is inside the player's readable
/// pocket. True whenever the mask is off, so nothing changes above ground.
fn in_visible_volume(sample_pos: vec3<f32>) -> bool {
    if globals.mask_enabled != 1u {
        return true;
    }
    let ai = vec3<i32>(floor(sample_pos)) - vec3<i32>(globals.mask_origin);
    if ai.x < 0 || ai.x >= 64 || ai.y < 0 || ai.y >= 64 || ai.z < 0 || ai.z >= 64 {
        return false;
    }
    let s = textureLoad(visibility_mask, ai, 0).r & 0x7Fu;
    return s != 0u && s <= CLARITY_RADIUS + 1u;
}

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    if globals.clip_enabled != 0u {
        let wp = in.world_position;
        if wp.x > globals.clip_max.x || wp.y > globals.clip_max.y || wp.z > globals.clip_max.z
        || wp.x < globals.clip_min.x || wp.y < globals.clip_min.y || wp.z < globals.clip_min.z {
            discard;
        }
    }

    // The air cell in front of the surface, which for water is the one above it.
    // Per fragment, not per mesh: a water surface spans many cells and only some
    // of them are in the pocket.
    //
    // Outside the pocket water joins the dark shell rather than disappearing -
    // a lake that vanished while the ground under it stayed read as a hole in
    // the world.
    if !in_visible_volume(in.world_position + normalize(in.normal) * 0.5) {
        let n = normalize(in.normal);
        var axis_shade = AXIS_TOP;
        if abs(n.y) < 0.5 {
            axis_shade = select(AXIS_SIDE_Z, AXIS_SIDE_X, abs(n.x) > abs(n.z));
        } else if n.y < 0.0 {
            axis_shade = AXIS_BOTTOM;
        }
        let shade = 0.45 + 0.55 * axis_shade;
        // Alpha 1.0, not water's usual 0.25: this is shell, and anything reading
        // the normal target should treat it the way it treats shell terrain.
        return FragmentOutput(vec4<f32>(VOID_MATERIAL * shade, 1.0), vec4<f32>(n, 1.0));
    }

    // Sample the copied scene by this fragment's own pixel. The scene renders into
    // the full alloc-sized target (no viewport), so the divisor must be the actual
    // texture size, NOT globals.render_size (the logical view size, which changes
    // every frame with zoom/pan). This is what keeps water locked to the world
    // instead of sliding with the camera.
    let dims = vec2<f32>(textureDimensions(scene_color));
    let frag = in.clip_position.xy;
    let screen_uv = frag / dims;
    let scene_d = textureLoad(scene_depth, vec2<i32>(frag), 0);

    // Manual occlusion: terrain nearer than this water fragment hides it.
    if scene_d < in.clip_position.z - 0.0001 {
        discard;
    }
    let has_bg = scene_d < 1.0; // opaque geometry behind the water (vs cleared sky)

    // Wave normal, stretched + scrolled along flow. All world-space => camera-stable.
    let t = globals.time;
    let flow_speed = length(in.flow);
    var dir = vec2<f32>(0.15, 0.1);
    if flow_speed > 1e-3 { dir = in.flow / flow_speed; }
    let wp2 = in.world_position.xz;
    let scroll = dir * t * (1.0 + flow_speed * 3.0) * water.params.w;
    let r1 = sin(dot(wp2, vec2<f32>(0.8, 0.6)) - dot(scroll, vec2<f32>(2.0, 2.0)));
    let r2 = sin(dot(wp2, vec2<f32>(-0.5, 1.1)) - dot(scroll, vec2<f32>(3.0, 3.0)) + 1.3);
    let wave = (r1 * 0.6 + r2 * 0.4) * water.params2.y;
    let n = normalize(in.normal + vec3<f32>(wave * dir.x + r1 * 0.05, 0.0, wave * dir.y + r2 * 0.05));

    // Thickness (world-space) from the sampled scene depth behind the surface.
    let depth_fade = max(water.params2.x, 0.5);
    var thickness = depth_fade;
    if has_bg {
        let behind = world_from_depth(screen_uv, scene_d);
        thickness = max(in.world_position.y - behind.y, 0.0);
    }
    let depth_t = clamp(max(thickness, in.depth) / depth_fade, 0.0, 1.0);

    // Refraction: offset-sample the scene behind the water. Guard the offset so it
    // never pulls in a sample above the water line, and skip it over open sky.
    var refracted: vec3<f32>;
    if has_bg {
        let refr_amt = water.params.x * (0.4 + 0.6 * depth_t);
        let refr_uv = clamp(screen_uv + n.xz * refr_amt, vec2<f32>(0.0), vec2<f32>(1.0));
        let refr_d = textureLoad(scene_depth, vec2<i32>(refr_uv * dims), 0);
        var uv_used = refr_uv;
        if refr_d >= 1.0 || world_from_depth(refr_uv, refr_d).y > in.world_position.y {
            uv_used = screen_uv;
        }
        refracted = textureSampleLevel(scene_color, scene_sampler, uv_used, 0.0).rgb;
    } else {
        refracted = textureSampleLevel(scene_color, scene_sampler, screen_uv, 0.0).rgb;
    }

    let depth_v = max(thickness, in.depth);
    let transmit = exp(-WATER_ABSORB * depth_v); // fraction of the bottom surviving, per channel
    let scatter = mix(SHALLOW_COLOR, DEEP_COLOR, depth_t); // water's own color with depth
    var col = mix(scatter, refracted * transmit, transmit);

    // Lighting (sun == sky in top-down), dimmed by cast + cloud shadow.
    let cloud_factor = cloud_light(in.world_position);
    let sun = normalize(globals.sun_direction);
    let shadow = compute_shadow(in.world_position);
    let sun_amt = max(dot(in.normal, sun), 0.0) * shadow * cloud_factor; // in.normal instead of n to prevent palette issues

    // Caustics (world-space, drift downstream).
    //let ca = caustic(wp2 * 0.9, dir, t);
    //let caustic_amt = quantize(ca, 3.0) * (1.0 - depth_t) * sun_amt * 0.15;
    //col += vec3<f32>(0.55, 0.8, 0.7) * caustic_amt;

    // --- Planar reflection (samples the mirrored reflection pass) ---
    // One reflected render (across reference plane P = params2.w) serves water at any
    // height: in ortho, the reflection across THIS fragment's water height h is that
    // same render shifted on screen by 2*(h - P)*(view_proj column 1). Sample at the
    // shifted UV, then keep it only if the reflected sample's un-mirrored height is
    // above h (so below-water geometry can't leak in).
    let refl_plane = water.params2.w;
    let h = in.world_position.y;
    let ndc_off = 2.0 * (h - refl_plane) * globals.view_proj[1].xy;
    // Small wave distortion so the reflection ripples instead of reading as a mirror.
    let refl_uv = screen_uv - vec2<f32>(ndc_off.x * 0.5, -ndc_off.y * 0.5) + n.xz * 0.01;

    let gn = normalize(in.normal);
    let view = -normalize(globals.view_dir);
    let sky = mix(globals.ambient_color, globals.sun_color, 0.6) * cloud_factor;
    var refl_color = sky;
    if refl_uv.x >= 0.0 && refl_uv.x <= 1.0 && refl_uv.y >= 0.0 && refl_uv.y <= 1.0 {
        let rdims = vec2<f32>(textureDimensions(reflection_depth));
        let refl_d = textureLoad(reflection_depth, vec2<i32>(refl_uv * rdims), 0);
        if refl_d < 1.0 {
            let refl_world = world_from_depth(refl_uv, refl_d);
            let orig_y = 2.0 * refl_plane - refl_world.y; // un-mirror the reflected sample
            if orig_y > h + 0.05 {
                refl_color = textureSampleLevel(reflection_color, scene_sampler, refl_uv, 0.0).rgb;
            }
        }
    }
    let fresnel = pow(1.0 - clamp(dot(gn, view), 0.0, 1.0), 4.0);
    let reflectivity = clamp(water.params.z * (0.15 + 0.85 * fresnel), 0.0, 0.6);

    // Foam: shoreline (thin thickness).
    let still = 1.0 - clamp(flow_speed * 2.0, 0.0, 1.0);
    let foam = select(0.0, (1.0 - smoothstep(0.0, FOAM_WIDTH, thickness)) * still, has_bg);
    col = mix(col, FOAM_COLOR, foam);

    // Final light + quantized cel sun glint.
    let light = globals.sun_color * sun_amt * 0.5 + globals.ambient_color * 0.6;
    col = col * light;
    let spec = pow(max(dot(reflect(-sun, in.normal), view), 0.0), 200.0);
    let crest = sin(dot(wp2, vec2<f32>(1.7, 1.3)) + t * 2.0)
              * sin(dot(wp2, vec2<f32>(-1.1, 2.1)) - t * 1.5);
    let sparkle = step(1.0, spec * step(0.2, crest));
    col += globals.sun_color * sparkle * shadow * cloud_factor * 0.4;

    // Reflection composited over the lit water
    col = mix(col, refl_color, reflectivity);

    // Opaque output:
    return FragmentOutput(vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0), vec4<f32>(n, 0.25));
}