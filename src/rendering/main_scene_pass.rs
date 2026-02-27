use glam::IVec3;

use crate::rendering::cap_pass::CapPass;
use crate::rendering::debug_lines::DebugLinePass;
use crate::rendering::frustum::Frustum;
use crate::rendering::render_graph::{PassDecl, RenderPassNode, ResourceId, ResourceMap};
use crate::rendering::vegetation_pass::VegetationPass;
use crate::rendering::water_pass::WaterPass;
use crate::world::chunk::Chunk;

/// Configuration for cross-section cap rendering within the main scene pass.
pub struct CapConfig {
    pub enabled: bool,
    pub clip_dirs: [f32; 3],
    pub clip_pos: [f32; 3],
}

/// Transient per-frame node for the main scene pass.
pub struct MainScenePassNode<'a> {
    pub sky_color: wgpu::Color,
    pub terrain_pipeline: &'a wgpu::RenderPipeline,
    pub uniform_bind_group: &'a wgpu::BindGroup,
    pub chunks: &'a [Chunk],
    pub frustum: &'a Frustum,
    // Sub-passes
    pub cap_pass: &'a CapPass,
    pub cap_config: CapConfig,
    pub vegetation_pass: &'a VegetationPass,
    pub vegetation_pipeline: &'a wgpu::RenderPipeline,
    pub water_pass: &'a WaterPass,
    pub water_pipeline: &'a wgpu::RenderPipeline,
    pub debug_line_pass: &'a DebugLinePass,
    pub show_debug_lines: bool,
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
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Terrain
        pass.set_pipeline(self.terrain_pipeline);
        pass.set_bind_group(0, self.uniform_bind_group, &[]);
        for chunk in self.chunks {
            if let Some(mesh) = &chunk.mesh {
                if self.frustum.is_chunk_visible(chunk.position) {
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        mesh.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
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

        // Vegetation
        if self.vegetation_pass.instance_count > 0 {
            pass.set_pipeline(self.vegetation_pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vegetation_pass.grass_vertex_buffer.slice(..));
            pass.set_vertex_buffer(1, self.vegetation_pass.instance_buffer.slice(..));
            pass.set_index_buffer(
                self.vegetation_pass.grass_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(
                0..self.vegetation_pass.grass_index_count,
                0,
                0..self.vegetation_pass.instance_count,
            );
        }

        // Water
        if self.water_pass.index_count > 0 {
            pass.set_pipeline(self.water_pipeline);
            pass.set_bind_group(0, self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.water_pass.vertex_buffer.slice(..));
            pass.set_index_buffer(
                self.water_pass.index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..self.water_pass.index_count, 0, 0..1);
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
    }
}