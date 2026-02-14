use glam::Vec2;

use crate::params::CloudParams;

pub struct CloudShadowState {
    pub offset: Vec2,
    pub coverage: f32,
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl CloudShadowState {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, params: &CloudParams) -> Self {
        let size = 512u32;

        let mut noise1 = fastnoise_lite::FastNoiseLite::with_seed(42);
        noise1.set_noise_type(Some(fastnoise_lite::NoiseType::OpenSimplex2));
        noise1.set_frequency(Some(params.noise_freq_1));

        let mut noise2 = fastnoise_lite::FastNoiseLite::with_seed(137);
        noise2.set_noise_type(Some(fastnoise_lite::NoiseType::OpenSimplex2));
        noise2.set_frequency(Some(params.noise_freq_2));

        let mut noise3 = fastnoise_lite::FastNoiseLite::with_seed(256);
        noise3.set_noise_type(Some(fastnoise_lite::NoiseType::OpenSimplex2));
        noise3.set_frequency(Some(params.noise_freq_3));

        let mut pixels = vec![0u8; (size * size) as usize];
        let tau = std::f32::consts::TAU;
        let radius = 1.0;

        for y in 0..size {
            for x in 0..size {
                let u = x as f32 / size as f32;
                let v = y as f32 / size as f32;

                let nx = radius * (u * tau).cos();
                let ny = radius * (u * tau).sin();
                let nz = radius * (v * tau).cos();
                let nw = radius * (v * tau).sin();

                let o1 = noise1.get_noise_3d(nx, ny, nz + nw) * 0.5 + 0.5;
                let o2 = noise2.get_noise_3d(nx, ny, nz + nw) * 0.5 + 0.5;
                let o3 = noise3.get_noise_3d(nx, ny, nz + nw) * 0.5 + 0.5;

                let val = (o1 + o2 * 0.5 + o3 * 0.25) / 1.75;
                pixels[(y * size + x) as usize] = (val * 255.0).clamp(0.0, 255.0) as u8;
            }
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud_shadow_texture"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            offset: Vec2::ZERO,
            coverage: params.base_coverage,
            texture,
            texture_view,
            sampler,
        }
    }

    pub fn update(&mut self, dt: f32, wind: &Vec2, elapsed: f32, params: &CloudParams) {
        self.offset += *wind * dt * params.scroll_speed;
        self.coverage = params.base_coverage
            + params.coverage_variation * (elapsed * params.coverage_frequency).sin();
    }
}