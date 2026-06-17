//! Parallel chunk data layers (design doc §3, §6, §7).
//!
//! These are the sidecar layers that hang off the data-model `Chunk` alongside
//! its voxels: foliage paint ([`DetailLayers`]), discrete scatter
//! ([`ScatterStore`]), fluids ([`FluidLayer`]), surface decals ([`DecalLayer`]),
//! and baked lighting ([`LightData`]). Per voxel-core's charter these sidecar
//! types live in the application crate, not `voxel-core`.
//!
//! Phase 3 ships them as **empty scaffolding**: every layer constructs empty via
//! `Default`, carries no generation logic, and has no consumers yet. The
//! data-model `Chunk` (Substep 3) is the first consumer; the multi-layer
//! persistence format (Subset 7) is the second.

#![allow(dead_code)] // Phase 3 scaffolding. Persistence (Substep 7) now consumes the
                     // override-diff and id types (DetailTexel, ScatterInstance,
                     // FluidCell, DecalEntry, the *Id newtypes), and the data Chunk
                     // holds the layer containers. The remaining surface - flag bits
                     // (ScatterFlags HARVESTABLE/PERSISTENT, FluidCell FLAG_*),
                     // FluidFillMode::Submerged, ScatterStore::by_type,
                     // DetailLayer::new, ScatterTypeId - awaits the foliage/fluid/
                     // decal systems in later phases, so this allow stays until then.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;
use voxel_core::{FaceAxis, LocalPos};

use super::chunk::{CHUNK_SIZE, CHUNK_VOLUME};

/// Number of columns in a chunk's XZ footprint (one detail texel per column).
const CHUNK_AREA: usize = CHUNK_SIZE * CHUNK_SIZE;

// ============================================================================
// Layer identifiers
// ============================================================================

/// Identifies a detail (foliage paint) layer within a chunk's [`DetailLayers`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct DetailLayerId(pub u16);

/// Identifies a scatter type (a foliage/prop species bucket) within a [`ScatterStore`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct ScatterTypeId(pub u16);

/// Identifies a prefab referenced by a [`ScatterInstance`].
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct PrefabId(pub u32);

/// Identifies a fluid kind (water, lava, …).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct FluidId(pub u16);

/// Stable identity for a generated scatter instance, surviving regeneration.
///
/// Design doc §9: `hash(world_seed, world_pos, prefab_id, sequence_in_anchor)`.
/// The hashing scheme is implemented when scatter generation lands; Phase 3
/// only needs the type to exist for override bookkeeping.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct StableInstanceId(pub u64);

// ============================================================================
// Tier 1 — Detail scatter (foliage paint)
// ============================================================================

/// A single detail-map cell: a per-column foliage descriptor.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct DetailTexel {
    /// Variant within the layer; `0` = none.
    pub species: u8,
    /// Density driving rendered blade count.
    pub density: u8,
    /// Palette index for tint.
    pub tint: u8,
    /// Bitflags: trampled, seasonal, wind strength, …
    pub flags: u8,
}

/// One detail paint layer: a 2D map over the chunk's XZ footprint.
#[derive(Clone, PartialEq, Debug)]
pub struct DetailLayer {
    /// Which detail layer this is.
    pub layer_id: DetailLayerId,
    /// One texel per chunk column (`CHUNK_SIZE x CHUNK_SIZE`).
    pub map: Box<[DetailTexel; CHUNK_AREA]>,
}

impl DetailLayer {
    /// Construct an all-empty paint map for `layer_id`.
    pub fn new(layer_id: DetailLayerId) -> Self {
        Self {
            layer_id,
            map: Box::new([DetailTexel::default(); CHUNK_AREA]),
        }
    }
}

/// The set of detail paint layers for a chunk (Tier 1 foliage).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct DetailLayers {
    /// Active paint layers; empty by default.
    pub layers: SmallVec<[DetailLayer; 4]>,
}

// ============================================================================
// Tier 2/3 — Discrete scatter (instances + prefabs)
// ============================================================================

/// Per-instance flag bits for a [`ScatterInstance`].
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct ScatterFlags(pub u8);

impl ScatterFlags {
    /// Placed by the player (an override, not generated).
    pub const PLAYER_PLACED: u8 = 1 << 0;
    /// Can be harvested.
    pub const HARVESTABLE: u8 = 1 << 1;
    /// Persists across regeneration.
    pub const PERSISTENT: u8 = 1 << 2;
}

/// A single placed foliage/prop instance.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct ScatterInstance {
    /// Owning voxel cell.
    pub anchor: LocalPos,
    /// Sub-voxel offset (~1/128 voxel precision per axis).
    pub sub_offset: [i8; 3],
    /// Yaw, quantized to `0..256`.
    pub rotation_y: u8,
    /// Scale variant selector.
    pub scale_variant: u8,
    /// Prefab reference.
    pub prefab_id: PrefabId,
    /// Instance flags.
    pub flags: ScatterFlags,
}

/// Per-chunk collection of scatter instances, indexed by type (Tier 2/3 foliage).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ScatterStore {
    /// Instances grouped by scatter type; empty by default.
    pub by_type: HashMap<ScatterTypeId, Vec<ScatterInstance>>,
}

// ============================================================================
// Fluids (design doc §7)
// ============================================================================

/// A single cell's fluid state.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct FluidCell {
    /// Fluid kind.
    pub fluid_id: FluidId,
    /// Fixed-point mass, `0..=65535` (`65535` = full source-equivalent).
    pub mass: u16,
    /// Bitflags: settled, falling, source.
    pub flags: u8,
}

impl FluidCell {
    /// Bit 0: this cell has settled (removed from the active set).
    pub const FLAG_SETTLED: u8 = 1 << 0;
    /// Bit 1: this cell is falling.
    pub const FLAG_FALLING: u8 = 1 << 1;
    /// Bit 2: this cell is a source.
    pub const FLAG_SOURCE: u8 = 1 << 2;
}

/// Chunk-level default for fluid presence.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum FluidFillMode {
    /// No fluid anywhere by default.
    #[default]
    Empty,
    /// Every empty voxel is full of this fluid by default (ocean fast path).
    Submerged(FluidId),
}

/// The fluid layer for a chunk: a fill-mode default plus sparse per-cell deviations.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FluidLayer {
    /// Chunk-wide default presence.
    pub fill_mode: FluidFillMode,
    /// Explicit per-cell deviations from `fill_mode`.
    pub cells: HashMap<LocalPos, FluidCell>,
    /// Cells currently ticking in the simulation.
    pub active: HashSet<LocalPos>,
}

// ============================================================================
// Decals (reserved)
// ============================================================================

/// A single surface decal entry.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct DecalEntry {
    /// Decal kind.
    pub decal_id: u16,
    /// Bitflags, reserved.
    pub flags: u8,
}

/// The decal layer for a chunk: surface marks keyed by cell + face.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct DecalLayer {
    /// Decals keyed by `(cell, face)`; empty by default.
    pub entries: HashMap<(LocalPos, FaceAxis), DecalEntry>,
}

// ============================================================================
// Lighting (design doc §3: baked skylight + block light)
// ============================================================================

/// Baked per-chunk lighting: skylight and block light, one byte per voxel each.
///
/// Empty by default (both `None`) - an unlit chunk costs O(1). Phase 3 does not
/// run a lighting bake (design doc §5 stage 11 is out of scope); this type
/// exists so the data `Chunk` has a lighting layer to default-construct.
///
/// This is the canonical lighting home for the data-model `Chunk`. The legacy
/// `storage::PopulatedChunk` lighting tier was removed in Substep 3a; Phase 3
/// does not run a bake, so this stays empty.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LightData {
    /// Baked skylight, `0..=255` per voxel; `None` when unbaked.
    pub skylight: Option<Box<[u8; CHUNK_VOLUME]>>,
    /// Baked block light, `0..=255` per voxel; `None` when unbaked.
    pub block_light: Option<Box<[u8; CHUNK_VOLUME]>>,
}
