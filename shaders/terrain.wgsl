// Tri-tonal axis lighting (design §11): brightness is chosen by a face's cardinal
// axis, not smooth n·l. Top brightest; the two side axes distinct so perpendicular
// walls read apart; bottom darkest. Makes height steps and slabs legible in iso.
const AXIS_TOP: f32 = 1.0;
const AXIS_SIDE_X: f32 = 0.75;
const AXIS_SIDE_Z: f32 = 0.60;
const AXIS_BOTTOM: f32 = 0.40;

// Flood cost at which the player's local space ends. Air past this is "fringe"
// (dimmed + outlined in a later step); the BUDGET-22 flood behind it is what the
// player *knows*, not what he *sees*. Raise to see more, lower to tighten.
const CLARITY_RADIUS: u32 = 24u;
const CIRCLE2_SCALE: f32 = 1.1; // circle 2 radius relative to the volume circle
const CIRCLE2_DROP: f32 = 0.2;  // circle 2 center offset below the volume (× r1)
const VOID_MATERIAL: vec3<f32> = vec3<f32>(0.032, 0.032, 0.042); // near-black; blends with the void, below cave stone
const VOLUME_CIRCLE_PAD: f32 = 1.2; // slack so circle 1 comfortably bounds the pocket

// Debug mode constants
const DEBUG_NONE: u32 = 0u;
const DEBUG_MATERIAL_ID: u32 = 1u;
const DEBUG_AO_ONLY: u32 = 2u;
const DEBUG_NORMALS: u32 = 3u;

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
    mask_origin: vec3<f32>,
    mask_enabled: u32,
    view_dir: vec3<f32>,
    _pad5: f32,
    render_size: vec2<f32>,
    volume_radius: f32,
    _pad6: f32,
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

@group(0) @binding(5)
var<storage, read> material_colors: array<vec4<f32>>;

@group(0) @binding(6)
var visibility_mask: texture_3d<u32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) packed_normal: vec4<f32>,   // Snorm8x4: xyz cardinal normal
    @location(3) mat_pair: vec2<u32>,        // Uint16x2: x = material_id
    @location(4) axis_data: vec4<u32>,       // Uint8x4: x = face_axis (0..5)
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) material_id: u32,
    @location(3) @interpolate(flat) face_axis: u32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    out.normal = in.packed_normal.xyz;
    out.material_id = in.mat_pair.x;
    out.face_axis = in.axis_data.x;
    return out;
}

// World-space normal-offset bias (in voxel units). Pushing the shadow sample
// point off the surface along its normal before projecting into light space
// decouples the bias from face-to-light slope, which is what a uniform
// DepthBiasState can't do for axis-aligned voxel faces at grazing sun angles
// (slope-scale bias there is effectively unbounded, forcing either acne on
// near-perpendicular faces or peter-panning on grazing ones).
//
// The offset still needs to grow as the light grows grazing to the surface
// (error scales ~1/dot(n,l)): with the sun orbiting through a full day/night
// cycle, a face that's near-perpendicular to the sun at noon can be nearly
// edge-on to it at dawn/dusk, so a single fixed offset that looks right at
// one angle bands into acne at others. Scale by n·l, floored so grazing/back
// faces don't blow the offset up unboundedly, and clamped so it never grows
// large enough to look like peter-panning.
//
// NORMAL_OFFSET_BASE must clear the shadow map's world-space texel size even
// at the *best* angle (n·l = 1, offset at its smallest), or the sample can
// land back in the same texel that rasterized this fragment and self-compare
// against its own depth -> periodic texel-pitch banding on fully lit,
// unoccluded surfaces (worst right under a high sun, where n·l is largest and
// the offset would otherwise shrink to its minimum). The shadow frustum here
// covers a ~256-unit cube (see shadow_radius in simulation/manager.rs) over
// SHADOW_MAP_SIZE=4096 texels (main.rs); worst-case rotated bounding box is
// up to ~sqrt(2) larger, so texel size is ~0.07-0.1 world units. 0.1 keeps
// the floor above that with margin; re-tune if those constants change.
//
// NORMAL_OFFSET_MAX is kept well under half a voxel deliberately: at a
// concave/interior corner (e.g. a groove cut into the terrain), pushing a
// receiver's sample point too far along its normal can shove it past the
// corner into the volume actually occluded by the *adjacent* perpendicular
// face - the surface isn't self-shadowing anymore, it's being falsely
// shadowed by its neighbor, which shows up as unexpected darkening right at
// interior corners with a soft/blended edge (the offset varies with local
// n·l, so the false-shadow boundary isn't a hard cutoff). Capping the offset
// keeps grazing angles from reaching that regime; DepthBiasState's
// slope_scale (pipelines.rs create_shadow_pipeline) picks up the remaining
// grazing-angle residual instead, so no single mechanism has to cover the
// whole range on its own.
const NORMAL_OFFSET_BASE: f32 = 0.04;
const NORMAL_OFFSET_MAX: f32 = 0.1;
const NORMAL_OFFSET_MIN_NDOTL: f32 = 0.1;

fn compute_shadow(world_pos: vec3<f32>, normal: vec3<f32>) -> f32 {
    let n_dot_l = max(dot(normal, normalize(globals.sun_direction)), NORMAL_OFFSET_MIN_NDOTL);
    let offset = min(NORMAL_OFFSET_BASE / n_dot_l, NORMAL_OFFSET_MAX);
    let biased_pos = world_pos + normal * offset;
    let light_space_pos = globals.light_space_matrix * vec4<f32>(biased_pos, 1.0);
    let proj_coords = light_space_pos.xyz / light_space_pos.w;

    // Convert NDC (-1 to 1) to UV coordinates (0 to 1)
    let shadow_uv = vec2<f32>(
        proj_coords.x * 0.5 + 0.5,
        proj_coords.y * -0.5 + 0.5,
    );

    let current_depth = proj_coords.z;

    // Early exit if the fragment falls outside the shadow map bounds
    if shadow_uv.x < 0.0 || shadow_uv.x > 1.0 || shadow_uv.y < 0.0 || shadow_uv.y > 1.0 {
        return 1.0;
    }
    if current_depth > 1.0 || current_depth < 0.0 {
        return 1.0;
    }

    // Single hard sample, no PCF: at this render resolution a blurred
    // multi-tap kernel (previously 3x3, ~0.15-0.2 world units of softening)
    // reads as mismatched against the crisp/pixelated look of the rest of the
    // scene. A hard edge matches the voxel aesthetic; if any softening is
    // wanted later, prefer widening this deliberately over reintroducing PCF.
    return textureSampleCompare(
        shadow_map,
        shadow_sampler,
        shadow_uv,
        current_depth // Passing raw current_depth; hardware handles the bias
    );
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

// Per-axis brightness. face_axis: 0=+X, 1=-X, 2=+Y(top), 3=-y(bottom), 4=+Z, 5=-Z.
fn tri_tonal_factor(face_axis: u32) -> f32 {
    switch (face_axis) {
        case 2u: { return AXIS_TOP; }
        case 3u: { return AXIS_BOTTOM; }
        case 0u, 1u: { return AXIS_SIDE_X; }
        default: { return AXIS_SIDE_Z; }
    }
}

fn project_screen(w: vec3<f32>) -> vec2<f32> {
    let c = globals.view_proj * vec4<f32>(w, 1.0);
    return (c.xy / c.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5)) * globals.render_size;
}

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_main(in: VertexOutput) -> FragmentOutput {
    let clip_wp = in.world_position;
    if globals.clip_enabled == 1u {
        // Cross-section (dev tool): discard fragments OUTSIDE the box.
        if clip_wp.x > globals.clip_max.x || clip_wp.y > globals.clip_max.y || clip_wp.z > globals.clip_max.z
        || clip_wp.x < globals.clip_min.x || clip_wp.y < globals.clip_min.y || clip_wp.z < globals.clip_min.z {
            discard;
        }
    }

    if globals.mask_enabled == 1u {
        let nrm = normalize(in.normal);
        let air_w = floor(clip_wp + 0.25 * nrm);
        let ai = vec3<i32>(air_w) - vec3<i32>(globals.mask_origin);
        var s = 0u;
        if ai.x >= 0 && ai.x < 64 && ai.y >= 0 && ai.y < 64 && ai.z >= 0 && ai.z < 64 {
            s = textureLoad(visibility_mask, ai, 0).r & 0x7Fu;
        }
        let lit = s != 0u && s <= CLARITY_RADIUS + 1u;

        // Volume circle, sized from the pocket's world radius, re-projected each frame.
        let head_w = globals.mask_origin + vec3<f32>(32.0);
        let head_px = project_screen(head_w);
        var r1 = 0.0;
        r1 = max(r1, distance(project_screen(head_w + vec3<f32>(globals.volume_radius, 0.0, 0.0)), head_px));
        r1 = max(r1, distance(project_screen(head_w + vec3<f32>(0.0, globals.volume_radius, 0.0)), head_px));
        r1 = max(r1, distance(project_screen(head_w + vec3<f32>(0.0, 0.0, globals.volume_radius)), head_px));
        r1 = r1 * VOLUME_CIRCLE_PAD;
        let r2 = r1 * CIRCLE2_SCALE;
        let c2_center = head_px + vec2<f32>(0.0, r1 * CIRCLE2_DROP);

        // Per-voxel membership: test the owning voxel's projected center so a whole voxel
        // culls together — a blocky, voxel-aligned edge instead of a smooth pixel cut.
        let vox_px = project_screen(floor(clip_wp - 0.25 * nrm) + vec3<f32>(0.5));
        let in1 = distance(vox_px, head_px) < r1;
        let in2 = distance(vox_px, c2_center) < r2;

        if lit {
            let base = air_w + vec3<f32>(0.5, 0.5, 0.5);
            for (var i = 1; i <= 64; i = i + 1) {
                let p = base + globals.view_dir * (f32(i) * 0.5);
                let c = vec3<i32>(floor(p)) - vec3<i32>(globals.mask_origin);
                if c.x < 0 || c.x >= 64 || c.y < 0 || c.y >= 64 || c.z < 0 || c.z >= 64 { break; }
                let raw = textureLoad(visibility_mask, c, 0).r;
                if (raw & 0x80u) != 0u { continue; }
                let behind = raw & 0x7Fu;
                if behind != 0u && behind < s { discard; }
            }
            if in2 && !in1 { discard; }
        } else {
            if in2 || in1 { discard; }
            let shade = 0.45 + 0.55 * tri_tonal_factor(in.face_axis);
            return FragmentOutput(vec4<f32>(VOID_MATERIAL * shade, 1.0), vec4<f32>(nrm, 1.0));
        }
    }

    let n = normalize(in.normal);

    // --- Debug modes ---
    if globals.debug_mode == DEBUG_MATERIAL_ID {
        return FragmentOutput(vec4<f32>(debug_material_color(in.material_id), 1.0), vec4<f32>(n, 1.0));
    }
    if globals.debug_mode == DEBUG_AO_ONLY {
        // Per-vertex AO not yet meshed (ao_factor is constant); show flat white
        return FragmentOutput(vec4<f32>(1.0, 1.0, 1.0, 1.0), vec4<f32>(n, 1.0));
    }
    if globals.debug_mode == DEBUG_NORMALS {
        let normal_color = n * 0.5 + 0.5;
        return FragmentOutput(vec4<f32>(normal_color, 1.0), vec4<f32>(n, 1.0));
    }

    // --- Tri-tonal axis lighting (design §11) ---
    let axis_factor = tri_tonal_factor(in.face_axis);

    let cloud_uv = in.world_position.xz * 0.015 + globals.cloud_shadow_offset;
    let cloud_sample = textureSample(cloud_texture, cloud_sampler, cloud_uv).r;
    let cloud_threshold = smoothstep(globals.cloud_coverage - 0.15, globals.cloud_coverage + 0.15, cloud_sample);
    let cloud_factor = mix(0.25, 1.0, cloud_threshold);

    let sun_dir = normalize(globals.sun_direction);
    let n_dot_l = max(dot(n, sun_dir), 0.0);
    let diffuse = globals.sun_color * n_dot_l;
    let ambient = globals.ambient_color * 0.8;

    let shadow = compute_shadow(in.world_position, n);

    // Sun (dimmed by cast + cloud shadow) plus unshadowed ambient, tinted per axis.
    let light = diffuse * (shadow * cloud_factor) * 0.6 + ambient * 0.55;
    let base_color = material_colors[in.material_id].rgb;
    var final_color = base_color * axis_factor * light;

    return FragmentOutput(vec4<f32>(final_color, 1.0), vec4<f32>(n, 1.0));
}