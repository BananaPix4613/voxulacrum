//! Typed world events (design §6.7, roadmap §11).
//!
//! **Subscribers, not call sites.** The external review's argument for building
//! this early is that a region-scale change must invalidate biome assignment,
//! spawn tables, ambient audio, structure placement, pathfinding, lighting and
//! meshes across a large area - and hard-wiring those calls into the code that
//! makes the change means every future system edits that code. An event stream
//! inverts the dependency: the mutation door announces what happened and knows
//! nothing about who cares.
//!
//! **Ordering.** Events reach subscribers through `bevy_ecs`'s double-buffered
//! `Events<WorldEvent>`, so ordering relative to `FrameStage` is the schedule's
//! ordering: a system in a later stage sees what an earlier stage wrote.
//!
//! **One frame of latency, by construction.** The mutation door is not a system
//! and cannot hold an `EventWriter`, so it accumulates into an outbox on `World`
//! that a pump publishes at the start of the next frame. Systems that *are*
//! systems write directly and have no such delay. Nothing here is ordered
//! against wall-clock time, so the delay is a property to know rather than one
//! to design around.
//!
//! **Replication policy** (design §6.7's "declared local-vs-replicated policy
//! per event type") is not modeled yet: every event is local, because there is
//! no network. The policy is added when the wire exists (0.7.0), and adding a
//! field to this enum is not a format change - events are not persisted.

use bevy_ecs::prelude::Event;
use glam::IVec3;

/// Something that happened to the world, announced for anyone who cares.
///
/// Deliberately coarse. An event per changed voxel would be a per-cell firehose
/// during a fill; an event per *command* matches the granularity the door
/// already validates, logs, and would replicate at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Event)]
pub enum WorldEvent {
    /// Voxels in a chunk were edited. `cells` is how many.
    VoxelsChanged { chunk: IVec3, cells: u32 },
    /// Scatter instances at an anchor were added or removed.
    ScatterChanged { chunk: IVec3 },
    /// Fluid changed in `chunks` chunks (a poured column or a committed tick).
    FluidChanged { chunks: u32 },
    /// A generated or loaded chunk became resident.
    ChunkLoaded { chunk: IVec3 },
    /// A chunk left the resident set.
    ChunkUnloaded { chunk: IVec3 },
    /// Cross-chunk seam finalization committed for a chunk.
    SeamFinalized { chunk: IVec3 },
    /// A background regeneration was swapped in, replacing `chunks` chunks.
    WorldRegenerated { chunks: u32 },
    /// A graph file changed on disk and resolves to `slot`.
    ///
    /// The one variant here describing an *input* rather than a change to world
    /// state. It belongs on this bus because design §6.7's list is engine events
    /// broadly - it names player mode changed and world time crossed threshold
    /// alongside voxel changed - and because a second bus for one variant would
    /// be premature. If the set of input events grows, that is the trigger to
    /// split them.
    GraphChanged { slot: crate::world::world_generator::GraphSlot },
    /// A blueprint was stamped, its anchor landing on `origin`. `cells` is the
    /// blueprint's authored cell count - the fact the command carried, rather
    /// than a per-chunk decomposition a subscriber would have to reassemble.
    BlueprintStamped { origin: IVec3, cells: u32 },
}

impl WorldEvent {
    /// Number of variants.
    pub const COUNT: usize = 9;

    /// Dense index, for per-kind counters. Kept adjacent to [`Self::NAMES`] so
    /// adding a variant is a compile error in both places.
    pub fn index(&self) -> usize {
        match self {
            WorldEvent::VoxelsChanged { .. } => 0,
            WorldEvent::ScatterChanged { .. } => 1,
            WorldEvent::FluidChanged { .. } => 2,
            WorldEvent::ChunkLoaded { .. } => 3,
            WorldEvent::ChunkUnloaded { .. } => 4,
            WorldEvent::SeamFinalized { .. } => 5,
            WorldEvent::WorldRegenerated { .. } => 6,
            WorldEvent::GraphChanged { .. } => 7,
            WorldEvent::BlueprintStamped { .. } => 8,
        }
    }

    /// Display names, indexed by [`Self::index`].
    pub const NAMES: [&'static str; Self::COUNT] = [
        "voxels",
        "scatter",
        "fluid",
        "chunk-loaded",
        "chunk-unloaded",
        "seam",
        "regenerated",
        "graph-changed",
        "blueprint-stamped",
    ];
}
