//! The two cloud textures the global bind group samples
//! (`docs/cloud-shadow-architecture.md`): Layer B, the baked detail tiles, and
//! Layer A, the sim-driven envelope. `CloudShadowState` builds both sets of
//! texels on the main side; this holds the GPU copies and uploads them when
//! they move.

use bevy_ecs::prelude::Resource;

use crate::versioned::{Uploaded, Versioned};

/// Bytes per texel: `Rgba8Unorm`, one channel per cloud layer.
const BYTES_PER_TEXEL: u32 = 4;

#[derive(Resource)]
pub struct CloudTextures {
    detail_texture: wgpu::Texture,
    pub detail_view: wgpu::TextureView,
    pub detail_sampler: wgpu::Sampler,
    detail_size: u32,
    detail_uploaded: Uploaded,
    envelope_texture: wgpu::Texture,
    pub envelope_view: wgpu::TextureView,
    pub envelope_sampler: wgpu::Sampler,
    envelope_size: u32,
    envelope_uploaded: Uploaded,
}

impl CloudTextures {
    /// Square textures of `detail_size` and `envelope_size` texels a side. Both
    /// start empty; `upload` fills them.
    pub fn new(device: &wgpu::Device, detail_size: u32, envelope_size: u32) -> Self {
        let detail_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud_shadow_texture"),
            size: wgpu::Extent3d { width: detail_size, height: detail_size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let detail_view = detail_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Repeat: this is a genuinely static, infinite-by-construction tiling
        // texture (baked once, seamlessly, by `CloudShadowState::new`) - not a
        // camera-centered window, so there's no edge to clamp.
        let detail_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Layer A - the envelope: shared Rgba8Unorm, one texel per macro
        // cell, rebuilt unconditionally every climate tick from the sim's
        // `ClimateGrid`s (architecture doc §2). Unlike Layer B this is
        // genuinely dynamic - ClampToEdge is fine because the shader
        // explicitly substitutes `cloud_default_N` outside the window rather
        // than relying on texture clamping (§3).
        let envelope_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud_envelope_texture"),
            size: wgpu::Extent3d { width: envelope_size, height: envelope_size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let envelope_view = envelope_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let envelope_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud_envelope_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            detail_texture,
            detail_view,
            detail_sampler,
            detail_size,
            detail_uploaded: Uploaded::default(),
            envelope_texture,
            envelope_view,
            envelope_sampler,
            envelope_size,
            envelope_uploaded: Uploaded::default(),
        }
    }

    /// Upload whichever set of texels moved since the last call.
    pub fn upload(
        &mut self,
        queue: &wgpu::Queue,
        detail: &Versioned<Vec<u8>>,
        envelope: &Versioned<Vec<u8>>,
    ) {
        if let Some(texels) = self.detail_uploaded.changed(detail) {
            write_square(queue, &self.detail_texture, self.detail_size, texels);
        }
        if let Some(texels) = self.envelope_uploaded.changed(envelope) {
            write_square(queue, &self.envelope_texture, self.envelope_size, texels);
        }
    }
}

/// Write the whole of a square `Rgba8Unorm` texture `size` texels a side.
fn write_square(queue: &wgpu::Queue, texture: &wgpu::Texture, size: u32, texels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * BYTES_PER_TEXEL),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
    );
}
