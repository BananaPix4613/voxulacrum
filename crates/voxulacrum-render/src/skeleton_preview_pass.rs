//! Solid rendering of the tree skeleton debug preview.
//!
//! The preview is not a chunk, so it cannot ride `CapsulePass` or `CanopyPass` -
//! those are keyed by chunk coordinate and rebuilt from chunk content, which
//! would drop the preview the moment its chunk remeshed. This holds the same two
//! instance kinds in one place with a different lifetime: rebuilt when a panel
//! input moves, cleared when the preview is.
//!
//! It draws through the *same* pipelines the world uses, which is the whole
//! point - a preview rendered by a second code path is a preview of that path.

use bevy_ecs::prelude::Resource;
use wgpu::util::DeviceExt;

use crate::pipelines::{CapsuleInstanceGpu, LobeInstanceGpu};
use crate::versioned::{Uploaded, Versioned};

/// The preview's two instance streams, built on the main side by
/// `skeleton_debug_system`. Both empty when there is no preview.
#[derive(Default)]
pub struct PreviewInstances {
    pub capsules: Vec<CapsuleInstanceGpu>,
    pub lobes: Vec<LobeInstanceGpu>,
}

#[derive(Resource, Default)]
pub struct SkeletonPreviewPass {
    pub capsule_buffer: Option<wgpu::Buffer>,
    pub capsule_count: u32,
    pub lobe_buffer: Option<wgpu::Buffer>,
    pub lobe_count: u32,
    uploaded: Uploaded,
}

impl SkeletonPreviewPass {
    /// Replace both instance streams when the preview moved. Buffers are
    /// recreated rather than mapped and written: this runs only when a panel
    /// input changes, and the counts change with it.
    pub fn upload(&mut self, device: &wgpu::Device, source: &Versioned<PreviewInstances>) {
        let Some(preview) = self.uploaded.changed(source) else { return };
        self.capsule_count = preview.capsules.len() as u32;
        self.capsule_buffer = (!preview.capsules.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skeleton_preview_capsules"),
                contents: bytemuck::cast_slice(&preview.capsules),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.lobe_count = preview.lobes.len() as u32;
        self.lobe_buffer = (!preview.lobes.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("skeleton_preview_lobes"),
                contents: bytemuck::cast_slice(&preview.lobes),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
    }
}
