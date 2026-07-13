//! The mutation command API (design doc §9): the single seam every world-state
//! mutation flows through. A [`MutationCommand`] declares its [`MutationOrigin`]
//! (dev-authoring vs play-time) and its intent ([`WorldMutation`]);
//! [`World::execute`](super::World::execute) validates the origin against the
//! engine's current [`EngineMode`], then dispatches the intent to its handler.
//! Each handler owns its own persistence marking, mesh-dirty flagging,
//! override-bucket creation, and invalidation, so call sites don't sprinkle those
//! across the codebase. No world mutation bypasses this seam.
//!
//! Phase 8 ships only [`EngineMode::Authoring`]; `Play` lands in Phase 9. The
//! mode-rejection contract is real and unit-tested against synthesized
//! `PlayTime` commands. Intents are added as their mutation sites migrate;
//! Substep 6 adds the batched voxel intent (`EditVoxelBatch`).

use std::collections::HashMap;
use std::sync::Arc;

use glam::IVec3;
use smallvec::SmallVec;
use voxel_core::{ChunkCoord, LocalPos, Voxel};

use super::chunk::LoadedChunk;
use super::world_generator::WorldGenerator;

/// The mode a mutation was issued from (design §9). `Authoring` = the developer
/// editing graphs in the integrated editor; `PlayTime` = a player editing during
/// play. The two never coexist; a command whose origin does not match the current
/// [`EngineMode`] is rejected rather than silently entering coexistence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationOrigin {
    Authoring,
    PlayTime,
}

/// The engine's current world-mutation mode (design §9). Phase 8 ships only
/// `Authoring`; `Play` lands in Phase 9. Transitions are explicit and rare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineMode {
    Authoring,
    /// Play mode lands in Phase 9 (player edits persist; graphs are read-only). No
    /// Phase-8 path constructs it; it is matched by `required_origin` and exercised
    /// by the mode-rejection tests.
    #[allow(dead_code)]
    Play,
}

impl EngineMode {
    /// The single origin this mode accepts. A command from any other origin is
    /// rejected at the API boundary ([`MutationError::WrongMode`]) - this is how
    /// §9's "authoring and play do not coexist" becomes a runtime property.
    pub fn required_origin(self) -> MutationOrigin {
        match self {
            EngineMode::Authoring => MutationOrigin::Authoring,
            EngineMode::Play => MutationOrigin::PlayTime,
        }
    }
}

/// Why a command was rejected without being applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationError {
    /// The command's origin did not match the engine's current mode.
    WrongMode { mode: EngineMode, origin: MutationOrigin },
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutationError::WrongMode { mode, origin } => {
                write!(f, "mutation origin {origin:?} rejected in {mode:?} mode")
            }
        }
    }
}
impl std::error::Error for MutationError {}

/// Side-effects a successful mutation asks the caller to finish. World-state
/// changes (storage, overrides, persist/mesh dirty flags) are applied inside the
/// handler; this reports the GPU-buffer rebuilds that live *outside* `World` so
/// the calling system can service them (scatter/water passes, mesher).
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    /// Chunks whose scatter instance buffer must be rebuilt.
    pub scatter_rebuild: SmallVec<[IVec3; 4]>,
    /// Chunks whose water mesh must be rebuilt.
    pub water_rebuild: SmallVec<[IVec3; 4]>,
    /// Chunks newly mesh-invalidated (the caller submits them to the mesher).
    pub mesh_invalidated: SmallVec<[IVec3; 4]>,
}

impl MutationOutcome {
    /// Fold another outcome into this one (used when a command touches several
    /// chunks or delegates to sub-handlers).
    #[allow(dead_code)] // used as multi-chunk handlers land (Substeps 1b/1c/6)
    pub fn extend(&mut self, other: MutationOutcome) {
        self.scatter_rebuild.extend(other.scatter_rebuild);
        self.water_rebuild.extend(other.water_rebuild);
        self.mesh_invalidated.extend(other.mesh_invalidated);
    }
}

/// The intent of a mutation: what it changes. Every world-state mutation is one
/// of these; handlers live on `World` and [`World::execute`](super::World::execute)
/// dispatches to them. The set grows as mutation sites migrate; Substep 6 adds
/// `EditVoxelBatch`.
pub enum WorldMutation {
    /// Set a single voxel at flat `index` within the chunk at `chunk` (chunk-space
    /// coordinate). Records the change in the override bucket and marks the chunk
    /// and its border neighbors mesh-dirty.
    EditVoxel { chunk: IVec3, index: u16, voxel: Voxel },
    /// Apply a batch of voxel edits to one chunk with a single storage clone
    /// (drift-review 3.1: the single-edit path clones once per voxel). The primary
    /// voxel-edit intent for Phase 9's brush tools; `EditVoxel` delegates here as a
    /// batch of one.
    #[allow(dead_code)] // Phase 9 brush tools consume this; exercised by tests
    EditVoxelBatch { chunk: IVec3, edits: Vec<(LocalPos, Voxel)> },
    /// Remove every effective scatter instance anchored at `anchor` in `chunk`:
    /// generated ones are tombstoned (`scatter_removed`), player-added ones dropped.
    RemoveScatter { chunk: IVec3, anchor: LocalPos },
    /// Place one player scatter instance at `anchor` in `chunk`. `world_voxel` is
    /// the anchor's world-space voxel coordinate, used to derive the stable id.
    PlaceScatter { chunk: IVec3, anchor: LocalPos, world_voxel: IVec3 },
    /// Pour a short settled water column above `anchor` (world-space voxel),
    /// spanning whatever chunks the column crosses. Each poured cell is written as
    /// a player fluid override and activated so the next sim tick flows it.
    PourFluidColumn { anchor: IVec3 },
    /// Mark chunks whose fluid field the simulation changed this tick as
    /// persist-dirty, ensuring each carries an override bucket so its fluid is
    /// snapshotted (delta save path). The sim mutates cells directly (physics, not
    /// a discrete command); this centralizes the resulting persistence bookkeeping.
    MarkFluidDirty { chunks: Vec<IVec3> },
    /// Insert a freshly generated/loaded chunk into the resident set and mark its
    /// six face neighbors mesh-dirty so their boundary faces re-cull against it.
    /// Issued by streaming when a generation task completes.
    InsertLoadedChunk { chunk: LoadedChunk },
    /// Swap a completed background regeneration into the world: merge the
    /// regenerated chunks over the resident set, adopt the new generator, and mark
    /// every resident chunk mesh-dirty. Authoring-mode authoritative regen (§9) -
    /// the regenerated chunks carry no overrides, so player-shaped state is
    /// intentionally replaced (not a bug; the design's mode model).
    SwapRegeneratedChunks {
        chunks: HashMap<ChunkCoord, LoadedChunk>,
        generator: Arc<WorldGenerator>,
    },
}

/// A world mutation plus the mode it was issued from. Constructed at each call
/// site and handed to [`World::execute`](super::World::execute).
pub struct MutationCommand {
    pub origin: MutationOrigin,
    pub mutation: WorldMutation,
}

impl MutationCommand {
    /// A command issued from authoring (editor / dev tools). In Phase 8 every
    /// live mutation site uses this; the player-edit sites switch to
    /// [`MutationCommand::play`] when Play mode lands (Phase 9).
    pub fn authoring(mutation: WorldMutation) -> Self {
        Self { origin: MutationOrigin::Authoring, mutation }
    }

    /// A command issued from play-time (player edits). No live site uses this in
    /// Phase 8; it exists so the mode-rejection contract can be exercised.
    #[allow(dead_code)] // no Play-mode site yet; used by mode-rejection tests
    pub fn play(mutation: WorldMutation) -> Self {
        Self { origin: MutationOrigin::PlayTime, mutation }
    }
}
