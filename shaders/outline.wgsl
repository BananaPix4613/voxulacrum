struct OutlineUniforms {
    texel_size: vec2<f32>,
    depth_threshold: f32,
    depth_strength: f32,
    normal_threshold: f32,
    normal_strength: f32,
    darken_strength: f32,
    brighten_strength: f32,
    enabled: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
};

@group(0) @binding(0)
var<uniform> params: OutlineUniforms;

@group(0) @binding(1)
var scene_texture: texture_2d<f32>;

@group(0) @binding(2)
var point_sampler: sampler;

@group(0) @binding(3)
var depth_texture: texture_depth_2d;

@group(0) @binding(4)
var normal_texture: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

fn clamp_coord(c: vec2<i32>, tex_size: vec2<i32>) -> vec2<i32> {
    return clamp(c, vec2<i32>(0), tex_size - vec2<i32>(1));
}

// Determines which of the two pixels sharing a normal-edge "owns" it and draws it.
//
// Primary criterion: the face whose world-space normal has a larger Y-component
// (i.e. faces more upward / is more top-facing) owns the edge.  In isometric view
// this unambiguously assigns ownership between a top face (ny ≈ 1) and any side
// face (ny ≈ 0) without depending on the depth difference, which can be smaller
// than float precision for nearly-coplanar adjacent geometry.
//
// Tiebreaker: when both faces have the same Y-component (two vertical side faces),
// the closer pixel (smaller depth) owns the edge.
fn owns_normal_edge(ny_c: f32, ny_n: f32, d_c: f32, d_n: f32) -> bool {
    let diff = ny_c - ny_n;
    return diff > 0.01 || (diff > -0.01 && d_c < d_n);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(scene_texture, point_sampler, in.uv);

    if params.enabled == 0u {
        return color;
    }

    let tex_size = vec2<i32>(textureDimensions(depth_texture));
    let coord = vec2<i32>(in.uv * vec2<f32>(tex_size));

    // 4-tap cross kernel: exactly 1 texel in each cardinal direction
    let coord_u = clamp_coord(coord + vec2<i32>(0, -1), tex_size);
    let coord_d = clamp_coord(coord + vec2<i32>(0,  1), tex_size);
    let coord_l = clamp_coord(coord + vec2<i32>(-1, 0), tex_size);
    let coord_r = clamp_coord(coord + vec2<i32>( 1, 0), tex_size);

    // Load center normal; skip sky (a = 0) and vegetation (a = 0.5) pixels
    let n_c_raw = textureLoad(normal_texture, coord, 0);
    if n_c_raw.a < 0.75 {
        return color;
    }

    // Load depth
    let d_c = textureLoad(depth_texture, coord,   0);
    let d_u = textureLoad(depth_texture, coord_u, 0);
    let d_d = textureLoad(depth_texture, coord_d, 0);
    let d_l = textureLoad(depth_texture, coord_l, 0);
    let d_r = textureLoad(depth_texture, coord_r, 0);

    // Load neighbor normals
    let n_u_raw = textureLoad(normal_texture, coord_u, 0);
    let n_d_raw = textureLoad(normal_texture, coord_d, 0);
    let n_l_raw = textureLoad(normal_texture, coord_l, 0);
    let n_r_raw = textureLoad(normal_texture, coord_r, 0);

    let n_c = n_c_raw.xyz;
    let n_u = n_u_raw.xyz;
    let n_d = n_d_raw.xyz;
    let n_l = n_l_raw.xyz;
    let n_r = n_r_raw.xyz;

    // ================================================================
    // DEPTH EDGE — center must be strictly in front of neighbor (d_c < d_n).
    // Sky is cleared to 1.0 so surface pixels always own sky-adjacent edges.
    // At cliff steps (same normals, different depths) only the shallower pixel
    // fires, giving a single-pixel dark silhouette.
    // ================================================================
    let dd_u = select(0.0, abs(d_c - d_u), d_c < d_u);
    let dd_d = select(0.0, abs(d_c - d_d), d_c < d_d);
    let dd_l = select(0.0, abs(d_c - d_l), d_c < d_l);
    let dd_r = select(0.0, abs(d_c - d_r), d_c < d_r);
    let max_depth_diff = max(max(dd_u, dd_d), max(dd_l, dd_r));
    // Only a top-facing surface owns the dark depth silhouette, so the outline lands on
    // a top face's far edge where it drops off and never darkens vertical side faces.
    let top_c = step(0.5, n_c.y);
    let depth_edge = step(params.depth_threshold, max_depth_diff) * top_c;

    // ================================================================
    // NORMAL EDGE (convex ridges) — ownership via world-up Y-component.
    // A face whose normal has a larger Y (more top-facing) owns the ridge
    // with its neighbor.  This is depth-independent, so it works even when
    // adjacent face pixels have nearly-identical depth values (common in
    // orthographic isometric projection).  For two vertical faces with equal
    // Y-component, depth is used as a tiebreaker (closer pixel owns).
    // Only terrain-to-terrain edges count (a >= 0.75 on both sides).
    // ================================================================
    let ny_c = n_c.y;
    let nc_u = select(0.0, length(cross(n_c, n_u)),
        owns_normal_edge(ny_c, n_u.y, d_c, d_u) && n_u_raw.a >= 0.75);
    let nc_d = select(0.0, length(cross(n_c, n_d)),
        owns_normal_edge(ny_c, n_d.y, d_c, d_d) && n_d_raw.a >= 0.75);
    let nc_l = select(0.0, length(cross(n_c, n_l)),
        owns_normal_edge(ny_c, n_l.y, d_c, d_l) && n_l_raw.a >= 0.75);
    let nc_r = select(0.0, length(cross(n_c, n_r)),
        owns_normal_edge(ny_c, n_r.y, d_c, d_r) && n_r_raw.a >= 0.75);
    let max_normal_diff = max(max(nc_u, nc_d), max(nc_l, nc_r));
    let normal_edge = step(params.normal_threshold, max_normal_diff);

    // Early out if no edge at all
    if max(depth_edge, normal_edge) < 0.001 {
        return color;
    }

    // ================================================================
    // COLOUR APPLICATION
    //   Normal edge (convex ridge) → brighten  — takes priority
    //   Depth edge  (silhouette)   → darken    — suppressed when normal edge fires
    //
    // Priority explanation: at the ridge between a top face and a side face,
    // both a depth edge and a normal edge may fire on the top-face pixel.
    // We want the ridge to be bright (convex highlight), not dark, so the
    // normal edge wins.  Pure depth edges (cliff steps, sky silhouettes)
    // where no normal edge fires remain dark.
    // ================================================================
    let darkening   = depth_edge * (1.0 - normal_edge) * params.darken_strength;
    let brightening = normal_edge * params.brighten_strength;

    let final_color = color.rgb * (1.0 - darkening + brightening);
    return vec4<f32>(final_color, color.a);
}
