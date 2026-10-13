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

use super::chunk::DELTA_THRESHOLD;
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
    #[allow(dead_code)] // voxel-edit toolkit; wired when editing lands
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

/// How a loaded chunk's player edits are tracked, and therefore how it saves.
///
/// An explicit state rather than `Option<ChunkOverrides>` because `None` had to
/// mean three different things: never edited, promoted past [`DELTA_THRESHOLD`],
/// and loaded from a Full record. Collapsing them let the next edit start a
/// nearly empty bucket that replaced the Full record on disk.
#[derive(Clone, PartialEq, Debug)]
pub enum EditState {
    /// Exactly what generation produces. Nothing to persist.
    Untouched,
    /// Sparse edits over generated terrain. Saves as a delta record.
    Delta(ChunkOverrides),
    /// Edited past the delta threshold, or loaded from a Full record. Saves the
    /// whole storage and keeps doing so whatever touches the chunk afterwards.
    /// The storage is the canonical voxel state, so the bucket holds no
    /// `voxel_diffs`; it carries only the layers a Full record has no slot for
    /// (scatter, fluid), which live for the session but are not persisted.
    Promoted(ChunkOverrides),
}

impl EditState {
    /// True when no edit of any kind has touched the chunk.
    pub fn is_untouched(&self) -> bool {
        matches!(self, EditState::Untouched)
    }

    /// The bucket, if this state carries one. `None` only for `Untouched`.
    pub fn bucket(&self) -> Option<&ChunkOverrides> {
        match self {
            EditState::Untouched => None,
            EditState::Delta(ovr) | EditState::Promoted(ovr) => Some(ovr),
        }
    }

    /// The bucket to record a non-voxel edit into. An untouched chunk becomes
    /// `Delta` here; a promoted one stays promoted.
    pub fn bucket_mut(&mut self) -> &mut ChunkOverrides {
        if self.is_untouched() {
            *self = EditState::Delta(ChunkOverrides::default());
        }
        match self {
            EditState::Delta(ovr) | EditState::Promoted(ovr) => ovr,
            EditState::Untouched => unreachable!("an untouched chunk was just made Delta"),
        }
    }

    /// Record a batch of voxel edits (flat chunk index, new voxel). Promotion is
    /// decided once, after the whole batch. A promoted chunk records nothing:
    /// its storage is already the canonical voxel state.
    pub fn record_voxels(&mut self, edits: impl IntoIterator<Item = (usize, Voxel)>) {
        if matches!(self, EditState::Promoted(_)) {
            return;
        }
        let ovr = self.bucket_mut();
        for (index, voxel) in edits {
            ovr.set_voxel(index, voxel);
        }
        if ovr.voxel_override_count() >= DELTA_THRESHOLD {
            self.promote();
        }
    }

    fn promote(&mut self) {
        let mut ovr = match std::mem::replace(self, EditState::Untouched) {
            EditState::Untouched => ChunkOverrides::default(),
            EditState::Delta(ovr) | EditState::Promoted(ovr) => ovr,
        };
        // The storage now holds every voxel, so the diffs are dead weight.
        // Replaced rather than cleared so the allocation is released.
        ovr.voxel_diffs = HashMap::new();
        *self = EditState::Promoted(ovr);
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

    fn cube(material: u16) -> Voxel {
        Voxel::cube(voxel_core::MaterialId(material))
    }

    #[test]
    fn bucket_mut_makes_untouched_delta_and_leaves_promoted_alone() {
        let mut state = EditState::Untouched;
        assert!(state.bucket().is_none());
        state.bucket_mut();
        assert!(matches!(state, EditState::Delta(_)));

        let mut promoted = EditState::Promoted(ChunkOverrides::default());
        promoted.bucket_mut();
        assert!(matches!(promoted, EditState::Promoted(_)));
    }

    #[test]
    fn reaching_the_threshold_promotes_and_releases_the_diffs() {
        let mut state = EditState::Untouched;
        state.record_voxels((0..DELTA_THRESHOLD - 1).map(|i| (i, cube(1))));
        assert!(matches!(state, EditState::Delta(_)), "one short of the threshold stays Delta");

        state.record_voxels([(DELTA_THRESHOLD, cube(1))]);
        match &state {
            EditState::Promoted(ovr) => assert!(ovr.voxel_diffs.is_empty()),
            _ => panic!("expected Promoted at the threshold"),
        }
    }

    #[test]
    fn a_promoted_chunk_stays_promoted_and_records_no_voxel_diffs() {
        let mut state = EditState::Promoted(ChunkOverrides::default());
        state.record_voxels([(5, cube(2))]);
        match &state {
            EditState::Promoted(ovr) => assert!(ovr.voxel_diffs.is_empty()),
            _ => panic!("a later edit must not undo promotion"),
        }
    }

    #[test]
    fn promotion_keeps_the_layers_a_full_record_cannot_hold() {
        let mut ovr = ChunkOverrides::default();
        ovr.scatter_removed.insert(StableInstanceId(7));
        let mut state = EditState::Delta(ovr);
        state.record_voxels((0..DELTA_THRESHOLD).map(|i| (i, cube(1))));
        match &state {
            EditState::Promoted(ovr) => {
                assert!(ovr.scatter_removed.contains(&StableInstanceId(7)));
            }
            _ => panic!("expected Promoted"),
        }
    }
}
