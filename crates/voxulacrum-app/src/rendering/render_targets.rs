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

/// Scene depth format. Carries a stencil aspect for the player-silhouette mask
/// (marked before foliage, composited after). Every pipeline drawing into the main
/// scene pass must declare this. The shadow pass has its own Depth32Float target.
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

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
    pub depth_sample_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub normal: wgpu::Texture,
    pub normal_view: wgpu::TextureView,
    pub alloc_w: u32,
    pub alloc_h: u32,
    pub view_w: f32,
    pub view_h: f32,
    pub k: u32,
    pub s: f32,
}

impl RenderTargets {
    pub fn new(ctx: &RenderContext, dims: &crate::RenderDims) -> Self {
        let tex_width = dims.alloc_w.max(1);
        let tex_height = dims.alloc_h.max(1);

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
            format: SCENE_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        // Sampling a combined depth-stencil texture requires a depth-only view; the
        // attachment view above keeps both aspects so stencil ops work.
        let depth_sample_view = depth.create_view(&wgpu::TextureViewDescriptor {
            label: Some("lowres_depth_sample"),
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        let normal = Self::create_color_texture(&ctx.device, tex_width, tex_height, NORMAL_FORMAT, "lowres_normal");
        let normal_view = normal.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            scene, scene_view,
            processed, processed_view,
            depth, depth_view, depth_sample_view,
            normal, normal_view,
            alloc_w: tex_width,
            alloc_h: tex_height,
            view_w: dims.view_w,
            view_h: dims.view_h,
            k: dims.k,
            s: dims.s,
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

    pub fn needs_recreate(&self, dims: &crate::RenderDims) -> bool {
        self.alloc_w != dims.alloc_w || self.alloc_h != dims.alloc_h
    }

    /// Zoom changes `view`/`k`/`s` continuously without reallocating; keep the
    /// fields current every frame so uniforms and the camera snap read them.
    pub fn update_view(&mut self, dims: &crate::RenderDims) {
        self.view_w = dims.view_w;
        self.view_h = dims.view_h;
        self.k = dims.k;
        self.s = dims.s;
    }
}