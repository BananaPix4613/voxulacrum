use bevy_ecs::prelude::Resource;
use wgpu::TextureFormat;

/// Bundles the immutable GPU handles used by all rendering operations.
/// 
/// `Device` and `Queue` are internally reference-counted (`Arc`), so
/// `Clone` is cheap - just bumps the refcount. This lets exclusive
/// systems clone the context out of the ECS World to avoid borrow conflicts.
#[derive(Clone, Resource)]
pub struct RenderContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface_format: TextureFormat,
}