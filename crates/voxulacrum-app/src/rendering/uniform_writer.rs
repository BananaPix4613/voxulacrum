use crate::cloud_shadow::CloudShadowState;
use crate::palette::{self, Palette};
use crate::params::EngineParams;
use crate::rendering::outline_pass::OutlinePass;
use crate::rendering::palette_pass::PalettePass;
use crate::rendering::post_process::PostProcessPass;
use crate::rendering::render_context::RenderContext;
use crate::rendering::render_targets::RenderTargets;
use crate::rendering::uniforms::{
    GlobalUniforms, OutlineUniforms, PostProcessUniforms, ShadowUniforms, UpscaleUniforms,
};
use crate::rendering::upscale_pass::UpscalePass;
use crate::simulation::manager::FrameState;

pub fn write_all_uniforms(
    ctx: &RenderContext,
    frame: &FrameState,
    params: &EngineParams,
    cloud_shadow: &CloudShadowState,
    global_buf: &wgpu::Buffer,
    shadow_buf: &wgpu::Buffer,
    post_process: &PostProcessPass,
    outline: &OutlinePass,
    palette_pass: &PalettePass,
    upscale: &UpscalePass,
    render_targets: &RenderTargets,
    loaded_palette: &Option<Palette>,
    surface_width: u32,
    surface_height: u32,
) {
    // Shadow uniforms
    let shadow_uniforms = ShadowUniforms {
        light_space_matrix: frame.light_space.to_cols_array_2d(),
        clip_min: frame.clip_min,
        clip_enabled: frame.clip_enabled,
        clip_max: frame.clip_max,
        _pad: 0.0,
    };
    ctx.queue.write_buffer(shadow_buf, 0, bytemuck::cast_slice(&[shadow_uniforms]));

    // Global uniforms
    let uniforms = GlobalUniforms {
        view_proj: frame.view_proj,
        light_space_matrix: frame.light_space.to_cols_array_2d(),
        sun_direction: frame.sun_direction,
        _pad0: 0.0,
        sun_color: frame.sun_color,
        _pad1: 0.0,
        ambient_color: frame.ambient_color,
        _pad2: 0.0,
        wind_vector: frame.wind_vector,
        time: frame.elapsed,
        debug_mode: frame.debug_mode,
        cloud_offset_0: cloud_shadow.offsets[0].into(),
        cloud_coverage_0: cloud_shadow.coverages[0],
        cloud_uv_scale_0: params.cloud.layers[0].uv_scale,
        cloud_offset_1: cloud_shadow.offsets[1].into(),
        cloud_coverage_1: cloud_shadow.coverages[1],
        cloud_uv_scale_1: params.cloud.layers[1].uv_scale,
        cloud_offset_2: cloud_shadow.offsets[2].into(),
        cloud_coverage_2: cloud_shadow.coverages[2],
        cloud_uv_scale_2: params.cloud.layers[2].uv_scale,
        cloud_offset_3: cloud_shadow.offsets[3].into(),
        cloud_coverage_3: cloud_shadow.coverages[3],
        cloud_uv_scale_3: params.cloud.layers[3].uv_scale,
        env_origin: cloud_shadow.env_origin_world.into(),
        env_extent: cloud_shadow.env_extent_world,
        _pad_env: 0.0,
        cloud_defaults: cloud_shadow.cloud_defaults,
        env_typical: cloud_shadow.env_typical,
        edge_strength: params.meshing.edge_strength,
        ortho_ao_strength: if params.meshing.ortho_ao_enabled {
            params.meshing.ortho_ao_strength
        } else {
            0.0
        },
        clip_enabled: frame.clip_enabled,
        _pad_a: 0.0,
        clip_min: frame.clip_min,
        _pad3: 0.0,
        clip_max: frame.clip_max,
        _pad4: 0.0,
        mask_origin: frame.mask_origin,
        mask_enabled: frame.mask_enabled,
        view_dir: frame.view_dir,
        _pad5: 0.0,
        render_size: [render_targets.view_w, render_targets.view_h],
        volume_radius: frame.volume_radius,
        _pad6: 0.0,
    };
    ctx.queue.write_buffer(global_buf, 0, bytemuck::cast_slice(&[uniforms]));

    // Post-process uniforms
    let pp = &params.post_process;
    let cs = &params.cross_section;

    let vp_mat = glam::Mat4::from_cols_array_2d(&frame.view_proj);
    let inv_vp = vp_mat.inverse().to_cols_array_2d();

    let clip_fog_enabled: u32 = if cs.enabled {
        if cs.show_edges { 2 } else { 1 }
    } else {
        0
    };

    let pp_uniforms = PostProcessUniforms {
        warm_tint: frame.warm_tint_color,
        warm_tint_strength: frame.warm_tint_strength,
        desaturation: (cloud_shadow.coverages[0]
            + cloud_shadow.coverages[1]
            + cloud_shadow.coverages[2]
            + cloud_shadow.coverages[3])
            * 0.25
            * pp.overcast_desaturation_factor,
        vignette_strength: pp.vignette_strength,
        exposure: pp.exposure,
        clip_fog_enabled,
        fog_color: cs.fog_color,
        fog_density: cs.fog_density,
        clip_max: frame.clip_max,
        _pad0: 0.0,
        clip_min: frame.clip_min,
        _pad1: 0.0,
        inv_view_proj: inv_vp,
        render_resolution: [render_targets.view_w, render_targets.view_h],
        _pad2: [0.0; 2],
    };
    post_process.update_uniforms(&ctx.queue, pp_uniforms);

    // Outline uniforms
    let op = &params.outline;
    outline.update_uniforms(
        &ctx.queue,
        OutlineUniforms {
            texel_size: [
                1.0 / render_targets.alloc_w as f32,
                1.0 / render_targets.alloc_h as f32,
            ],
            depth_threshold: op.depth_threshold,
            depth_strength: op.depth_strength,
            normal_threshold: op.normal_threshold,
            normal_strength: op.normal_strength,
            darken_strength: op.darken_strength,
            brighten_strength: op.brighten_strength,
            enabled: if op.enabled { 1 } else { 0 },
            _pad: [0; 3],
        },
    );

    // Palette uniforms
    let palette_uniforms = if params.palette.mode == 0 {
        if let Some(ref pal) = loaded_palette {
            palette::palette_to_uniforms(pal, &params.palette)
        } else {
            palette::stepping_uniforms(&params.palette)
        }
    } else {
        palette::stepping_uniforms(&params.palette)
    };
    palette_pass.update_uniforms(&ctx.queue, palette_uniforms);

    // Upscale uniforms
    let upscale_uniforms = UpscaleUniforms {
        subpixel_offset: frame.subpixel_offset,
        render_resolution: [render_targets.view_w, render_targets.view_h],
        window_resolution: [surface_width as f32, surface_height as f32],
        tex_resolution: [render_targets.alloc_w as f32, render_targets.alloc_h as f32],
    };
    upscale.update_uniforms(&ctx.queue, upscale_uniforms);
}