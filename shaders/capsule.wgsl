// Analytic tapered-capsule impostors for wood (spec §5).
//
// One instanced quad per branch, built to face the camera, with the round-cone
// solved per fragment. The silhouette is mathematically exact at any radius,
// which is the whole reason wood is not meshed: a grid-aligned or faceted trunk
// is the failure this pass exists to avoid.
//
// Orthographic makes this cheap - the ray direction is a uniform, not a
// per-fragment computation, and moving the quad along the view axis does not
// change its screen coverage.

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
    view_right: vec3<f32>,
    _pad7: f32,
    view_up: vec3<f32>,
    _pad8: f32,
};

@group(0) @binding(0) var<uniform> globals: GlobalUniforms;
@group(0) @binding(1) var cloud_texture: texture_2d<f32>;
@group(0) @binding(2) var cloud_sampler: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var visibility_mask: texture_3d<u32>;
@group(0) @binding(7) var cloud_env_texture: texture_2d<f32>;
@group(0) @binding(8) var cloud_env_sampler: sampler;

/// Must match `terrain.wgsl`, `detail_paint.wgsl` and `scatter.wgsl`. A fourth
/// copy because WGSL here has no include mechanism; the passes disagreeing
/// shows up as trees surviving where their ground went dark.
const CLARITY_RADIUS: u32 = 24u;

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

struct InstanceInput {
    @location(0) a: vec3<f32>,
    @location(1) ra: f32,
    @location(2) b: vec3<f32>,
    @location(3) rb: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    /// Ray origin for this fragment: its world position on the billboard.
    @location(0) ray_origin: vec3<f32>,
    @location(1) cap_a: vec3<f32>,
    @location(2) cap_b: vec3<f32>,
    @location(3) radii: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstanceInput) -> VertexOutput {
    var out: VertexOutput;

    // Cull the whole capsule on its midpoint, matching how the scatter pass
    // culls a prop at its anchor: a branch is one thing, and a per-fragment
    // march would pay the 64-step loop for every pixel of every trunk.
    let mid = (inst.a + inst.b) * 0.5;
    if !in_visible_volume(mid) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.ray_origin = vec3<f32>(0.0);
        out.cap_a = inst.a;
        out.cap_b = inst.b;
        out.radii = vec2<f32>(inst.ra, inst.rb);
        return out;
    }

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[vi];

    let maxr = max(inst.ra, inst.rb);
    let ea = inst.a - mid;
    // Half-extents of the capsule projected onto the view basis, grown by the
    // radius so the silhouette's bulge is inside the quad.
    let hu = abs(dot(ea, globals.view_right)) + maxr;
    let hv = abs(dot(ea, globals.view_up)) + maxr;

    // Pushed in front of the *whole* capsule, not just its radius.
    //
    // The quad is where each fragment's ray starts, so anything nearer the
    // camera than this plane solves to a negative `t` and is discarded as a
    // miss. A capsule's near end reaches `|dot(ea, view_dir)|` past its
    // midpoint - most of its length when it points at the camera - so omitting
    // that term sliced the front off every segment and deleted outright the
    // ones aimed at the viewer. Joint spheres were unaffected because `ea` is
    // zero for them, which is why branches degraded into strings of beads.
    //
    // Free under ortho: moving along the view axis changes the depth the ray
    // starts from, never the screen coverage.
    let plane = mid - globals.view_dir * (abs(dot(ea, globals.view_dir)) + maxr + 0.01);
    let world_pos = plane + globals.view_right * (c.x * hu) + globals.view_up * (c.y * hv);

    out.clip_position = globals.view_proj * vec4<f32>(world_pos, 1.0);
    out.ray_origin = world_pos;
    out.cap_a = inst.a;
    out.cap_b = inst.b;
    out.radii = vec2<f32>(inst.ra, inst.rb);
    return out;
}

/// Ray vs. the tangent cone between two spheres - or vs. a single sphere when
/// the endpoints coincide. Returns `vec4(t, normal)`, `t < 0.0` on a miss.
/// `rd` must be unit length.
///
/// **Cones carry no end caps.** Every joint sphere is emitted once as its own
/// degenerate instance, so a cone that also drew its caps would lay a second
/// copy of the same surface over the first. That duplication is what the
/// banding was. The cone body is tangent to both end spheres, so the union of
/// the two instance kinds is smooth across every joint.
fn intersect_wood(ro: vec3<f32>, rd: vec3<f32>, pa: vec3<f32>, pb: vec3<f32>, ra: f32, rb: f32) -> vec4<f32> {
    let miss = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    let ba = pb - pa;
    let oa = ro - pa;
    let m0 = dot(ba, ba);

    // A joint sphere.
    if m0 <= 1e-8 {
        let m3 = dot(rd, oa);
        let m5 = dot(oa, oa);
        let h = m3 * m3 - m5 + ra * ra;
        if h < 0.0 {
            return miss;
        }
        let t = -m3 - sqrt(h);
        return vec4<f32>(t, normalize(oa + t * rd));
    }

    let rr = ra - rb;
    let d2 = m0 - rr * rr;
    // One end sphere swallows the other, so there is no tangent cone between
    // them. The joint spheres already cover this segment.
    if d2 <= 0.0 {
        return miss;
    }

    let m1 = dot(ba, oa);
    let m2 = dot(ba, rd);
    let m3 = dot(rd, oa);
    let m5 = dot(oa, oa);

    let k2 = d2 - m2 * m2;
    let k1 = d2 * m3 - m1 * m2 + m2 * rr * ra;
    let k0 = d2 * m5 - m1 * m1 + m1 * rr * ra * 2.0 - m0 * ra * ra;

    // Degenerate when the ray runs along the axis; the joint spheres cover it.
    if abs(k2) <= 1e-8 {
        return miss;
    }
    let h = k1 * k1 - k0 * k2;
    if h < 0.0 {
        return miss;
    }
    // Dividing by a negative `k2` reverses which root is nearer, and `k2` is
    // `m0*sin^2(theta) - rr^2` - negative whenever a branch points toward or
    // away from the camera. Taking the fixed root there returns the *back*
    // surface with an inward normal, which reads as a dark bite out of the
    // trunk beside every branch aimed at the viewer.
    let sq = sqrt(h);
    let t = select((-k1 + sq) / k2, (-k1 - sq) / k2, k2 > 0.0);
    // Outside the band between the two tangency circles is the spheres' job.
    let y = m1 - ra * rr + t * m2;
    if y <= 0.0 || y >= d2 {
        return miss;
    }
    return vec4<f32>(t, normalize(d2 * (oa + t * rd) - ba * y));
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
    let biased_depth = current_depth - 0.004;
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
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    let rd = globals.view_dir;
    let hit = intersect_wood(in.ray_origin, rd, in.cap_a, in.cap_b, in.radii.x, in.radii.y);
    if hit.x < 0.0 {
        discard;
    }
    let world_pos = in.ray_origin + rd * hit.x;
    let n = hit.yzw;

    if globals.clip_enabled != 0u {
        if world_pos.x > globals.clip_max.x || world_pos.y > globals.clip_max.y || world_pos.z > globals.clip_max.z
        || world_pos.x < globals.clip_min.x || world_pos.y < globals.clip_min.y || world_pos.z < globals.clip_min.z {
            discard;
        }
    }

    // Cloud shadow, matching `scatter.wgsl` and `terrain.wgsl`: four layers,
    // each a coverage threshold against an environment density, multiplied
    // together. Applied to the diffuse term only - ambient is skylight and does
    // not go away under a cloud.
    let env_uv = (world_pos.xz - globals.env_origin) / globals.env_extent;
    let inside_env = env_uv.x >= 0.0 && env_uv.x <= 1.0 && env_uv.y >= 0.0 && env_uv.y <= 1.0;

    var env_0 = globals.cloud_defaults.x;
    var env_1 = globals.cloud_defaults.y;
    var env_2 = globals.cloud_defaults.z;
    var env_3 = globals.cloud_defaults.w;
    if inside_env {
        let e = textureSample(cloud_env_texture, cloud_env_sampler, env_uv).r;
        env_0 = e;
        env_1 = e;
        env_2 = e;
        env_3 = e;
    }

    let detail = vec4<f32>(
        textureSample(cloud_texture, cloud_sampler, world_pos.xz * globals.cloud_uv_scale_0 + globals.cloud_offset_0).r,
        textureSample(cloud_texture, cloud_sampler, world_pos.xz * globals.cloud_uv_scale_1 + globals.cloud_offset_1).g,
        textureSample(cloud_texture, cloud_sampler, world_pos.xz * globals.cloud_uv_scale_2 + globals.cloud_offset_2).b,
        textureSample(cloud_texture, cloud_sampler, world_pos.xz * globals.cloud_uv_scale_3 + globals.cloud_offset_3).a,
    );
    let env = vec4<f32>(env_0, env_1, env_2, env_3);
    let coverage = vec4<f32>(
        globals.cloud_coverage_0, globals.cloud_coverage_1,
        globals.cloud_coverage_2, globals.cloud_coverage_3,
    );
    let density = env * detail;
    let threshold = smoothstep(coverage - vec4<f32>(0.015), coverage + vec4<f32>(0.015), density);
    let cloud_depth = clamp(env / max(globals.env_typical, vec4<f32>(0.05)), vec4<f32>(0.0), vec4<f32>(2.0));
    // Per-layer darkening, thickest layer first - the same 0.55/0.68/0.85/0.94
    // ladder the sibling passes use.
    let dark = mix(vec4<f32>(1.0), vec4<f32>(0.55, 0.68, 0.85, 0.94), cloud_depth);
    let factors = mix(vec4<f32>(1.0), clamp(dark, vec4<f32>(0.0), vec4<f32>(1.0)), threshold);
    let cloud_factor = factors.x * factors.y * factors.z * factors.w;

    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);
    let shadow = compute_shadow(world_pos);
    let lit = globals.sun_color * n_dot_l * shadow * cloud_factor + globals.ambient_color * 0.6;

    // Bark. A constant until the material's color reaches this pass; wood is
    // render-delegated, so the terrain palette never sees it.
    let albedo = vec3<f32>(0.36, 0.26, 0.17);

    // Orthographic clip depth is already 0..1 (glam's `orthographic_rh`), and w
    // is 1, so this is exactly what the rasterizer would have written for a
    // triangle at the hit point.
    let clip = globals.view_proj * vec4<f32>(world_pos, 1.0);

    var out: FragmentOutput;
    out.color = vec4<f32>(albedo * lit, 1.0);
    // 0.9 is the wood class. The alpha channel is a surface class, not a mask:
    // 0.0 sky, 0.5 vegetation, 0.9 wood, 1.0 terrain. Wood needs its own value
    // because the outline pass's ownership rules assume flat axis-aligned faces
    // and get a curved vertical object exactly backwards - see `outline.wgsl`.
    out.normal = vec4<f32>(n, 0.9);
    out.depth = clip.z / clip.w;
    return out;
}