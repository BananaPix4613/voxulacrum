//! The Tier-1 scenarios the determinism manifest hashes beside the generated
//! base (roadmap §12, M3's determinism gate): a fluid pour stepped 200 times,
//! and a run of voxel edits ending in a blueprint stamp. Both drive the
//! mutation door on a block of chunks generated here, as the engine would.
//!
//! Each scenario checks that it exercised what it exists for and fails if not,
//! for the reason the generated base checks its chunk kinds: a scenario that
//! does nothing hashes identically everywhere and proves nothing.

use std::collections::HashMap;
use std::sync::Arc;

use glam::IVec3;
use rayon::prelude::*;
use voxel_core::{
    ChunkCoord, DestructionPolicy, LocalPos, MaterialId, MaterialRegistry, ResolvedBlueprint,
    ShapeId, Voxel, Yaw,
};

use super::{LayerHash, Record};
use crate::world::chunk::{LoadedChunk, CHUNK_SIZE, DELTA_THRESHOLD};
use crate::world::mutation::{MutationCommand, WorldMutation};
use crate::world::overrides::EditState;
use crate::world::persistence;
use crate::world::world_generator::WorldGenerator;
use crate::world::{blueprint, split_world_voxel, World};

/// Fluid steps the pour runs for.
const FLUID_STEPS: u32 = 200;
/// Steps between hashes of the fluid layers.
const FLUID_CHECKPOINT: u32 = 10;

/// The scenarios' chunks, `[min, max)`: three by three chunk columns about the
/// world origin, over the chunk-Y layers the generated base covers. Both
/// scenarios work at the origin, where four of those columns meet, so their
/// results cross chunk seams.
const BLOCK_MIN: IVec3 = IVec3::new(-1, 0, -1);
const BLOCK_MAX: IVec3 = IVec3::new(2, 4, 2);

/// The four world columns meeting at the origin, one in each chunk column.
const CORNER: [(i32, i32); 4] = [(-1, -1), (-1, 0), (0, -1), (0, 0)];

/// A world holding the scenario block, freshly generated, in `Authoring` mode.
fn scenario_world(generator: &Arc<WorldGenerator>, materials: &Arc<MaterialRegistry>) -> World {
    let mut coords = Vec::new();
    for z in BLOCK_MIN.z..BLOCK_MAX.z {
        for y in BLOCK_MIN.y..BLOCK_MAX.y {
            for x in BLOCK_MIN.x..BLOCK_MAX.x {
                coords.push(IVec3::new(x, y, z));
            }
        }
    }
    let chunks: HashMap<ChunkCoord, LoadedChunk> = coords
        .par_iter()
        .map(|&pos| (ChunkCoord::from(pos), LoadedChunk::from_generated(pos, generator.generate_chunk(pos))))
        .collect();
    World::from_chunks(chunks, generator.clone(), materials.clone(), BLOCK_MIN.y, BLOCK_MAX.y)
}

/// The world's chunks in coordinate order, so records never follow map order.
fn sorted_coords(world: &World) -> Vec<IVec3> {
    let mut coords: Vec<IVec3> = world.chunks.keys().map(|&c| IVec3::from(c)).collect();
    coords.sort_unstable_by_key(|p| (p.x, p.y, p.z));
    coords
}

/// World-Y of the topmost solid voxel in the world column `(x, z)`, or `None`.
fn surface_y(world: &World, x: i32, z: i32) -> Option<i32> {
    let bottom = world.min_chunk_y * CHUNK_SIZE as i32;
    let top = world.max_chunk_y * CHUNK_SIZE as i32 - 1;
    (bottom..=top).rev().find(|&y| {
        let (chunk, local) = split_world_voxel(IVec3::new(x, y, z));
        world
            .get_chunk(chunk)
            .is_some_and(|c| c.data.voxels.voxel(local.to_index()).is_solid())
    })
}

fn chunk_label(pos: IVec3) -> String {
    format!("chunk ({}, {}, {})", pos.x, pos.y, pos.z)
}

/// Pour a water column on each of the four corner columns, then step the fluid
/// `FLUID_STEPS` times. Every chunk's fluid layer is hashed after the pour and
/// every `FLUID_CHECKPOINT` steps.
///
/// The columns sit on the edges of four chunks with active water on both sides
/// of every seam, so the seam transfers run from the first step.
pub(super) fn fluid(
    generator: &Arc<WorldGenerator>,
    materials: &Arc<MaterialRegistry>,
) -> Result<Vec<Record>, String> {
    let mut world = scenario_world(generator, materials);
    let mut poured_into: Vec<IVec3> = Vec::new();
    for (x, z) in CORNER {
        let y = surface_y(&world, x, z)
            .ok_or_else(|| format!("fluid: column ({x}, {z}) has no surface to pour on"))?;
        let outcome = world
            .execute(MutationCommand::authoring(WorldMutation::PourFluidColumn {
                anchor: IVec3::new(x, y, z),
            }))
            .map_err(|e| format!("fluid: the pour was refused: {e:?}"))?;
        poured_into.extend(outcome.water_rebuild);
    }

    let mut records = fluid_records(&world, 0);
    let after_pour: Vec<String> = records.iter().map(|r| r.layers[0].hash.clone()).collect();
    let mut stepped = 0u32;
    for step in 1..=FLUID_STEPS {
        if world.step_fluids().is_some() {
            stepped += 1;
        }
        if step % FLUID_CHECKPOINT == 0 {
            records.extend(fluid_records(&world, step));
        }
    }

    // Water that reached a chunk the pour did not touch crossed a seam by
    // flowing, which is the transfer most worth comparing.
    let last = &records[records.len() - after_pour.len()..];
    let reached = sorted_coords(&world)
        .into_iter()
        .zip(last.iter().zip(&after_pour))
        .filter(|(pos, (r, poured))| r.layers[0].hash != **poured && !poured_into.contains(pos))
        .count();
    if stepped < FLUID_CHECKPOINT || reached == 0 {
        return Err(format!(
            "fluid: the water stepped {stepped} of {FLUID_STEPS} times and flowed into {reached} \
             chunk(s) the pour did not touch; it must still be moving at the first checkpoint \
             and cross a seam. Check that the corner columns are dry land with room around them."
        ));
    }
    println!("  fluid: stepped {stepped} of {FLUID_STEPS} times, flowed into {reached} more chunk(s)");
    Ok(records)
}

/// Every chunk's fluid layer at `step`.
fn fluid_records(world: &World, step: u32) -> Vec<Record> {
    sorted_coords(world)
        .into_iter()
        .map(|pos| {
            let chunk = world.get_chunk(pos).expect("scenario chunks stay resident");
            Record {
                at: format!("step {step}, {}", chunk_label(pos)),
                kinds: Vec::new(),
                layers: vec![LayerHash::of("fluid", |b| {
                    persistence::write_fluid_layer(b, &chunk.data.fluids)
                })],
            }
        })
        .collect()
}

/// Four batches of edits through the door, every chunk's voxels and saved
/// record hashed after each: single edits, a wall built across a seam, a batch
/// large enough to promote a chunk to a full save, and a rotated blueprint stamp
/// across the corner.
pub(crate) fn edits(
    generator: &Arc<WorldGenerator>,
    materials: &Arc<MaterialRegistry>,
) -> Result<Vec<Record>, String> {
    let [a, b] = [MaterialId(1), MaterialId(2)];
    for id in [a, b] {
        if materials.get(id).is_none() {
            return Err(format!("edits: the registry has no material {id:?} to build with"));
        }
    }
    let cube = |m| Voxel::cube(m);
    let slab = Voxel { material: b, shape: ShapeId::SlabBottom, flags: 0 };

    let mut world = scenario_world(generator, materials);
    let ground = |world: &World, x: i32, z: i32| {
        surface_y(world, x, z).ok_or_else(|| format!("edits: column ({x}, {z}) has no surface"))
    };
    let mut records = Vec::new();

    // 1. Single edits: dig the corner column's top two voxels.
    let y = ground(&world, 0, 0)?;
    for dy in 0..2 {
        let (chunk, local) = split_world_voxel(IVec3::new(0, y - dy, 0));
        execute(&mut world, WorldMutation::EditVoxel {
            chunk,
            index: local.to_index() as u16,
            voxel: Voxel::EMPTY,
        })?;
    }
    records.extend(edit_records(&world, "batch 1 (single edits)"));

    // 2. A wall across the x seam, cubes with a slab course on top.
    let mut wall = Vec::new();
    for x in -3..3 {
        let y = ground(&world, x, 3)?;
        for dy in 1..=3 {
            wall.push((IVec3::new(x, y + dy, 3), cube(a)));
        }
        wall.push((IVec3::new(x, y + 4, 3), slab));
    }
    for (chunk, edits) in group_by_chunk(&wall) {
        execute(&mut world, WorldMutation::EditVoxelBatch { chunk, edits })?;
    }
    records.extend(edit_records(&world, "batch 2 (wall across a seam)"));

    // 3. One batch past the delta threshold, which promotes its chunk.
    let (promoted, _) = split_world_voxel(IVec3::new(8, ground(&world, 8, 8)?, 8));
    let edits: Vec<(LocalPos, Voxel)> = (0..DELTA_THRESHOLD)
        .map(|i| (LocalPos::from_index(i), cube(if i % 3 == 0 { b } else { a })))
        .collect();
    execute(&mut world, WorldMutation::EditVoxelBatch { chunk: promoted, edits })?;
    let state = &world.get_chunk(promoted).expect("scenario chunks stay resident").data.edit_state;
    if !matches!(state, EditState::Promoted(_)) {
        return Err(format!("edits: chunk {promoted} was not promoted by {DELTA_THRESHOLD} edits"));
    }
    records.extend(edit_records(&world, "batch 3 (promotion)"));

    // 4. A rotated blueprint stamped across the corner.
    let arch = arch(cube(a), cube(b), slab);
    let origin = IVec3::new(0, ground(&world, 0, 0)? + 1, 0);
    let touched = blueprint::stamp_batches(&arch, origin, Yaw::Deg90).len();
    if touched < 2 {
        return Err(format!("edits: the stamp touched {touched} chunk(s); it must cross a seam"));
    }
    execute(&mut world, WorldMutation::StampBlueprint { origin, blueprint: arch, yaw: Yaw::Deg90 })?;
    records.extend(edit_records(&world, "batch 4 (blueprint stamp)"));

    println!("  edits: 4 batches, stamp across {touched} chunks");
    Ok(records)
}

fn execute(world: &mut World, mutation: WorldMutation) -> Result<(), String> {
    world
        .execute(MutationCommand::authoring(mutation))
        .map(|_| ())
        .map_err(|e| format!("edits: an edit was refused: {e:?}"))
}

/// World-space cells grouped into one batch per chunk, in first-seen order.
fn group_by_chunk(cells: &[(IVec3, Voxel)]) -> Vec<(IVec3, Vec<(LocalPos, Voxel)>)> {
    let mut batches: Vec<(IVec3, Vec<(LocalPos, Voxel)>)> = Vec::new();
    for &(at, voxel) in cells {
        let (chunk, local) = split_world_voxel(at);
        match batches.iter_mut().find(|(c, _)| *c == chunk) {
            Some((_, edits)) => edits.push((local, voxel)),
            None => batches.push((chunk, vec![(local, voxel)])),
        }
    }
    batches
}

/// An asymmetric arch: two pillars, a lintel with a spur, and a slab on top, so
/// a wrong rotation or anchor lands visibly elsewhere.
fn arch(pillar: Voxel, lintel: Voxel, cap: Voxel) -> ResolvedBlueprint {
    let mut cells = Vec::new();
    for y in 0..3 {
        cells.push(([0, y, 0], pillar));
        cells.push(([3, y, 0], pillar));
    }
    for x in 0..4 {
        cells.push(([x, 3, 0], lintel));
    }
    cells.push(([0, 3, 1], lintel));
    cells.push(([1, 4, 0], cap));
    ResolvedBlueprint {
        name: "determinism_arch".to_string(),
        anchor: [1, 0, 0],
        cells,
        destruction: DestructionPolicy::Destroy,
        protected_volume: false,
    }
}

/// Every chunk's voxels and the record a save would write for it, after `batch`.
fn edit_records(world: &World, batch: &str) -> Vec<Record> {
    sorted_coords(world)
        .into_iter()
        .map(|pos| {
            let chunk = world.get_chunk(pos).expect("scenario chunks stay resident");
            Record {
                at: format!("{batch}, {}", chunk_label(pos)),
                kinds: Vec::new(),
                layers: vec![
                    LayerHash::of("voxels", |b| persistence::write_voxels(b, &chunk.data.voxels)),
                    LayerHash::of("record", |b| {
                        if let Some(record) = persistence::build_chunk_record(chunk) {
                            b.extend(
                                persistence::serialize_chunk_record_raw(&record)
                                    .expect("a chunkn record serializes"),
                            );
                        }
                    }),
                ],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_group_into_one_batch_per_chunk_across_a_seam() {
        let v = Voxel::cube(MaterialId(1));
        let cells = [(IVec3::new(-1, 5, 3), v), (IVec3::new(0, 5, 3), v), (IVec3::new(-2, 5, 3), v)];
        let batches = group_by_chunk(&cells);
        assert_eq!(batches.len(), 2, "the x = 0 seam splits the cells into two chunks");
        assert_eq!(batches[0].0, IVec3::new(-1, 0, 0), "first-seen chunk first");
        assert_eq!(batches[0].1.len(), 2);
        assert_eq!(batches[1].0, IVec3::ZERO);
        assert_eq!(batches[1].1, vec![(LocalPos::new_unchecked(0, 5, 3), v)]);
    }
}
