//! The cross-section cap mesh: one quad per solid (or surface-crossing) voxel
//! cell on each active clip plane, so a cut reads as solid rather than hollow.
//! Rebuilt on the main side when the cut moves; `CapPass` uploads it.

use bevy_ecs::prelude::Resource;
use glam::IVec3;

use crate::params::CrossSectionParams;
use crate::rendering::cap_pass::{CapMeshData, CapVertex};
use crate::rendering::versioned::Versioned;
use crate::world::chunk::{CHUNK_SIZE, VOXEL_SCALE};
use crate::world::World;

/// The cap mesh and the inputs it was last built from.
#[derive(Resource, Default)]
pub struct CapMesh {
    pub mesh: Versioned<CapMeshData>,
    // Invalidation cache
    last_voxel_planes: [Option<i32>; 3],
    last_fog_color: [f32; 3],
    last_clip_dirs: [f32; 3],
    last_enabled: bool,
}

impl CapMesh {
    /// Rebuild the cap mesh if the clip plane crossed a voxel boundary, color changed,
    /// or clip direction changed (camera rotation).
    pub fn maybe_rebuild(
        &mut self,
        world: &World,
        cs: &CrossSectionParams,
        clip_pos: [f32; 3],
        clip_dirs: [f32; 3],
    ) {
        if !cs.enabled {
            if self.last_enabled {
                self.mesh.set(CapMeshData::default());
                self.last_enabled = false;
                self.last_voxel_planes = [None; 3];
            }
            return;
        }

        // Compute snapped voxel planes for each active axis.
        // For +dir we use floor (cap sits at the low side of the clipped voxel),
        // for -dir we use ceil (cap sits at the high side).
        let planes = [
            if cs.x_offset > 0.0 {
                if clip_dirs[0] > 0.0 {
                    Some((clip_pos[0] / VOXEL_SCALE).floor() as i32)
                } else {
                    Some((clip_pos[0] / VOXEL_SCALE).ceil() as i32)
                }
            } else { None },
            if cs.y_offset > 0.0 {
                Some((clip_pos[1] / VOXEL_SCALE).floor() as i32)
            } else { None },
            if cs.z_offset > 0.0 {
                if clip_dirs[2] > 0.0 {
                    Some((clip_pos[2] / VOXEL_SCALE).floor() as i32)
                } else {
                    Some((clip_pos[2] / VOXEL_SCALE).ceil() as i32)
                }
            } else { None },
        ];

        let color_changed = cs.fog_color != self.last_fog_color;
        let planes_changed = planes != self.last_voxel_planes;
        let dirs_changed = clip_dirs != self.last_clip_dirs;

        if !planes_changed && !color_changed && !dirs_changed && self.last_enabled {
            return; // No rebuild needed
        }

        let (vertices, indices) = generate_cap_mesh(world, &planes, clip_dirs, cs.fog_color);
        self.mesh.set(CapMeshData { vertices, indices });

        self.last_voxel_planes = planes;
        self.last_fog_color = cs.fog_color;
        self.last_clip_dirs = clip_dirs;
        self.last_enabled = true;
    }
}

fn loaded_voxel_bounds(world: &World) -> (i32, i32, i32, i32, i32, i32) {
    // Returns (min_vx, max_vx, min_vy, max_vy, min_vz, max_vz) in voxel coords
    if world.chunks.is_empty() {
        return (0, 0, 0, 0, 0, 0);
    }
    let mut min = IVec3::splat(i32::MAX);
    let mut max = IVec3::splat(i32::MIN);
    for coord in world.chunks.keys() {
        let pos = IVec3::from(*coord);
        min = min.min(pos);
        max = max.max(pos);
    }
    (
        min.x * CHUNK_SIZE as i32,
        (max.x + 1) * CHUNK_SIZE as i32,
        min.y * CHUNK_SIZE as i32,
        (max.y + 1) * CHUNK_SIZE as i32,
        min.z * CHUNK_SIZE as i32,
        (max.z + 1) * CHUNK_SIZE as i32,
    )
}

// is_solid_at and has_mc_surface_at use IVec3 lookup:
fn is_solid_at(world: &World, vx: i32, vy: i32, vz: i32) -> bool {
    let cx = vx.div_euclid(CHUNK_SIZE as i32);
    let cy = vy.div_euclid(CHUNK_SIZE as i32);
    let cz = vz.div_euclid(CHUNK_SIZE as i32);
    let lx = vx.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = vy.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = vz.rem_euclid(CHUNK_SIZE as i32) as usize;
    match world.get_chunk(IVec3::new(cx, cy, cz)) {
        Some(chunk) => chunk.is_solid(lx, ly, lz),
        None => false,
    }
}

/// Check if the marching cubes cell at absolute voxel coordinates has surface
/// geometry (at least one solid and one non-solid corner among its 8 corners).
/// This detects transitional cells where MC creates diagonal faces that extend
/// beyond the solid voxel center into "air" voxels.
fn has_mc_surface_at(world: &World, vx: i32, vy: i32, vz: i32) -> bool {
    let mut has_solid = false;
    let mut has_air = false;

    // Check all 8 corners of the MC cell at (vx, vy, vz).
    // Corners span from (vx, vy, vz) to (vx+1, vy+1, vz+1),
    // matching CORNER_OFFSETS in marching_cubes.rs.
    for dz in 0..=1i32 {
        for dy in 0..=1i32 {
            for dx in 0..=1i32 {
                if is_solid_at(world, vx + dx, vy + dy, vz + dz) {
                    has_solid = true;
                } else {
                    has_air = true;
                }

                if has_solid && has_air {
                    return true;
                }
            }
        }
    }

    false
}

/// Generate cap mesh geometry for all active clip planes.
fn generate_cap_mesh(
    world: &World,
    planes: &[Option<i32>; 3],
    clip_dirs: [f32; 3],
    fog_color: [f32; 3],
) -> (Vec<CapVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let (min_vx, max_vx, min_vy, max_vy, min_vz, max_vz) = loaded_voxel_bounds(world);

    // X clip plane
    if let Some(vx) = planes[0] {
        if vx >= min_vx && vx < max_vx {
            generate_plane_x(
                world, vx, min_vy, max_vy, min_vz, max_vz,
                clip_dirs[0], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    // Y clip plane
    if let Some(vy) = planes[1] {
        if vy >= min_vy && vy < max_vy {
            generate_plane_y(
                world, vy, min_vx, max_vx, min_vz, max_vz,
                clip_dirs[1], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    // Z clip plane
    if let Some(vz) = planes[2] {
        if vz >= min_vz && vz < max_vz {
            generate_plane_z(
                world, vz, min_vx, max_vx, min_vy, max_vy,
                clip_dirs[2], fog_color, &mut vertices, &mut indices,
            );
        }
    }

    (vertices, indices)
}

/// Generate cap quads for the X clip plane.
/// `dir` is +1.0 (clip +X side, normal faces +X) or -1.0 (clip -X side, normal faces -X).
fn generate_plane_x(
    world: &World,
    vx: i32,
    min_vy: i32, max_vy: i32,
    min_vz: i32, max_vz: i32,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [dir, 0.0, 0.0];
    let wx = vx as f32 * VOXEL_SCALE;

    for vz in min_vz..max_vz {
        for vy in min_vy..max_vy {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let y0 = vy as f32 * VOXEL_SCALE;
                let y1 = y0 + VOXEL_SCALE;
                let z0 = vz as f32 * VOXEL_SCALE;
                let z1 = z0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [wx, y0, z0], normal, color });
                vertices.push(CapVertex { position: [wx, y1, z0], normal, color });
                vertices.push(CapVertex { position: [wx, y1, z1], normal, color });
                vertices.push(CapVertex { position: [wx, y0, z1], normal, color });

                if dir > 0.0 {
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}

/// Generate cap quads for the Y clip plane.
/// `dir` is +1.0 (clip +Y side, normal faces +Y) or -1.0 (clip -Y side, normal faces -Y).
fn generate_plane_y(
    world: &World,
    vy: i32,
    min_vx: i32, max_vx: i32,
    min_vz: i32, max_vz: i32,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [0.0, dir, 0.0];
    let wy = vy as f32 * VOXEL_SCALE;

    for vz in min_vz..max_vz {
        for vx in min_vx..max_vx {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let x0 = vx as f32 * VOXEL_SCALE;
                let x1 = x0 + VOXEL_SCALE;
                let z0 = vz as f32 * VOXEL_SCALE;
                let z1 = z0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [x0, wy, z0], normal, color });
                vertices.push(CapVertex { position: [x0, wy, z1], normal, color });
                vertices.push(CapVertex { position: [x1, wy, z1], normal, color });
                vertices.push(CapVertex { position: [x1, wy, z0], normal, color });

                if dir > 0.0 {
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}

/// Generate cap quads for the Z clip plane.
/// `dir` is +1.0 (clip +Z side, normal faces +Z) or -1.0 (clip -Z side, normal faces -Z).
fn generate_plane_z(
    world: &World,
    vz: i32,
    min_vx: i32, max_vx: i32,
    min_vy: i32, max_vy: i32,
    dir: f32,
    color: [f32; 3],
    vertices: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
) {
    let normal = [0.0, 0.0, dir];
    let wz = vz as f32 * VOXEL_SCALE;

    for vy in min_vy..max_vy {
        for vx in min_vx..max_vx {
            if is_solid_at(world, vx, vy, vz) || has_mc_surface_at(world, vx, vy, vz) {
                let x0 = vx as f32 * VOXEL_SCALE;
                let x1 = x0 + VOXEL_SCALE;
                let y0 = vy as f32 * VOXEL_SCALE;
                let y1 = y0 + VOXEL_SCALE;

                let base = vertices.len() as u32;
                vertices.push(CapVertex { position: [x0, y0, wz], normal, color });
                vertices.push(CapVertex { position: [x1, y0, wz], normal, color });
                vertices.push(CapVertex { position: [x1, y1, wz], normal, color });
                vertices.push(CapVertex { position: [x0, y1, wz], normal, color });

                if dir > 0.0 {
                    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                } else {
                    indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
}
