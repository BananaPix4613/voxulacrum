struct ShadowUniforms {
    light_space_matrix: mat4x4<f32>,
    clip_min: vec3<f32>,
    clip_enabled: u32,
    clip_max: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> shadow: ShadowUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = shadow.light_space_matrix * vec4<f32>(in.position, 1.0);
    out.world_position = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) {
    // Cross-section clipping for shadows
    if shadow.clip_enabled == 1u {
        let wp = in.world_position;
        if wp.x > shadow.clip_max.x || wp.y > shadow.clip_max.y || wp.z > shadow.clip_max.z
        || wp.x < shadow.clip_min.x || wp.y < shadow.clip_min.y || wp.z < shadow.clip_min.z {
            discard;
        }
    }
}
