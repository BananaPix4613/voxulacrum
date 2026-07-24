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

/// Reflection targets render at 1/REFLECTION_DOWNSCALE of scene resolution -
/// pixelated reflections read fine, and the planar reflection pass re-renders the
/// scene, so keeping it small keeps that cost down.
pub const REFLECTION_DOWNSCALE: u32 = 1;

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
    pub scene_copy: wgpu::Texture,
    pub scene_copy_view: wgpu::TextureView,
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
    #[allow(dead_code)]
    pub reflection_color: wgpu::Texture,
    pub reflection_color_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub reflection_normal: wgpu::Texture,
    pub reflection_normal_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub reflection_depth: wgpu::Texture,
    pub reflection_depth_view: wgpu::TextureView,
    pub reflection_depth_sample_view: wgpu::TextureView,
    #[allow(dead_code)]
    pub reflection_w: u32,
    #[allow(dead_code)]
    pub reflection_h: u32,
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

        let scene = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lowres_scene"),
            size: wgpu::Extent3d { width: tex_width, height: tex_height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ctx.surface_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let scene_view = scene.create_view(&wgpu::TextureViewDescriptor::default());

        // Water samples last frame's opaque scene from here (SCENE is copied into it
        // before the water pass so refraction/SSR can read what's behind the water).
        let scene_copy = Self::create_color_texture(
            &ctx.device, tex_width, tex_height, ctx.surface_format, "lowres_scene_copy",
        );
        let scene_copy_view = scene_copy.create_view(&wgpu::TextureViewDescriptor::default());

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

        // Reflection targets (half-res). Same formats as the main pass so Step 2 can
        // reuse the existing scene pipelines; color is sampled by the water shader,
        // depth is sampled for the per-fragment above-water clip.
        let refl_w = (tex_width / REFLECTION_DOWNSCALE).max(1);
        let refl_h = (tex_height / REFLECTION_DOWNSCALE).max(1);

        let reflection_color =
            Self::create_color_texture(&ctx.device, refl_w, refl_h, ctx.surface_format, "reflection_color");
        let reflection_color_view = reflection_color.create_view(&wgpu::TextureViewDescriptor::default());

        let reflection_normal =
            Self::create_color_texture(&ctx.device, refl_w, refl_h, NORMAL_FORMAT, "reflection_normal");
        let reflection_normal_view = reflection_normal.create_view(&wgpu::TextureViewDescriptor::default());

        let reflection_depth = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection_depth"),
            size: wgpu::Extent3d { width: refl_w, height: refl_h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let reflection_depth_view = reflection_depth.create_view(&wgpu::TextureViewDescriptor::default());
        let reflection_depth_sample_view = reflection_depth.create_view(&wgpu::TextureViewDescriptor {
            label: Some("reflection_depth_sample"),
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });

        Self {
            scene, scene_view,
            scene_copy, scene_copy_view,
            processed, processed_view,
            depth, depth_view, depth_sample_view,
            normal, normal_view,
            reflection_color, reflection_color_view,
            reflection_normal, reflection_normal_view,
            reflection_depth, reflection_depth_view, reflection_depth_sample_view,
            reflection_w: refl_w,
            reflection_h: refl_h,
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
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST,
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