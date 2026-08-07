//! World-side blueprint operations: capture a region of the live world into an
//! authored [`Blueprint`], and decompose a [`ResolvedBlueprint`] into the
//! per-chunk voxel batches the mutation door writes.
//!
//! **Stamping is additive.** A blueprint holds only solid cells - the format has
//! no spelling for empty, because absence *is* how it says empty - so a stamp
//! writes its cells and leaves every other cell in the volume alone. A rock
//! placed on a hillside must not punch a box out of the hill. The consequence
//! worth knowing: capture followed by stamp reproduces the original only where
//! the destination is air. Clearing a volume is a different intent, and gets a
//! different command if something ever needs one.
//!
//! **Capture refuses to truncate.** A region reaching outside the resident set
//! returns an error naming the missing chunk, rather than a blueprint quietly
//! missing part of what the author selected.

use glam::IVec3;
use voxel_core::{
    Blueprint, BlueprintCell, BlueprintShape, DestructionPolicy, LocalPos, MaterialRegistry,
    ResolvedBlueprint, Voxel, Yaw, BLUEPRINT_VERSION,
};
use std::path::PathBuf;

use super::{split_world_voxel, World};

/// The authored blueprint library on disk.
pub fn blueprint_dir() -> PathBuf {
    crate::paths::asset_root().join("assets").join("blueprints")
}

/// Capture the solid voxels in the inclusive world-space box `[min, max]` into a
/// blueprint named `name`.
///
/// Cells are emitted at offsets relative to `min` with the anchor at the origin,
/// so stamping the result at world position `p` puts the captured `min` corner
/// at `p`. Air is skipped - that is what makes the result sparse.
pub fn capture(
    world: &World,
    name: &str,
    min: IVec3,
    max: IVec3,
    registry: &MaterialRegistry,
) -> Result<Blueprint, String> {
    let mut cells = Vec::new();
    for y in min.y..=max.y {
        for z in min.z..=max.z {
            for x in min.x..=max.x {
                let world_voxel = IVec3::new(x, y, z);
                let (chunk_pos, local) = split_world_voxel(world_voxel);
                let chunk = world.get_chunk(chunk_pos).ok_or_else(|| {
                    format!("capture region reaches chunk {chunk_pos}, which is not resident")
                })?;
                let voxel = chunk.data.voxels.voxel(local.to_index());
                let Some(shape) = BlueprintShape::from_shape_id(voxel.shape) else {
                    continue;
                };
                let material = registry
                    .get(voxel.material)
                    .ok_or_else(|| {
                        format!(
                            "voxel at {world_voxel} carries material {:?}, which is not in the registry",
                            voxel.material
                        )
                    })?
                    .id_name
                    .clone();
                cells.push(BlueprintCell {
                    at: (world_voxel - min).to_array(),
                    shape,
                    material,
                });
            }
        }
    }
    Ok(Blueprint {
        version: BLUEPRINT_VERSION,
        name: name.to_string(),
        anchor: [0, 0, 0],
        cells,
        destruction: DestructionPolicy::Destroy,
        protected_volume: false,
    })
}

/// Decompose a stamp into one voxel batch per touched chunk.
///
/// The blueprint's anchor cell lands on `origin`; every other cell keeps its
/// offset from the anchor, rotated by `yaw`. Rotation happens here rather than
/// on the `ResolvedBlueprint` so the stored template stays the authored one and
/// a rotated stamp is a property of the placement, not a second template.
///
/// Grouping by chunk is what lets the handler pay one storage clone per chunk
/// rather than one per cell. The linear scan over `batches` is deliberate: a
/// blueprint touches a handful of chunks, and a map would cost more than it
/// saves while making the output order unstable.
pub fn stamp_batches(
    blueprint: &ResolvedBlueprint,
    origin: IVec3,
    yaw: Yaw,
) -> Vec<(IVec3, Vec<(LocalPos, Voxel)>)> {
    let anchor = IVec3::from_array(blueprint.anchor);
    let mut batches: Vec<(IVec3, Vec<(LocalPos, Voxel)>)> = Vec::new();
    for &(at, voxel) in &blueprint.cells {
        let rel = IVec3::from_array(yaw.apply((IVec3::from_array(at) - anchor).to_array()));
        let world_voxel = origin + rel;
        let (chunk_pos, local) = split_world_voxel(world_voxel);
        match batches.iter_mut().find(|(c, _)| *c == chunk_pos) {
            Some((_, edits)) => edits.push((local, voxel)),
            None => batches.push((chunk_pos, vec![(local, voxel)])),
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::world::chunk::LoadedChunk;
    use crate::world::mutation::{
        EngineMode, MutationCommand, MutationLog, MutationOrigin, WorldMutation,
    };
    use crate::world::world_generator;
    use voxel_core::{ChunkCoord, MaterialId, ShapeId};

    /// Two adjacent air.chunks, so a capture or stamp can cross the seam
    /// between them - the case that would silently lose cells if the
    /// decomposition were wrong.
    fn world_with_two_air_chunks() -> World {
        let generator = world_generator::load_default().expect("default generator loads");
        let mut chunks = HashMap::new();
        for pos in [IVec3::ZERO, IVec3::new(1, 0, 0)] {
            chunks.insert(ChunkCoord::from(pos), LoadedChunk::new_air(pos));
        }
        World {
            chunks,
            generator,
            min_chunk_y: 0,
            max_chunk_y: 4,
            mode: EngineMode::Authoring,
            mutation_log: MutationLog::default(),
            event_outbox: Vec::new(),
        }
    }

    fn resolved(anchor: [i32; 3], cells: Vec<([i32; 3], Voxel)>) -> ResolvedBlueprint {
        ResolvedBlueprint {
            name: "test".to_string(),
            anchor,
            cells,
            destruction: DestructionPolicy::Destroy,
            protected_volume: false,
        }
    }

    #[test]
    fn the_anchor_cell_lands_on_the_stamp_origin() {
        let v = Voxel::cube(MaterialId(2));
        let bp = resolved([1, 0, 1], vec![([1, 0, 1], v), ([0, 0, 0], v)]);
        let batches = stamp_batches(&bp, IVec3::new(5, 6, 7), Yaw::Deg0);

        assert_eq!(batches.len(), 1, "both cells are in one chunk");
        let edits = &batches[0].1;
        assert_eq!(batches[0].0, IVec3::ZERO);
        // The anchor cell sits exactly on the origin.
        assert!(edits.contains(&(LocalPos::new_unchecked(5, 6, 7), v)));
        // The other keeps its offset from the anchor: (0,0,0) - (1,0,1).
        assert!(edits.contains(&(LocalPos::new_unchecked(4, 6, 6), v)));
    }

    #[test]
    fn a_stamp_spanning_a_chunk_boundary_reaches_both_chunks() {
        let v = Voxel::cube(MaterialId(2));
        let bp = resolved([0, 0, 0], vec![([0, 0, 0], v), ([1, 0, 0], v)]);
        // x = 31 is the last column of chunk 0; x = 32 is the first of chunk 1.
        let batches = stamp_batches(&bp, IVec3::new(31, 0, 0), Yaw::Deg0);

        assert_eq!(batches.len(), 2, "the stamp straddles the seam");
        let total: usize = batches.iter().map(|(_, e)| e.len()).sum();
        assert_eq!(total, 2, "no cell is lost or duplicated at the seam");
        assert!(batches.iter().any(|(c, _)| *c == IVec3::ZERO));
        assert!(batches.iter().any(|(c, _)| *c == IVec3::new(1, 0, 0)));
    }

    #[test]
    fn a_rotated_stamp_turns_about_the_anchor_not_the_template_origin() {
        let v = Voxel::cube(MaterialId(2));
        // An L: the anchor plus one cell along +X.
        let bp = resolved([1, 0, 1], vec![([1, 0, 1], v), ([2, 0, 1], v)]);
        let batches = stamp_batches(&bp, IVec3::new(10, 0, 10), Yaw::Deg90);

        let edits = &batches[0].1;
        // The anchor is a fixed point of the rotation, whatever the yaw.
        assert!(edits.contains(&(LocalPos::new_unchecked(10, 0, 10), v)));
        // The +X arm swings onto +Z.
        assert!(
            edits.contains(&(LocalPos::new_unchecked(10, 0, 11), v)),
            "the arm should rotate about the anchor, not the template origin"
        );
    }

    #[test]
    fn capture_then_stamp_reproduces_the_solids_and_is_one_command() {
        let registry = MaterialRegistry::load_initial();
        let mut world = world_with_two_air_chunks();
        let granite = Voxel::cube(registry.resolve("granite").unwrap());
        let soil = Voxel { shape: ShapeId::SlabTop, material: registry.resolve("soil").unwrap(), flags: 0 };

        // Author a two-cell shape straddling the chunk seam.
        for (at, voxel) in [(IVec3::new(31, 2, 3), granite), (IVec3::new(32, 2, 3), soil)] {
            let (chunk, local) = split_world_voxel(at);
            world
                .execute(MutationCommand::authoring(WorldMutation::EditVoxel {
                    chunk,
                    index: local.to_index() as u16,
                    voxel,
                }))
                .expect("authoring edit accepted");
        }

        // Capture a box larger than the shape: the air around it must not appear.
        let bp = capture(&world, "seam_piece", IVec3::new(30, 1, 2), IVec3::new(33, 3, 4), &registry)
            .expect("capture succeeds over resident chunks");
        assert_eq!(bp.cells.len(), 2, "only the solid cells are captured");

        let stamp_index = WorldMutation::StampBlueprint {
            origin: IVec3::ZERO,
            blueprint: resolved([0, 0, 0], Vec::new()),
            yaw: Yaw::Deg0,
        }
        .index();
        let before = world.mutation_log().counts[stamp_index] [MutationOrigin::Authoring.index()];

        // Stamp it into fresh air elsewhere, and read it back.
        let target = IVec3::new(4, 8, 9);
        world
            .execute(MutationCommand::authoring(WorldMutation::StampBlueprint {
                origin: target,
                blueprint: bp.resolve(&registry).expect("resolves"),
                yaw: Yaw::Deg0,
            }))
            .expect("stamp accepted in authoring mode");

        for (offset, expected) in [(IVec3::new(1, 1, 1), granite), (IVec3::new(2, 1, 1), soil)] {
            let (chunk, local) = split_world_voxel(target + offset);
            let got = world.get_chunk(chunk).unwrap().data.voxels.voxel(local.to_index());
            assert_eq!(got, expected, "stamped cell at {offset} matches the captured one");
        }

        let after = world.mutation_log().counts[stamp_index] [MutationOrigin::Authoring.index()];
        assert_eq!(after, before + 1, "a stamp is one command at the door, not one per chunk");
    }

    #[test]
    fn capture_refuses_a_region_that_leaves_the_resident_set() {
        let registry = MaterialRegistry::load_initial();
        let world = world_with_two_air_chunks();
        // Chunk (2,0,0) is not resident. A truncated blueprint would look like a
        // successful capture, which is the failure this error exists to prevent.
        let err = capture(&world, "too_wide", IVec3::new(60, 0, 0), IVec3::new(70, 0, 0), &registry)
            .expect_err("must refuse rather than truncate");
        assert!(err.contains("not resident"), "the error should say why: {err}");
    }
}
