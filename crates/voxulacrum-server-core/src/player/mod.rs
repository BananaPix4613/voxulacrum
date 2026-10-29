//! The player: the pure step ([`sim`]) and the world's implementation of the
//! [`WorldView`] seam it reads through.

pub mod sim;

use glam::IVec3;
use voxel_core::ShapeId;

use crate::world::chunk::CHUNK_SIZE;
use crate::world::World;
use sim::WorldView;

/// Read-only world access for the pure sim: a voxel's solid vertical interval,
/// honoring slabs, by world coordinate.
impl WorldView for World {
    fn solid_interval(&self, voxel: IVec3) -> Option<(f32, f32)> {
        let dim = CHUNK_SIZE as i32;
        let chunk = IVec3::new(
            voxel.x.div_euclid(dim),
            voxel.y.div_euclid(dim),
            voxel.z.div_euclid(dim),
        );
        let v = self.get_chunk(chunk)?.voxel(
            voxel.x.rem_euclid(dim) as usize,
            voxel.y.rem_euclid(dim) as usize,
            voxel.z.rem_euclid(dim) as usize,
        );
        match v.shape {
            ShapeId::Cube => Some((0.0, 1.0)),
            ShapeId::SlabBottom => Some((0.0, 0.5)),
            ShapeId::SlabTop => Some((0.5, 1.0)),
            ShapeId::Empty => None,
        }
    }
}
