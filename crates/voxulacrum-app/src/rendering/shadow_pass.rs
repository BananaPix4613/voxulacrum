use crate::rendering::render_graph::{PassDecl, RenderPassNode, ResourceId, ResourceMap};
use crate::world::chunk::LoadedChunk;

/// Transient per-frame node - borrows from AppState.
pub struct ShadowPassNode<'a> {
    pub pipeline: &'a wgpu::RenderPipeline,
    pub bind_group: &'a wgpu::BindGroup,
    pub shadow_depth_view: &'a wgpu::TextureView,
    pub chunks: &'a [&'a LoadedChunk],
}

impl<'a> RenderPassNode for ShadowPassNode<'a> {
    fn declaration(&self) -> PassDecl {
        PassDecl {
            name: "shadow",
            reads: &[],
            writes: &[ResourceId::SHADOW_DEPTH],
        }
    }

    fn record(&self, encoder: &mut wgpu::CommandEncoder, _resources: &ResourceMap) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow_pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: self.shadow_depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(0, self.bind_group, &[]);
        for &chunk in self.chunks {
            if let Some(mesh) = &chunk.mesh {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }
    }
}