use crate::rendering::cap_pass::CapPass;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::frustum::Frustum;
use crate::rendering::render_graph::{PassDecl, RenderPassNode, ResourceId, ResourceMap};
use crate::rendering::detail_paint_pass::DetailPaintPass;
use crate::rendering::player_pass::PlayerPass;
use crate::rendering::scatter_pass::ScatterPass;
use crate::world::chunk::LoadedChunk;

/// Configuration for cross-section cap rendering within the main scene pass.
pub struct CapConfig {
    pub enabled: bool,
    // Cross-section clip plane; retained for the cap-rendering path.
    #[allow(dead_code)]
    pub clip_dirs: [f32; 3],
    #[allow(dead_code)]
    pub clip_pos: [f32; 3],
}

/// Transient per-frame node for the main scene pass.
pub struct MainScenePassNode<'a> {
    pub sky_color: wgpu::Color,
    pub terrain_pipeline: &'a wgpu::RenderPipeline,
    pub uniform_bind_group: &'a wgpu::BindGroup,
    pub chunks: &'a [&'a LoadedChunk],
    pub frustum: &'a Frustum,
    // Sub-passes
    pub cap_pass: &'a CapPass,
    pub cap_config: CapConfig,
    pub detail_paint_pass: &'a DetailPaintPass,
    pub detail_paint_pipeline: &'a wgpu::RenderPipeline,
    pub scatter_pass: &'a ScatterPass,
    pub scatter_pipeline: &'a wgpu::RenderPipeline,
    pub debug_line_pass: &'a DebugLinePass,
    pub show_debug_lines: bool,
    pub hide_foliage: bool,
    pub player_pass: &'a PlayerPass,
}

impl<'a> RenderPassNode for MainScenePassNode<'a> {
    fn declaration(&self) -> PassDecl {
        PassDecl {
            name: "main_scene",
            reads: &[ResourceId::SHADOW_DEPTH],
            writes: &[ResourceId::SCENE, ResourceId::NORMAL, ResourceId::DEPTH],
        }
    }

    fn record(&self, encoder: &mut wgpu::CommandEncoder, resources: &ResourceMap) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main_pass"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: resources.get(ResourceId::SCENE),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.sky_color),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: resources.get(ResourceId::NORMAL),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: resources.get(ResourceId::DEPTH),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Terrain
        pass.set_pipeline(self.terrain_pipeline);
        pass.set_bind_group(0, self.uniform_bind_group, &[]);
        for &chunk in self.chunks {
            if let Some(mesh) = &chunk.mesh {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
        }

        // Cross-section cap mesh
        if self.cap_config.enabled && self.cap_pass.index_count > 0 {
            pass.set_pipeline(&self.cap_pass.pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(
                0,
                self.cap_pass.vertex_buffer.as_ref().unwrap().slice(..),
            );
            pass.set_index_buffer(
                self.cap_pass.index_buffer.as_ref().unwrap().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..self.cap_pass.index_count, 0, 0..1);
        }

        // Player silhouette step 1 (R5): mark the stencil wherever the avatar is behind
        // terrain. Before foliage, so the mask reflects terrain occlusion only.
        if self.player_pass.visible && self.player_pass.body_index_count > 0 {
            pass.set_pipeline(&self.player_pass.silhouette_mark_pipeline);
            pass.set_stencil_reference(1);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.player_pass.vertex_buffer.slice(..));
            pass.set_index_buffer(self.player_pass.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.player_pass.body_index_count, 0, 0..1);
        }

        // Tier-1 detail paint - GPU-generated blades from per-chunk density buffers
        if !self.hide_foliage && !self.detail_paint_pass.chunk_paint.is_empty() {
            pass.set_pipeline(self.detail_paint_pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.detail_paint_pass.blade_vertex_buffer.slice(..));
            pass.set_index_buffer(
                self.detail_paint_pass.blade_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            for (&chunk_pos, chunk) in &self.detail_paint_pass.chunk_paint {
                if self.frustum.is_chunk_visible(chunk_pos) {
                    pass.set_bind_group(1, &chunk.bind_group, &[]);
                    pass.draw_indexed(
                        0..self.detail_paint_pass.blade_index_count,
                        0,
                        0..crate::rendering::detail_paint_pass::INSTANCES_PER_CHUNK,
                    );
                }
            }
        }

        // Tier-2/3 scatter - one instanced draw per prefab mesh, grouped by prefab
        if !self.hide_foliage && !self.scatter_pass.chunk_scatter.is_empty() {
            pass.set_pipeline(self.scatter_pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            for (prefab_idx, mesh) in self.scatter_pass.prefab_meshes.iter().enumerate() {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for (&chunk_pos, sc) in &self.scatter_pass.chunk_scatter {
                    if !self.frustum.is_chunk_visible(chunk_pos) {
                        continue;
                    }
                    if let Some(Some(insts)) = sc.by_prefab.get(prefab_idx) {
                        pass.set_vertex_buffer(1, insts.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..insts.instance_count);
                    }
                }
            }
        }

        // Player silhouette step 2: composite where the stencil was marked, depth testing
        // off so it draws over foliage. Before the normal avatar, so the visible player
        // still wins wherever it isn't actually occluded.
        if self.player_pass.visible && self.player_pass.body_index_count > 0 {
            pass.set_pipeline(&self.player_pass.silhouette_pipeline);
            pass.set_stencil_reference(1);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.player_pass.vertex_buffer.slice(..));
            pass.set_index_buffer(self.player_pass.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.player_pass.body_index_count, 0, 0..1);
        }

        // Player avatar + drop shadow (between foliage and water).
        if self.player_pass.visible && self.player_pass.index_count > 0 {
            pass.set_pipeline(&self.player_pass.pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.player_pass.vertex_buffer.slice(..));
            pass.set_index_buffer(self.player_pass.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.player_pass.index_count, 0, 0..1);
        }

        // Debug lines
        if self.show_debug_lines {
            if let Some(ref vb) = self.debug_line_pass.vertex_buffer {
                pass.set_pipeline(&self.debug_line_pass.pipeline);
                pass.set_bind_group(0, self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.draw(0..self.debug_line_pass.vertex_count, 0..1);
            }
        }

        // Pick highlight box - drawn whenever a voxel is picked (8b).
        if let Some(ref vb) = self.debug_line_pass.highlight_buffer {
            pass.set_pipeline(&self.debug_line_pass.pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.draw(0..self.debug_line_pass.highlight_count, 0..1);
        }
        
        // Blueprint capture region - drawn whenever the panel holds a selection.
        if let Some(ref vb) = self.debug_line_pass.selection_buffer {
            pass.set_pipeline(&self.debug_line_pass.pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.draw(0..self.debug_line_pass.selection_count, 0..1);
        }
    }
}
