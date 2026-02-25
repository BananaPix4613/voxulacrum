@group(0) @binding(0)
var scene_tex: texture_2d<f32>;

@group(0) @binding(1)
var bilinear_sampler: sampler;

struct UpscaleUniforms {
    subpixel_offset: vec2<f32>,
    render_resolution: vec2<f32>,
    window_resolution: vec2<f32>,
    tex_resolution: vec2<f32>,
};

@group(0) @binding(2)
var<uniform> params: UpscaleUniforms;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // T3ssel8r-style pixel-art upscale.
    //
    // Maps window pixels to content texels via UV-snapping with a smoothstep
    // transition at texel boundaries.  At integer pixel_scale the smoothstep
    // evaluations land exactly at 0, producing identical results to nearest-
    // neighbour (perfectly sharp).  At non-integer ratios the smooth blending
    // eliminates the "swimming" artifact where pixel patterns shift each frame.

    // Sub-pixel correction: convert window-pixel offset back to content texels
    // and shift the sampling position to restore smooth apparent motion between
    // texel-snapped frames.  The 1-texel border around the render target
    // provides the extra coverage needed for shifts up to ±0.5 texels.
    let texel_shift = params.subpixel_offset * params.render_resolution / params.window_resolution;

    // Continuous content-texel coordinate (with sub-pixel scroll applied).
    let texel_coord = in.uv * params.render_resolution + texel_shift;

    // Box size in texels — how many texels one screen pixel covers.
    // For a fullscreen blit this equals render_resolution / window_resolution
    // (≈ 1/pixel_scale).  Clamped to [ε, 1] per T3ssel8r's method.
    let box_size = clamp(fwidth(texel_coord), vec2(1e-5), vec2(1.0));

    // Shift by half the box so the transition straddles the texel boundary.
    let tx = texel_coord - 0.5 * box_size;

    // Smooth transition: 0 inside a texel, ramps to 1 at the boundary.
    let tx_offset = smoothstep(vec2(1.0) - box_size, vec2(1.0), fract(tx));

    // Snapped texel coordinate — lands on texel centers when sharp,
    // interpolates between neighbours at boundaries.
    let snapped = floor(tx) + 0.5 + tx_offset;

    // Clamp to content range widened by ±0.5 texels to allow sub-pixel shifts
    // to reach into the 1-texel border (which contains valid rendered content).
    let clamped = clamp(snapped, vec2(0.0), params.render_resolution);

    // Convert to texture UV: +1.0 skips the 1-texel border.
    let sample_uv = (clamped + vec2(1.0)) / params.tex_resolution;

    return textureSample(scene_tex, bilinear_sampler, sample_uv);
}
