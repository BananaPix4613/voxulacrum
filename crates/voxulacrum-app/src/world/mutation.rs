//! The mutation command API (design doc §9): the single seam every world-state
//! mutation flows through. A [`MutationCommand`] declares its [`MutationOrigin`]
//! (dev-authoring vs play-time) and its intent ([`WorldMutation`]);
//! [`World::execute`](super::World::execute) validates the origin against the
//! engine's current [`EngineMode`], then dispatches the intent to its handler.
//! Each handler owns its own persistence marking, override-bucket creation and
//! invalidation, and reports the chunks it changed to the change feed, so call
//! sites don't sprinkle those across the codebase. No world mutation bypasses
//! this seam.
//!
//! Phase 8 ships only [`EngineMode::Authoring`]; `Play` lands in Phase 9. The
//! mode-rejection contract is real and unit-tested against synthesized
//! `PlayTime` commands. Intents are added as their mutation sites migrate;
//! Substep 6 adds the batched voxel intent (`EditVoxelBatch`).

use std::collections::HashMap;
use std::sync::Arc;
use glam::IVec3;
use smallvec::SmallVec;
use voxel_core::{ChunkCoord, LocalPos, ResolvedBlueprint, Voxel, Yaw};

use super::fluid_sim::ChunkPlan;
use super::layers::PrefabId;
use super::changes::{ChunkFaces, ChunkLayers};
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
    System,
}

impl MutationOrigin {
    pub const COUNT: usize = 3;

    pub fn index(self) -> usize {
        match self {
            MutationOrigin::Authoring => 0,
            MutationOrigin::PlayTime => 1,
            MutationOrigin::System => 2,
        }
    }

    /// Short label for the mutation log's columns.
    pub fn label(self) -> &'static str {
        match self {
            MutationOrigin::Authoring => "auth",
            MutationOrigin::PlayTime => "play",
            MutationOrigin::System => "sys",
        }
    }
}

/// The engine's current world-mutation mode (design §9). Phase 8 ships only
/// `Authoring`; `Play` lands in Phase 9. Transitions are explicit and rare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineMode {
    Authoring,
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
    /// A `System`-origin command carried an intent that is not engine
    /// infrastructure. `System` skips the mode gate, so what it may carry is a
    /// whitelist rather than a convention - see
    /// [`WorldMutation::allows_system_origin`].
    SystemOriginNotAllowed { intent: &'static str },
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MutationError::WrongMode { mode, origin } => {
                write!(f, "mutation origin {origin:?} rejected in {mode:?} mode")
            }
            MutationError::SystemOriginNotAllowed { intent } => {
                write!(f, "intent {intent} may not be issued with System origin")
            }
        }
    }
}
impl std::error::Error for MutationError {}

/// What a successful mutation changed. The handler applies every world-state
/// change itself; this names the chunks whose data it changed, which
/// `World::execute` publishes to the change feed. It names data, never render
/// work: what to rebuild is the client's decision.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    /// Chunks whose data changed, with the layers and boundary faces changed,
    /// one entry per chunk. `World::execute` stamps each with a version.
    pub changed: SmallVec<[(IVec3, ChunkLayers, ChunkFaces); 4]>,
}

impl MutationOutcome {
    /// Fold another outcome into this one (used when a command touches several
    /// chunks or delegates to sub-handlers).
    pub fn extend(&mut self, other: MutationOutcome) {
        for (chunk, layers, faces) in other.changed {
            self.touch(chunk, layers, faces);
        }
    }
    
    /// Record that the command changed `layers` and `faces` of `chunk`, merged
    /// into an earlier record of the same chunk.
    pub fn touch(&mut self, chunk: IVec3, layers: ChunkLayers, faces: ChunkFaces) {
        match self.changed.iter_mut().find(|(c, _, _)| *c == chunk) {
            Some((_, l, f)) => {
                *l |= layers;
                *f |= faces;
            }
            None => self.changed.push((chunk, layers, faces)),
        }
    }
}

/// The intent of a mutation: what it changes. Every world-state mutation is one
/// of these; handlers live on `World` and [`World::execute`](super::World::execute)
/// dispatches to them. The set grows as mutation sites migrate; Substep 6 adds
/// `EditVoxelBatch`.
pub enum WorldMutation {
    /// Set a single voxel at flat `index` within the chunk at `chunk` (chunk-space
    /// coordinate). Records the change in the override bucket and reports the
    /// chunk's voxels changed, with the faces the cell lies on.
    EditVoxel { chunk: IVec3, index: u16, voxel: Voxel },
    /// Apply a batch of voxel edits to one chunk with a single storage clone
    /// (drift-review 3.1: the single-edit path clones once per voxel). The primary
    /// voxel-edit intent for Phase 9's brush tools; `EditVoxel` delegates here as a
    /// batch of one.
    #[allow(dead_code)] // Phase 9 brush tools consume this; exercised by tests
    EditVoxelBatch { chunk: IVec3, edits: Vec<(LocalPos, Voxel)> },
    /// Remove every effective scatter instance anchored at `anchor` in `chunk`:
    /// generated ones are tombstoned (`scatter_removed`), player-added ones dropped.
    #[allow(dead_code)]
    RemoveScatter { chunk: IVec3, anchor: LocalPos },
    /// Place one player scatter instance of `prefab` at `anchor` in `chunk`.
    /// `world_voxel` is the anchor's world-space voxel coordinate, used to derive
    /// the stable id. The prefab is the caller's choice; the engine picks none.
    #[allow(dead_code)]
    PlaceScatter { chunk: IVec3, anchor: LocalPos, world_voxel: IVec3, prefab: PrefabId },
    /// Pour a short settled water column above `anchor` (world-space voxel),
    /// spanning whatever chunks the column crosses. Each poured cell is written as
    /// a player fluid override and activated so the next sim tick flows it.
    PourFluidColumn { anchor: IVec3 },
    /// Commit one fluid-simulation tick's planned cell transfers.
    ///
    /// The sim's plan phase is read-only and needs cross-chunk neighbor access,
    /// so it runs at the call site; this is the write half - apply each chunk's
    /// plan, make the mirrored edge cell on the far side of every changed
    /// boundary cell so flow keeps crossing seams, and mark each changed chunk
    /// persist-dirty with an override bucket so the delta save path snapshots
    /// its field.
    ///
    /// A whole tick commits as one command rather than one command per cell:
    /// fluid is continuous physics over the resident set, not a discrete
    /// authored edit, and a tick is the granularity a server would replicate at.
    /// This supersedes the former `MarkFluidDirty` - persistence marking is now
    /// part of committing rather than a separate call the caller had to
    /// remember, which is what let drift 1.2 hide for two phases.
    CommitFluidPlans { plans: Vec<(ChunkCoord, ChunkPlan)> },
    /// Insert a freshly generated/loaded chunk into the resident set and report
    /// every layer and face changed, so its six face neighbors re-cull against it.
    /// Issued by streaming when a generation task completes.
    InsertLoadedChunk { chunk: LoadedChunk },
    /// Drop a chunk from the resident set (streaming eviction). The caller
    /// persists it first if dirty, since persistence lives outside `World`;
    /// this is the residency change itself. The door owns it so insert and
    /// evict are symmetric - per-observer streaming (0.6.0) and the streaming
    /// visualizer both need to observe residency, and neither can if half of it
    /// happens off the seam.
    EvictChunk { chunk: IVec3 },
    /// Apply the cross-chunk slab demotions `world::seam` derives once a
    /// chunk's six face neighbors are resident: rewrite the demoted voxels,
    /// fill any the ocean now reaches, drop foliage the new water submerges,
    /// record the decision in the override bucket so a reload neither
    /// re-derives nor reverts it, and mark the chunk seam-finalized.
    ///
    /// `demotions` is derived read-only by the caller because it needs neighbor
    /// storage this handler does not have - the same split `EditVoxelBatch`
    /// uses: compute outside, write inside.
    FinalizeSeam { chunk: IVec3, demotions: Vec<(usize, Voxel)> },
    /// Swap a completed background regeneration into the world: merge the
    /// regenerated chunks over the resident set, adopt the new generator, and
    /// report every swapped chunk changed. Authoring-mode authoritative regen (§9) -
    /// the regenerated chunks carry no overrides, so player-shaped state is
    /// intentionally replaced (not a bug; the design's mode model).
    SwapRegeneratedChunks {
        chunks: HashMap<ChunkCoord, LoadedChunk>,
        generator: Arc<WorldGenerator>,
    },
    /// Stamp a resolved blueprint into the world, its anchor cell landing on
    /// `origin`. Decomposes into one voxel batch per touched chunk internally.
    /// 
    /// One intent rather than N `EditVoxelBatch` commands because the door's
    /// value is that *intent* is legible at the seam - the same reason
    /// `FinalizeSeam` and `CommitFluidPlans` exist despite both being
    /// expressible as batches. A stamp is one authored act: one log row, one
    /// event, one thing undo must treat atomically, one message to replicate.
    /// 
    /// Additive: a blueprint carries only solid cells, so this writes them and
    /// leaves the rest of the volume alone. It never clears.
    StampBlueprint { origin: IVec3, blueprint: ResolvedBlueprint, yaw: Yaw },
}

impl WorldMutation {
    /// Dense index for the mutation log's per-intent counters. Kept adjacent to
    /// [`INTENT_NAMES`] so adding a variant is a compile error in both places.
    pub fn index(&self) -> usize {
        match self {
            WorldMutation::EditVoxel { .. } => 0,
            WorldMutation::EditVoxelBatch { .. } => 1,
            WorldMutation::RemoveScatter { .. } => 2,
            WorldMutation::PlaceScatter { .. } => 3,
            WorldMutation::PourFluidColumn { .. } => 4,
            WorldMutation::CommitFluidPlans { .. } => 5,
            WorldMutation::InsertLoadedChunk { .. } => 6,
            WorldMutation::EvictChunk { .. } => 7,
            WorldMutation::FinalizeSeam { .. } => 8,
            WorldMutation::SwapRegeneratedChunks { .. } => 9,
            WorldMutation::StampBlueprint { .. } => 10,
        }
    }

    /// Stable name for diagnostics, rejection messages, and log rows. Derived
    /// from the same table the log labels its rows with, so the two cannot drift.
    pub fn name(&self) -> &'static str {
        INTENT_NAMES[self.index()]
    }

    /// Whether this intent may be issued with [`MutationOrigin::System`].
    ///
    /// `System` is exempt from the authoring/play coexistence gate, so without a
    /// whitelist it is a universal bypass: any call site could declare `System`
    /// and write anything in any mode, and the gate P5 exists to enforce would
    /// protect nothing. The exemption is for engine infrastructure - streaming
    /// residency, generation finalization, simulation bookkeeping - and never
    /// for an actor edit. `EditVoxel` from `System` is a bug by construction,
    /// and this is what makes it one at runtime rather than by convention.
    pub fn allows_system_origin(&self) -> bool {
        matches!(
            self,
            WorldMutation::CommitFluidPlans { .. }
                | WorldMutation::InsertLoadedChunk { .. }
                | WorldMutation::EvictChunk { .. }
                | WorldMutation::FinalizeSeam { .. }
        )
    }
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

    /// A command issued by engine infrastructure (streaming, sim bookkeeping),
    /// accepted in any mode. It is not an actor edit and does not participate in
    /// the authoring/play coexistence gate.
    pub fn system(mutation: WorldMutation) -> Self {
        Self { origin: MutationOrigin::System, mutation }
    }
}

/// Number of [`WorldMutation`] variants.
pub const INTENT_COUNT: usize = 11;

/// Intent names, indexed by [`WorldMutation::index`].
pub const INTENT_NAMES: [&str; INTENT_COUNT] = [
    "EditVoxel",
    "EditVoxelBatch",
    "RemoveScatter",
    "PlaceScatter",
    "PourFluidColumn",
    "CommitFluidPlans",
    "InsertLoadedChunk",
    "EvictChunk",
    "FinalizeSeam",
    "SwapRegeneratedChunks",
    "StampBlueprint",
];

/// How many recent actor commands the log keeps.
pub const RECENT_LEN: usize = 12;

/// One actor command as it went through the door.
#[derive(Clone, Copy, Debug)]
pub struct MutationRecord {
    pub intent: &'static str,
    pub origin: MutationOrigin,
    /// The engine mode at the time - the other half of the coexistence gate.
    pub mode: EngineMode,
    /// Chunks whose data the command changed.
    pub changed: u16,
}

/// The mutation and event log (roadmap §4.4).
///
/// Deliberately two-tiered. `counts` covers **every** command, because that is
/// what keeps the mutation-door gate honest: an unexpected `(intent, origin)`
/// pair is visible, and `rejected_*` should stay at zero - a nonzero value means
/// a call site is constructing a command the gate refuses, which would otherwise
/// be an `Err` somebody discarded.
///
/// `recent` holds only **non-`System`** commands. Streaming, the fluid sim and
/// seam finalization issue hundreds of `System` commands per second and would
/// drown the handful of actor edits a human is actually trying to debug.
///
/// `Copy`, with fixed arrays rather than a `VecDeque`, so mirroring it into the
/// UI each frame costs no allocation.
#[derive(Clone, Copy, Debug, Default)]
pub struct MutationLog {
    /// `[intent][origin]` counts since launch.
    pub counts: [[u64; MutationOrigin::COUNT]; INTENT_COUNT],
    /// Commands refused because origin and mode would have coexisted.
    pub rejected_wrong_mode: u64,
    /// `System`-origin commands carrying a non-infrastructure intent.
    pub rejected_system_origin: u64,
    /// Ring of recent actor commands; `recent_next` is the write cursor.
    pub recent: [Option<MutationRecord>; RECENT_LEN],
    pub recent_next: usize,
}

impl MutationLog {
    /// Record a command that was accepted and dispatched.
    pub fn record(
        &mut self,
        intent_index: usize,
        intent: &'static str,
        origin: MutationOrigin,
        mode: EngineMode,
        outcome: &MutationOutcome,
    ) {
        self.counts[intent_index][origin.index()] += 1;
        if origin == MutationOrigin::System {
            return;
        }
        self.recent[self.recent_next] = Some(MutationRecord {
            intent,
            origin,
            mode,
            changed: outcome.changed.len().min(u16::MAX as usize) as u16,
        });
        self.recent_next = (self.recent_next + 1) % RECENT_LEN;
    }

    /// Recent actor commands, newest first.
    pub fn recent_newest_first(&self) -> impl Iterator<Item = &MutationRecord> {
        (0..RECENT_LEN)
            .map(move |i| (self.recent_next + RECENT_LEN - 1 - i) % RECENT_LEN)
            .filter_map(move |i| self.recent[i].as_ref())
    }
}
