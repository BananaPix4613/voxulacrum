//! Overlays drawn over the world, built on the main side: the chunk boundaries,
//! the pick highlight, the authoring selection, free-form debug lines, and the
//! tree skeleton preview's instances. `DebugLinePass` and `SkeletonPreviewPass`
//! upload them.

use bevy_ecs::prelude::Resource;
use glam::{IVec3, Vec3};

use crate::rendering::debug_lines::DebugLineVertex;
use crate::rendering::frustum::Frustum;
use crate::rendering::skeleton_preview_pass::PreviewInstances;
use crate::rendering::versioned::Versioned;
use crate::world::chunk::{CHUNK_WORLD_SIZE, VOXEL_SCALE};

/// The four line lists the debug-line pass draws.
#[derive(Resource, Default)]
pub struct DebugLines {
    /// Chunk boundaries, drawn while `show_chunk_boundaries` is on.
    pub boundaries: Versioned<Vec<DebugLineVertex>>,
    /// The single-voxel pick highlight, drawn independently of the boundaries.
    pub highlight: Versioned<Vec<DebugLineVertex>>,
    /// The authoring-region outline (blueprint capture selection).
    pub selection: Versioned<Vec<DebugLineVertex>>,
    /// Free-form world-space lines for debug views.
    pub overlay: Versioned<Vec<DebugLineVertex>>,
    /// The corners `selection` was built from, so a frame that changes nothing
    /// rebuilds nothing. Unlike the pick highlight, which moves with the cursor,
    /// a selection changes on a click.
    selection_key: Option<(IVec3, IVec3)>,
}

impl DebugLines {
    /// Set (or clear) the pick highlight box around a single world voxel.
    pub fn set_highlight(&mut self, anchor: Option<IVec3>, color: [f32; 3]) {
        self.highlight.set(anchor.map(|v| voxel_box_lines(v, color)).unwrap_or_default());
    }

    /// Set (or clear) the authoring-region outline spanning two world voxels,
    /// inclusive. A no-op when the corners are unchanged since the last call.
    pub fn set_selection(&mut self, corners: Option<(IVec3, IVec3)>, color: [f32; 3]) {
        if self.selection_key == corners {
            return;
        }
        self.selection_key = corners;
        self.selection
            .set(corners.map(|(a, b)| region_box_lines(a, b, color)).unwrap_or_default());
    }

    /// Set a free-form world-space line overlay; an empty list clears it.
    ///
    /// Unlike `set_selection` this holds no key and rebuilds on every call:
    /// the geometry is a whole vertex list rather than two corners, so
    /// comparing it would cost more than rebuilding it. **The caller owns the
    /// change detection** and is expected to call only when its own inputs
    /// moved.
    pub fn set_overlay(&mut self, lines: Vec<DebugLineVertex>) {
        self.overlay.set(lines);
    }
}

/// The tree skeleton preview's instances, drawn by `SkeletonPreviewPass`.
#[derive(Resource, Default)]
pub struct SkeletonPreview(pub Versioned<PreviewInstances>);

/// Generates 12 line edges per chunk AABB.
/// Visible chunks are green, culled chunks are red.
pub fn generate_chunk_boundary_lines(
    chunk_positions: &[IVec3],
    frustum: &Frustum,
) -> Vec<DebugLineVertex> {
    let mut lines = Vec::with_capacity(chunk_positions.len() * 24); // 12 edges * 2 verts

    for &pos in chunk_positions {
        let min = Vec3::new(
            pos.x as f32 * CHUNK_WORLD_SIZE,
            pos.y as f32 * CHUNK_WORLD_SIZE,
            pos.z as f32 * CHUNK_WORLD_SIZE,
        );
        let max = min + Vec3::splat(CHUNK_WORLD_SIZE);
        let visible = frustum.intersects_aabb(min, max);
        let color: [f32; 3] = if visible {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };

        let corners = [
            [min.x, min.y, min.z], // 0
            [max.x, min.y, min.z], // 1
            [max.x, min.y, max.z], // 2
            [min.x, min.y, max.z], // 3
            [min.x, max.y, min.z], // 4
            [max.x, max.y, min.z], // 5
            [max.x, max.y, max.z], // 6
            [min.x, max.y, max.z], // 7
        ];

        let edges: [(usize, usize); 12] = [
            (0, 1), (1, 2), (2, 3), (3, 0), // bottom
            (4, 5), (5, 6), (6, 7), (7, 4), // top
            (0, 4), (1, 5), (2, 6), (3, 7), // verticals
        ];

        for (a, b) in edges {
            lines.push(DebugLineVertex { position: corners[a], color });
            lines.push(DebugLineVertex { position: corners[b], color });
        }
    }

    lines
}

/// 12-edge wireframe box (24 verts) spanning `[min, max]` in one color.
fn box_edges(min: Vec3, max: Vec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let corners = [
        [min.x, min.y, min.z],
        [max.x, min.y, min.z],
        [max.x, min.y, max.z],
        [min.x, min.y, max.z],
        [min.x, max.y, min.z],
        [max.x, max.y, min.z],
        [max.x, max.y, max.z],
        [min.x, max.y, max.z],
    ];
    let edges: [(usize, usize); 12] = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    let mut lines = Vec::with_capacity(24);
    for (a, b) in edges {
        lines.push(DebugLineVertex { position: corners[a], color });
        lines.push(DebugLineVertex { position: corners[b], color });
    }
    lines
}

/// Wireframe box hugging a single world voxel, slightly outset to avoid z-fighting.
pub fn voxel_box_lines(v: IVec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let s = VOXEL_SCALE;
    let eps = s * 0.03;
    let min = Vec3::new(v.x as f32 * s - eps, v.y as f32 * s - eps, v.z as f32 * s - eps);
    let max = Vec3::new(
        (v.x as f32 + 1.0) * s + eps,
        (v.y as f32 + 1.0) * s + eps,
        (v.z as f32 + 1.0) * s + eps,
    );
    box_edges(min, max, color)
}

/// Inclusive world-voxel box outline, expanded to cover `max`'s full cell.
fn region_box_lines(a: IVec3, b: IVec3, color: [f32; 3]) -> Vec<DebugLineVertex> {
    let s = VOXEL_SCALE;
    // Slightly larger than the pick highlight's epsilon, so a one-voxel
    // selection reads as a distinct box rather than z-fighting with it.
    let eps = s * 0.06;
    let (lo, hi) = (a.min(b), a.max(b));
    let min = Vec3::new(lo.x as f32 * s - eps, lo.y as f32 * s - eps, lo.z as f32 * s - eps);
    let max = Vec3::new(
        (hi.x as f32 + 1.0) * s + eps,
        (hi.y as f32 + 1.0) * s + eps,
        (hi.z as f32 + 1.0) * s + eps,
    );
    box_edges(min, max, color)
}
