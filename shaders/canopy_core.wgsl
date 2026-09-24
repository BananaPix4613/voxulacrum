// Analytic ellipsoid impostors for canopy lobes (spec §6.2).
//
// The same construction as `capsule.wgsl`: one instanced quad per lobe, built
// to face the camera, with the surface solved per fragment and true depth
// written. What differs is only the primitive.
//
// §6.2 specifies these as depth-only. They write color until §7's leaf scatter
// exists to cover them - an invisible occluder with nothing drawn over it takes
// a canopy-shaped bite out of everything behind it. When leaves land, the color
// targets' write mask goes to `empty()` in `create_canopy_core_pipeline` and
// this shader is unchanged.

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
@group(0) @binding(6) var visibility_mask: texture_3d<u32>;

/// Must match `terrain.wgsl`, `detail_paint.wgsl`, `scatter.wgsl` and
/// `capsule.wgsl`. A fifth copy because WGSL here has no include mechanism.
const CLARITY_RADIUS: u32 = 24u;

/// §6.2's shrink. The core sits inside the leaf shell so leaves keep the
/// silhouette; too large eats them, too small loses the interior culling.
const CORE_SCALE: f32 = 0.75;

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
    @location(0) center: vec3<f32>,
    @location(1) radii: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    /// Ray origin for this fragment: its world position on the billboard.
    @location(0) ray_origin: vec3<f32>,
    @location(1) center: vec3<f32>,
    /// Already shrunk by `CORE_SCALE`, so the fragment stage and the quad that
    /// covers it can never disagree about how big this lobe is.
    @location(2) radii: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstanceInput) -> VertexOutput {
    var out: VertexOutput;
    let r = inst.radii * CORE_SCALE;
    out.center = inst.center;
    out.radii = r;

    // Culled on the center, as the capsule pass culls a branch on its midpoint:
    // a lobe is one thing, and the mask walk is far too expensive per fragment.
    if !in_visible_volume(inst.center) {
        out.clip_position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        out.ray_origin = vec3<f32>(0.0);
        return out;
    }

    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[vi];

    // An axis-aligned ellipsoid's reach along any unit direction `u` is
    // `length(radii * u)` - its support function. Exact rather than a bounding
    // sphere, which would waste most of a quad on an elongated lobe.
    let hu = length(r * globals.view_right);
    let hv = length(r * globals.view_up);

    // Pushed in front of the ellipsoid's *full* reach along the view axis, not
    // its mean radius. The quad is where each ray starts, so anything nearer the
    // camera than this plane solves to a negative `t` and is discarded - the
    // failure that sliced the front off every capsule until the same term was
    // added there. Free under ortho: sliding along the view axis changes where
    // the ray starts, never the screen coverage.
    let plane = inst.center - globals.view_dir * (length(r * globals.view_dir) + 0.01);
    let world_pos = plane + globals.view_right * (c.x * hu) + globals.view_up * (c.y * hv);

    out.clip_position = globals.view_proj * vec4<f32>(world_pos, 1.0);
    out.ray_origin = world_pos;
    return out;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    let rd = globals.view_dir;

    // Ray vs. ellipsoid, solved in the unit-sphere space the radii define.
    //
    // No root-selection hazard here, unlike the round cone: `a` is
    // `dot(rd_1, rd_1)` and is strictly positive, so `-b - sqrt(h)` is always
    // the near root. The capsule's `k2` can go negative and silently return the
    // back surface; this cannot.
    let ro_l = (in.ray_origin - in.center) / in.radii;
    let rd_l = rd / in.radii;
    let a = dot(rd_l, rd_l);
    let b = dot(ro_l, rd_l);
    let k = dot(ro_l, ro_l) - 1.0;
    let h = b * b - a * k;
    if h < 0.0 {
        discard;
    }
    let t = (-b - sqrt(h)) / a;
    if t < 0.0 {
        discard;
    }
    let world_pos = in.ray_origin + rd * t;

    if globals.clip_enabled != 0u {
        if world_pos.x > globals.clip_max.x || world_pos.y > globals.clip_max.y || world_pos.z > globals.clip_max.z
        || world_pos.x < globals.clip_min.x || world_pos.y < globals.clip_min.y || world_pos.z < globals.clip_min.z {
            discard;
        }
    }

    // The gradient of `|(p - c) / r|^2`, which is the surface normal wherever
    // the radii differ - normalizing `p - c` would be the sphere's normal and
    // would light an elongated lobe as though it were round.
    let n = normalize((world_pos - in.center) / (in.radii * in.radii));

    let sun_dir = normalize(globals.sun_direction);
    // Wrapped diffuse rather than clamped: a canopy is a volume of scattering
    // leaves, and a hard terminator on a smooth ellipsoid reads as a plastic
    // ball. Cloud shadow and the shadow map are deliberately absent - this is
    // the placement view, and §7's leaves carry the real lighting.
    let n_dot_l = dot(n, sun_dir) * 0.5 + 0.5;
    let lit = globals.sun_color * n_dot_l + globals.ambient_color * 0.6;
    let albedo = vec3<f32>(0.20, 0.34, 0.15);

    let clip = globals.view_proj * vec4<f32>(world_pos, 1.0);

    var out: FragmentOutput;
    out.color = vec4<f32>(albedo * lit, 1.0);
    // 0.5 is the vegetation class, which the outline pass ignores outright -
    // correct for a smooth curved surface, and the same reason wood needed its
    // own class rather than terrain's.
    out.normal = vec4<f32>(n, 0.5);
    out.depth = clip.z / clip.w;
    return out;
}