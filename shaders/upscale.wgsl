@group(0) @binding(0)
var scene_tex: texture_2d<f32>;

@group(0) @binding(1)
var point_sampler: sampler;

struct UpscaleUniforms {
    subpixel_offset: vec2<f32>,
    render_resolution: vec2<f32>,
    window_resolution: vec2<f32>,
    _pad: vec2<f32>,
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
    // Shift UV by sub-pixel offset to restore smooth camera motion.
    // The offset is in native-resolution pixels; convert to UV space.
    let offset_uv = params.subpixel_offset / params.window_resolution;
    let sample_uv = in.uv - offset_uv;

    return textureSample(scene_tex, point_sampler, sample_uv);
}