struct ShadowUniforms {
    light_space_matrix: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> shadow: ShadowUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) ao: f32,
    @location(4) material_id: u32,
};

@vertex
fn vs_main(in: VertexInput) -> @builtin(position) vec4<f32> {
    return shadow.light_space_matrix * vec4<f32>(in.position, 1.0);
}