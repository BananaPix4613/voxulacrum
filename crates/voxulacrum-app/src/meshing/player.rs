//! The player avatar mesh: a capsule the size of the player's body, a facing
//! visor and a drop shadow, rebuilt each frame on the main side by
//! `player_render_prep_system`. `PlayerPass` uploads it.

use bevy_ecs::prelude::Resource;
use glam::{Vec2, Vec3};

use crate::rendering::cap_pass::CapVertex;
use crate::rendering::player_pass::PlayerMeshData;
use crate::rendering::versioned::Versioned;

/// This frame's avatar mesh; `None` hides the avatar.
#[derive(Resource, Default)]
pub struct PlayerMesh(pub Versioned<Option<PlayerMeshData>>);

/// Build the avatar mesh: a capsule the size of the player's `body`, a facing
/// visor on the front, and a circular hard drop shadow at `ground_y` (the first
/// solid surface below the player) when present.
pub fn build_player_mesh(
    feet: Vec3,
    facing: f32,
    ground_y: Option<f32>,
    body: &crate::play::PlayerTuning,
) -> PlayerMeshData {
    let mut verts: Vec<CapVertex> = Vec::with_capacity(256);
    let mut indices: Vec<u32> = Vec::with_capacity(1024);

    let radius = body.half_width;
    let total_height = body.height;
    let cyl_height = total_height - 2.0 * radius;
    build_capsule(&mut verts, &mut indices, feet, radius, cyl_height, 12, 5, [0.30, 0.55, 0.75]);

    // Facing visor ("eyes") on the front, oriented to the look direction.
    let facing_dir = Vec2::new(facing.cos(), facing.sin());
    let eye_center = feet + Vec3::new(
        facing_dir.x * radius * 0.96,
        total_height * 0.72,
        facing_dir.y * radius * 0.96,
    );
    push_visor(&mut verts, &mut indices, eye_center, facing_dir, 0.40, 0.18, [0.05, 0.05, 0.08]);

    // Index count for the body alone (capsule + visor). The drop shadow is appended
    // after this so the silhouette pass can draw the body without it.
    let body_index_count = indices.len() as u32;
    if let Some(gy) = ground_y {
        push_disc(&mut verts, &mut indices, feet.x, gy + 0.02, feet.z, 0.42, 20, [0.04, 0.04, 0.05]);
    }

    PlayerMeshData { vertices: verts, indices, body_index_count }
}

/// A capsule (two hemispheres joined by a cylinder) whose base (feet) is at
/// `base`, spanning `base.y .. base.y + 2*radius + cyl_height`. Smooth normals.
fn build_capsule(
    verts: &mut Vec<CapVertex>,
    indices: &mut Vec<u32>,
    base: Vec3,
    radius: f32,
    cyl_height: f32,
    segments: usize,
    rings: usize,
    color: [f32; 3],
) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let bottom_c = base.y + radius;
    let top_c = base.y + radius + cyl_height;

    // Ring latitudes: bottom hemisphere (-π/2..0), then top hemisphere (0..π/2).
    // The two φ=0 equator rings sit at bottom_c and top_c -> the cylinder wall.
    let mut ring_lat: Vec<(f32, f32)> = Vec::with_capacity(2 * (rings + 1)); // (y, ring_radius)
    let mut ring_ny: Vec<f32> = Vec::with_capacity(2 * (rings + 1));
    for i in 0..=rings {
        let phi = -FRAC_PI_2 + (i as f32 / rings as f32) * FRAC_PI_2;
        ring_lat.push((bottom_c + radius * phi.sin(), radius * phi.cos()));
        ring_ny.push(phi.sin());
    }
    for i in 0..=rings {
        let phi = (i as f32 / rings as f32) * FRAC_PI_2;
        ring_lat.push((top_c + radius * phi.sin(), radius * phi.cos()));
        ring_ny.push(phi.sin());
    }

    let start = verts.len() as u32;
    let stride = (segments + 1) as u32;
    for (ri, &(y, rr)) in ring_lat.iter().enumerate() {
        let ny = ring_ny[ri];
        let horiz = (1.0 - ny * ny).max(0.0).sqrt(); // |cos φ| for the normal's xz scale
        for j in 0..=segments {
            let theta = j as f32 / segments as f32 * TAU;
            let (st, ct) = theta.sin_cos();
            verts.push(CapVertex {
                position: [base.x + rr * ct, y, base.z + rr * st],
                normal: [horiz * ct, ny, horiz * st],
                color,
            });
        }
    }
    let ring_count = ring_lat.len() as u32;
    for i in 0..ring_count - 1 {
        for j in 0..segments as u32 {
            let a = start + i * stride + j;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            indices.extend(&[a, c, b, b, c, d]);
        }
    }
}

/// A small quad facing outward along `facing_dir` (the player's eyes/visor).
fn push_visor(verts: &mut Vec<CapVertex>, indices: &mut Vec<u32>, center: Vec3, facing_dir: Vec2, width: f32, height: f32, color: [f32; 3]) {
    let right = Vec3::new(-facing_dir.y, 0.0, facing_dir.x) * (width * 0.5);
    let up = Vec3::new(0.0, height * 0.5, 0.0);
    let n = [facing_dir.x, 0.0, facing_dir.y];
    let base = verts.len() as u32;
    for corner in [center - right - up, center - right + up, center + right + up, center + right - up] {
        verts.push(CapVertex { position: corner.to_array(), normal: n, color });
    }
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// A horizontal filled disc (triangle fan) - the circular drop shadow.
fn push_disc(verts: &mut Vec<CapVertex>, indices: &mut Vec<u32>, cx: f32, y: f32, cz: f32, r: f32, segments: usize, color: [f32; 3]) {
    use std::f32::consts::TAU;
    let n = [0.0, 1.0, 0.0];
    let center = verts.len() as u32;
    verts.push(CapVertex { position: [cx, y, cz], normal: n, color });
    for j in 0..=segments {
        let a = j as f32 / segments as f32 * TAU;
        verts.push(CapVertex { position: [cx + r * a.cos(), y, cz + r * a.sin()], normal: n, color });
    }
    for j in 0..segments as u32 {
        indices.extend_from_slice(&[center, center + 1 + j, center + 2 + j]);
    }
}
