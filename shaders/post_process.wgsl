struct PostProcessUniforms {
    warm_tint: vec3<f32>,
    warm_tint_strength: f32,
    desaturation: f32,
    vignette_strength: f32,
    exposure: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> pp: PostProcessUniforms;

@group(0) @binding(1)
var scene_texture: texture_2d<f32>;

@group(0) @binding(2)
var scene_sampler: sampler;

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

    // Simple Reinhard tone mapping
    color = color / (color + vec3<f32>(1.0));

    return vec4<f32>(color, 1.0);
}