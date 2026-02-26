struct PostProcessUniforms {
    warm_tint: vec3<f32>,
    warm_tint_strength: f32,
    desaturation: f32,
    vignette_strength: f32,
    exposure: f32,
    clip_fog_enabled: u32,
    fog_color: vec3<f32>,
    fog_density: f32,
    clip_max: vec3<f32>,
    _pad0: f32,
    clip_min: vec3<f32>,
    _pad1: f32,
    inv_view_proj: mat4x4<f32>,
    render_resolution: vec2<f32>,
    _pad2: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> pp: PostProcessUniforms;

@group(0) @binding(1)
var scene_texture: texture_2d<f32>;

@group(0) @binding(2)
var scene_sampler: sampler;

@group(0) @binding(3)
var depth_texture: texture_depth_2d;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Fullscreen triangle: 3 vertices cover the entire screen
    var out: VertexOutput;
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(scene_texture, scene_sampler, in.uv).rgb;

    // Exposure adjustment
    color = color * pp.exposure;

    // Time-of-day warm/cool tint
    color = mix(color, color * pp.warm_tint, pp.warm_tint_strength);

    // Overcast desaturation
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(color, vec3<f32>(luminance), pp.desaturation);

    // Vignette
    let dist = distance(in.uv, vec2<f32>(0.5));
    let vig = smoothstep(0.5, 0.9, dist) * pp.vignette_strength;
    color = color * (1.0 - vig);

    // Cross-section fog: blend fog color near clip plane boundaries
    if pp.clip_fog_enabled != 0u {
        let tex_size = vec2<i32>(textureDimensions(depth_texture));
        let coord = vec2<i32>(in.uv * vec2<f32>(tex_size));
        let depth = textureLoad(depth_texture, coord, 0);

        // Skip sky pixels (depth at far plane)
        if depth < 1.0 {
            // Reconstruct world position from depth via inverse view-projection
            let ndc = vec4<f32>(
                in.uv.x * 2.0 - 1.0,
                (1.0 - in.uv.y) * 2.0 - 1.0,
                depth,
                1.0
            );
            let world_h = pp.inv_view_proj * ndc;
            let world_pos = world_h.xyz / world_h.w;

            // Distance from world position to nearest active clip plane.
            // Smaller distance = closer to the cut surface = more fog.
            var min_dist = 1000.0;
            if pp.clip_max.x < 9000.0 {
                min_dist = min(min_dist, pp.clip_max.x - world_pos.x);
            }
            if pp.clip_max.y < 9000.0 {
                min_dist = min(min_dist, pp.clip_max.y - world_pos.y);
            }
            if pp.clip_max.z < 9000.0 {
                min_dist = min(min_dist, pp.clip_max.z - world_pos.z);
            }
            if pp.clip_min.x > -9000.0 {
                min_dist = min(min_dist, world_pos.x - pp.clip_min.x);
            }
            if pp.clip_min.y > -9000.0 {
                min_dist = min(min_dist, world_pos.y - pp.clip_min.y);
            }
            if pp.clip_min.z > -9000.0 {
                min_dist = min(min_dist, world_pos.z - pp.clip_min.z);
            }

            // Fog ramps from full at the clip plane to zero further inside
            let fog_amount = 1.0 - exp(-pp.fog_density * max(0.0, 1.0 - min_dist));
            color = mix(color, pp.fog_color, clamp(fog_amount, 0.0, 0.85));

            // Debug edge highlight: orange line at clip boundary
            if pp.clip_fog_enabled >= 2u {
                if min_dist < 0.3 && min_dist > -0.1 {
                    color = mix(color, vec3<f32>(1.0, 0.4, 0.1), 0.7);
                }
            }
        }
    }

    // Simple Reinhard tone mapping
    color = color / (color + vec3<f32>(1.0));

    return vec4<f32>(color, 1.0);
}
