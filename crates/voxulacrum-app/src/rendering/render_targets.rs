use bevy_ecs::prelude::Resource;

use crate::rendering::render_context::RenderContext;

/// Low-resolution render targets for the stylized pixel-art pipeline.
///
/// Owns four textures:
/// - `scene` / `scene_view`: Main pass writes color here.
/// - `normal` / `normal_view`: Main pass writes world-space normals here (MRT).
/// - `processed` / `processed_view`: Intermediate ping-pong buffer.
/// - `depth` / `depth_view`: Main pass depth buffer.
///
/// The pipeline chain is:
///   Main -> scene + depth + normal
///   Outline -> reads scene+depth+normal -> writes processed
///   PostProcess -> reads processed -> writes scene
///   Palette -> reads scene -> writes processed
///   Upscale -> reads processed -> writes swap chain

pub const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[derive(Resource)]
pub struct RenderTargets {
    // RAII: these textures own the storage the matching *_view handles borrow;
    // held for resize/recreate, not read directly.
    #[allow(dead_code)]
    pub scene: wgpu::Texture,
    pub scene_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub processed: wgpu::Texture,
    pub processed_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub depth: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub normal: wgpu::Texture,
    pub normal_view: wgpu::TextureView,
    pub render_width: u32,
    pub render_height: u32,
    pub tex_width: u32,
    pub tex_height: u32,
    pub effective_pixel_scale: f32,
}

impl RenderTargets {
    pub fn new(
        ctx: &RenderContext,
        render_width: u32,
        render_height: u32,
        effective_pixel_scale: f32,
    ) -> Self {
        let render_width = render_width.max(1);
        let render_height = render_height.max(1);

        let tex_width = render_width + 2;
        let tex_height = render_height + 2;

        let scene = Self::create_color_texture(&ctx.device, tex_width, tex_height, ctx.surface_format, "lowres_scene");
        let scene_view = scene.create_view(&wgpu::TextureViewDescriptor::default());

        let processed = Self::create_color_texture(&ctx.device, tex_width, tex_height, ctx.surface_format, "lowres_processed");
        let processed_view = processed.create_view(&wgpu::TextureViewDescriptor::default());

        let depth = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lowres_depth"),
            size: wgpu::Extent3d {
                width: tex_width,
                height: tex_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let normal = Self::create_color_texture(&ctx.device, tex_width, tex_height, NORMAL_FORMAT, "lowres_normal");
        let normal_view = normal.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            scene, scene_view,
            processed, processed_view,
            depth, depth_view,
            normal, normal_view,
            render_width, render_height,
            tex_width, tex_height,
            effective_pixel_scale,
        }
    }

    fn create_color_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    pub fn needs_recreate(&self, render_width: u32, render_height: u32) -> bool {
        self.render_width != render_width || self.render_height != render_height
    }
}