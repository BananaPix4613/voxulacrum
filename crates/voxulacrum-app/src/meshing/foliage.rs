//! The client's foliage builders: what each chunk's detail paint, scatter, tree
//! capsules and canopy lobes look like to the GPU, built from the chunk on the
//! CPU. The passes in `rendering/` upload the results and read no chunk.

use crate::foliage::FoliageRegistry;
use crate::prefabs::{PrefabRegistry, PrefabShape};
use crate::rendering::detail_paint_pass::{ColumnTexel, DetailPaintData};
use crate::rendering::pipelines::{
    CapsuleInstanceGpu, LobeInstanceGpu, ScatterInstanceGpu, ScatterVertex,
};
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, VOXEL_SCALE};
use crate::world::overrides::ChunkOverrides;
use voxel_core::ShapeId;

const CHUNK_AREA: usize = CHUNK_SIZE * CHUNK_SIZE;

/// Build the per-column density records from a chunk's first detail layer.
/// `None` if the chunk has no paint to draw.
pub fn build_detail_paint(
    chunk: &LoadedChunk,
    foliage: &FoliageRegistry,
) -> Option<DetailPaintData> {
    let layer = chunk.data.detail_layers.layers.first()?;

    let mut any = false;
    let mut columns = Vec::with_capacity(CHUNK_AREA);
    for idx in 0..CHUNK_AREA {
        let lx = idx % CHUNK_SIZE;
        let lz = idx / CHUNK_SIZE;
        let texel = layer.map[idx];
        if texel.density == 0 {
            columns.push(ColumnTexel { packed: 0, surface_top: 0.0 });
            continue;
        }
        // Topmost solid voxel in this column = the painted surface. Use the
        // voxel's shape so blades sit on a slab's top face, not the cell top.
        let mut surface_top = None;
        for ly in (0..CHUNK_SIZE).rev() {
            if chunk.is_solid(lx, ly, lz) {
                let cell_top = match chunk.voxel(lx, ly, lz).shape {
                    ShapeId::SlabBottom => ly as f32 + 0.5,
                    _ => ly as f32 + 1.0,
                };
                surface_top = Some(cell_top);
                break;
            }
        }
        match surface_top {
            Some(top) => {
                let packed = texel.density as u32
                    | ((texel.species as u32) << 8)
                    | ((texel.tint as u32) << 16);
                columns.push(ColumnTexel { packed, surface_top: top });
                any = true;
            }
            None => columns.push(ColumnTexel { packed: 0, surface_top: 0.0 }),
        }
    }
    if !any {
        return None;
    }

    let c = chunk.data.coord;
    let span = CHUNK_SIZE as f32 * VOXEL_SCALE;
    Some(DetailPaintData {
        columns,
        origin: [c.x as f32 * span, c.y as f32 * span, c.z as f32 * span],
        layer_slot: foliage.layer_slot(layer.layer_id).unwrap_or(u32::MAX),
    })
}

/// Group a chunk's effective scatter into per-prefab instance lists
/// (index = `PrefabId`). Instances with an unknown prefab draw the content's
/// placeholder. `None` if the chunk has no scatter.
pub fn build_scatter(
    chunk: &LoadedChunk,
    prefabs: &PrefabRegistry,
) -> Option<Vec<Vec<ScatterInstanceGpu>>> {
    let mut grouped: Vec<Vec<ScatterInstanceGpu>> = vec![Vec::new(); prefabs.len()];

    let generated = &chunk.data.scatter_instances;
    let no_overrides = ChunkOverrides::default();
    let overrides = chunk.data.edit_state.bucket().unwrap_or(&no_overrides);
    let c = chunk.data.coord;
    let span = CHUNK_SIZE as f32 * VOXEL_SCALE;
    let origin = [c.x as f32 * span, c.y as f32 * span, c.z as f32 * span];

    for si in overrides.effective_scatter(generated) {
        // An id this table does not define (an instance saved or generated
        // against different content) draws the placeholder the content declares.
        let (prefab_idx, def) = match prefabs.get(si.prefab_id) {
            Some(def) => (si.prefab_id.0 as usize, def),
            None => {
                let placeholder = prefabs.placeholder();
                let def = prefabs.get(placeholder).expect("the placeholder names a prefab");
                (placeholder.0 as usize, def)
            }
        };

        let (ax, ay, az) = (si.anchor.x as usize, si.anchor.y as usize, si.anchor.z as usize);
        // sub_offset encodes the in-cell fraction: frac = (s + 128) / 256.
        let frac_x = (si.sub_offset[0] as f32 + 128.0) / 256.0;
        let frac_z = (si.sub_offset[2] as f32 + 128.0) / 256.0;
        let top = match chunk.voxel(ax, ay, az).shape {
            ShapeId::SlabBottom => ay as f32 + 0.5,
            _ => ay as f32 + 1.0,
        };
        let position = [
            origin[0] + (ax as f32 + frac_x) * VOXEL_SCALE,
            origin[1] + top * VOXEL_SCALE,
            origin[2] + (az as f32 + frac_z) * VOXEL_SCALE,
        ];
        let rotation_y = si.rotation_y as f32 / 256.0 * std::f32::consts::TAU;
        // Prefab base scale plus a little per-instance variation.
        let scale = def.scale * (0.85 + (si.scale_variant as f32 / 255.0) * 0.3);

        grouped[prefab_idx].push(ScatterInstanceGpu {
            position,
            rotation_y,
            color: def.color,
            scale,
        });
    }

    if grouped.iter().all(|g| g.is_empty()) {
        return None;
    }
    Some(grouped)
}

/// A chunk's branch capsules as instances. `None` when the chunk owns none, so
/// an empty chunk costs no buffer.
pub fn build_capsules(chunk: &LoadedChunk) -> Option<Vec<CapsuleInstanceGpu>> {
    if chunk.segments.is_empty() {
        return None;
    }
    Some(
        chunk
            .segments
            .iter()
            .map(|s| CapsuleInstanceGpu {
                // Segments are in voxel units; the render world scales by
                // VOXEL_SCALE, exactly as scatter does for its anchors.
                a: (s.a * VOXEL_SCALE).to_array(),
                ra: s.ra * VOXEL_SCALE,
                b: (s.b * VOXEL_SCALE).to_array(),
                rb: s.rb * VOXEL_SCALE,
            })
            .collect(),
    )
}

/// A chunk's canopy lobes as instances. `None` when the chunk owns none, so a
/// treeless chunk costs no buffer.
pub fn build_lobes(chunk: &LoadedChunk) -> Option<Vec<LobeInstanceGpu>> {
    if chunk.lobes.is_empty() {
        return None;
    }
    Some(
        chunk
            .lobes
            .iter()
            .map(|l| LobeInstanceGpu {
                // Lobes are in voxel units, as segments are; the render world
                // scales by VOXEL_SCALE. Radii scale with position because the
                // scale is uniform.
                center: (l.center * VOXEL_SCALE).to_array(),
                radii: (l.radii * VOXEL_SCALE).to_array(),
            })
            .collect(),
    )
}

/// One mesh per prefab, in `PrefabId` order, for `ScatterPass::new`.
pub fn prefab_meshes(prefabs: &PrefabRegistry) -> Vec<(Vec<ScatterVertex>, Vec<u32>)> {
    prefabs.iter().map(|def| prefab_mesh(def.shape)).collect()
}

/// Procedural mesh for a prefab shape (position + normal; color is per-instance).
/// Local coords sit in x,z ∈ [-0.5, 0.5], y ∈ [0, ~1]; the instance scale is
/// applied at draw time. Drawn with cull disabled so crossed quads show both sides.
fn prefab_mesh(shape: PrefabShape) -> (Vec<ScatterVertex>, Vec<u32>) {
    match shape {
        PrefabShape::Rock => rock_mesh(),
        PrefabShape::Bush => crossed_quads(2, 0.9, 0.75),
        PrefabShape::GrassTuft => crossed_quads(3, 0.28, 1.0),
    }
}

/// `n` vertical quads evenly rotated around Y - a crossed-quad clump. Normal
/// points up for soft foliage shading.
fn crossed_quads(n: usize, width: f32, height: f32) -> (Vec<ScatterVertex>, Vec<u32>) {
    let hw = width * 0.5;
    let normal = [0.0, 1.0, 0.0];
    let mut verts = Vec::with_capacity(n * 4);
    let mut indices = Vec::with_capacity(n * 6);
    for i in 0..n {
        let theta = i as f32 * std::f32::consts::PI / n as f32;
        let (s, c) = theta.sin_cos();
        let (dx, dz) = (c * hw, s * hw);
        let base = verts.len() as u32;
        verts.push(ScatterVertex { position: [-dx, 0.0, -dz], normal });
        verts.push(ScatterVertex { position: [dx, 0.0, dz], normal });
        verts.push(ScatterVertex { position: [dx, height, dz], normal });
        verts.push(ScatterVertex { position: [-dx, height, -dz], normal });
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, indices)
}

/// A low truncated pyramid - a chunky boulder. Normals point outward from the center.
fn rock_mesh() -> (Vec<ScatterVertex>, Vec<u32>) {
    let (b, t, h) = (0.5, 0.32, 0.55);
    let base = [[-b, 0.0, -b], [b, 0.0, -b], [b, 0.0, b], [-b, 0.0, b]];
    let top = [[-t, h, -t], [t, h, -t], [t, h, t], [-t, h, t]];
    let center = [0.0, h * 0.5, 0.0];

    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        push_outward_quad(&mut verts, &mut indices, [base[i], base[j], top[j], top[i]], center);
    }
    push_outward_quad(&mut verts, &mut indices, [top[0], top[1], top[2], top[3]], center);
    (verts, indices)
}

fn push_outward_quad(
    verts: &mut Vec<ScatterVertex>,
    indices: &mut Vec<u32>,
    quad: [[f32; 3]; 4],
    center: [f32; 3],
) {
    let i0 = verts.len() as u32;
    for p in quad {
        let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        verts.push(ScatterVertex { position: p, normal: [d[0] / len, d[1] / len, d[2] / len] });
    }
    indices.extend_from_slice(&[i0, i0 + 1, i0 + 2, i0, i0 + 2, i0 + 3]);
}
