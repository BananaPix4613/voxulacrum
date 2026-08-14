// Wood impostors as shadow casters.
//
// The same analytic solve as `capsule.wgsl`, against the light instead of the
// camera. Directional light means an orthographic shadow projection, so the ray
// direction is again a uniform and the quad is again a billboard - just facing
// the light. Depth-only: the shadow map wants occluder depth and nothing else.

struct ShadowUniforms {
    light_space_matrix: mat4x4<f32>,
    clip_min: vec3<f32>,
    clip_enabled: u32,
    clip_max: vec3<f32>,
    _pad: f32,
    light_dir: vec3<f32>,
    _pad_l0: f32,
    light_right: vec3<f32>,
    _pad_l1: f32,
    light_up: vec3<f32>,
    _pad_l2: f32,
};

@group(0) @binding(0) var<uniform> shadow: ShadowUniforms;

struct InstanceInput {
    @location(0) a: vec3<f32>,
    @location(1) ra: f32,
    @location(2) b: vec3<f32>,
    @location(3) rb: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ray_origin: vec3<f32>,
    @location(1) cap_a: vec3<f32>,
    @location(2) cap_b: vec3<f32>,
    @location(3) radii: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: InstanceInput) -> VertexOutput {
    var out: VertexOutput;
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[vi];

    let mid = (inst.a + inst.b) * 0.5;
    let maxr = max(inst.ra, inst.rb);
    let ea = inst.a - mid;
    let hu = abs(dot(ea, shadow.light_right)) + maxr;
    let hv = abs(dot(ea, shadow.light_up)) + maxr;

    // Past the whole capsule, not just its radius - see `capsule.wgsl`. The
    // shadow map would otherwise carry the same holes as the visible pass.
    let plane = mid - shadow.light_dir * (abs(dot(ea, shadow.light_dir)) + maxr + 0.01);
    let world_pos = plane + shadow.light_right * (c.x * hu) + shadow.light_up * (c.y * hv);

    out.clip_position = shadow.light_space_matrix * vec4<f32>(world_pos, 1.0);
    out.ray_origin = world_pos;
    out.cap_a = inst.a;
    out.cap_b = inst.b;
    out.radii = vec2<f32>(inst.ra, inst.rb);
    return out;
}

/// Identical to `capsule.wgsl`'s solve. Duplicated because these shaders have
/// no include mechanism - the same reason the visibility-mask march exists four
/// times. If one is changed the other must be.
fn intersect_wood(ro: vec3<f32>, rd: vec3<f32>, pa: vec3<f32>, pb: vec3<f32>, ra: f32, rb: f32) -> vec4<f32> {
    let miss = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    let ba = pb - pa;
    let oa = ro - pa;
    let m0 = dot(ba, ba);

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

    if abs(k2) <= 1e-8 {
        return miss;
    }
    let h = k1 * k1 - k0 * k2;
    if h < 0.0 {
        return miss;
    }
    let sq = sqrt(h);
    let t = select((-k1 + sq) / k2, (-k1 - sq) / k2, k2 > 0.0);
    let y = m1 - ra * rr + t * m2;
    if y <= 0.0 || y >= d2 {
        return miss;
    }
    return vec4<f32>(t, normalize(d2 * (oa + t * rd) - ba * y));
}

struct FragmentOutput {
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    let hit = intersect_wood(in.ray_origin, shadow.light_dir, in.cap_a, in.cap_b, in.radii.x, in.radii.y);
    if hit.x < 0.0 {
        discard;
    }
    let world_pos = in.ray_origin + shadow.light_dir * hit.x;

    if shadow.clip_enabled == 1u {
        if world_pos.x > shadow.clip_max.x || world_pos.y > shadow.clip_max.y || world_pos.z > shadow.clip_max.z
        || world_pos.x < shadow.clip_min.x || world_pos.y < shadow.clip_min.y || world_pos.z < shadow.clip_min.z {
            discard;
        }
    }

    let clip = shadow.light_space_matrix * vec4<f32>(world_pos, 1.0);
    var out: FragmentOutput;
    out.depth = clip.z / clip.w;
    return out;
}