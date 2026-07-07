//! Player-edit override model (design doc §9).
//!
//! [`ChunkOverrides`] is the canonical representation of player edits, in memory
//! and on disk - it replaces the former per-chunk `Vec<VoxelEdit>` edit list.
//! Generated content is reproducible from seed + graph and is never persisted;
//! only a chunk's overrides (and, later, tag changes) and written back, then
//! re-applied on top of freshly generated terrain when the chunk reloads.
//!
//! Phase 3 populates only [`ChunkOverrides::voxel_diffs`]; an edit that clears a
//! cell records `Voxel::EMPTY` there. `voxel_removed` and the scatter/detail/
//! fluid/decal diff fields exist per design §9 but stay empty until their
//! systems land in later phases (and gain on-disk encoding in Substep 7).

use std::collections::{HashMap, HashSet};

use voxel_core::{FaceAxis, LocalPos, Voxel};

use super::layers::{
    DecalEntry, DetailLayerId, DetailTexel, FluidCell, ScatterInstance, ScatterStore,
    StableInstanceId,
};

/// All player edits to a single chunk, layered on top of generated content.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ChunkOverrides {
    /// Explicit voxel changes (a cleared cell = `Voxel::EMPTY`). The only field
    /// populated in Phase 3.
    pub voxel_diffs: HashMap<LocalPos, Voxel>,
    /// Generated voxels explicitly destroyed. Reserved; empty in Phase 3.
    pub voxel_removed: HashSet<LocalPos>,
    /// Generated scatter instances removed by the player. Reserved; empty in Phase 3.
    pub scatter_removed: HashSet<StableInstanceId>,
    /// Player-placed scatter instances. Reserved; empty in Phase 3.
    pub scatter_added: Vec<ScatterInstance>,
    /// Detail-paint overrides keyed by `(layer, cell)`. Reserved; empty in Phase 3.
    pub detail_diffs: HashMap<(DetailLayerId, LocalPos), DetailTexel>,
    /// Fluid-cell overrides. Reserved; empty in Phase 3.
    pub fluid_diffs: HashMap<LocalPos, FluidCell>,
    /// Decal overrides keyed by `(cell, face)`. Reserved; empty in Phase 3.
    pub decal_diffs: HashMap<(LocalPos, FaceAxis), DecalEntry>,
}

impl ChunkOverrides {
    /// True when no override of any kind is present.
    pub fn is_empty(&self) -> bool {
        self.voxel_diffs.is_empty()
            && self.voxel_removed.is_empty()
            && self.scatter_removed.is_empty()
            && self.scatter_added.is_empty()
            && self.detail_diffs.is_empty()
            && self.fluid_diffs.is_empty()
            && self.decal_diffs.is_empty()
    }
    
    /// Count of voxel-level overrides - drives the delta-vs-full persistence
    /// choice. In Phase 3 this is just the `voxel_diffs` count.
    pub fn voxel_override_count(&self) -> usize {
        self.voxel_diffs.len()
    }
    
    /// Record a voxel change at flat chunk index `index` (air clears the cell).
    /// Repeated edits to the same cell are last-write-wins for free.
    pub fn set_voxel(&mut self, index: usize, voxel: Voxel) {
        self.voxel_diffs.insert(LocalPos::from_index(index), voxel);
    }

    /// The effective scatter for a chunk after applying overrides: every
    /// generated instance whose `stable_id` was not removed, followed by the
    /// player-added instances. Shared by rendering and (later) interaction.
    pub fn effective_scatter<'a>(
        &'a self,
        generated: &'a ScatterStore,
    ) -> impl Iterator<Item = &'a ScatterInstance> + 'a {
        generated
            .by_type
            .values()
            .flatten()
            .filter(move |si| !self.scatter_removed.contains(&si.stable_id))
            .chain(self.scatter_added.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layers::{PrefabId, ScatterFlags, ScatterStore, ScatterTypeId};
    
    fn inst(stable: u64) -> ScatterInstance {
        ScatterInstance {
            anchor: LocalPos::from_index(0),
            sub_offset: [0, 0, 0],
            rotation_y: 0,
            scale_variant: 0,
            prefab_id: PrefabId(0),
            flags: ScatterFlags::default(),
            stable_id: StableInstanceId(stable),
        }
    }
    
    #[test]
    fn effective_scatter_filters_removed_and_appends_added() {
        let mut generated = ScatterStore::default();
        generated.by_type.insert(ScatterTypeId(0), vec![inst(10), inst(20), inst(30)]);
        
        let mut ovr = ChunkOverrides::default();
        ovr.scatter_removed.insert(StableInstanceId(20));
        ovr.scatter_added.push(inst(99));
        
        let ids: Vec<u64> = ovr.effective_scatter(&generated).map(|si| si.stable_id.0).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&10) && ids.contains(&30));  // survivors
        assert!(!ids.contains(&20));                         // removed
        assert!(ids.contains(&99));                          // player-added
    }
}
