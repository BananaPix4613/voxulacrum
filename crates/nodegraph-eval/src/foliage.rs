//! Foliage generation output - the DetailGraph evaluator's product.
//!
//! These are the *generation-domain* foliage types. The application crate owns
//! the storage-domain `DetailLayers` / `ScatterStore` (per the voxel-core
//! charter) and translates a [`ChunkFoliage`] into them at the chunk-construction
//! boundary, mirroring the terrain `ChunkBuffer` -> `ChunkStorage` seam.

use crate::field::CHUNK_DIM;

/// One detail texel per chunk column.
const AREA: usize = CHUNK_DIM * CHUNK_DIM;

/// A per-column foliage paint cell (Tier 1). Mirrors the app's `DetailTexel`.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub struct PaintTexel {
    /// Species variant within the layer; `0` = none.
    pub species: u8,
    /// Density driving rendered blade count.
    pub density: u8,
    /// Tint palette index.
    pub tint: u8,
    /// Bitflags (reserved).
    pub flags: u8,
}

/// One paint layer: a per-column texel map over the chunk footprint.
#[derive(Clone, PartialEq, Debug)]
pub struct PaintLayer {
    /// Target detail layer id (app `DetailLayerId`).
    pub layer_id: u16,
    /// One texel per column, index `x + z*CHUNK_DIM`.
    pub texels: Box<[PaintTexel; AREA]>,
}

/// A placed scatter instance (Tier 2/3). Mirrors the app's `ScatterInstance`
/// plus the generated stable id.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FoliageInstance {
    /// Chunk-local voxel coords of the owning (anchor) cell.
    pub anchor: [u8; 3],
    /// Sub-voxel offset, ~1/128 voxel precision per axis.
    pub sub_offset: [i8; 3],
    /// Yaw quantized to `0..256`.
    pub rotation_y: u8,
    /// Scale / variant selector.
    pub scale_variant: u8,
    /// Prefab to instance (app `PrefabId`).
    pub prefab_id: u32,
    /// Stable identity (design doc §9); rederivable, not persisted.
    pub stable_id: u64,
    /// Instance flag bits.
    pub flags: u8,
}

/// Scatter instances for one scatter type.
#[derive(Clone, PartialEq, Debug)]
pub struct ScatterBucket {
    /// Scatter type bucket (app `ScatterTypeId`).
    pub type_id: u16,
    /// Instances of this type.
    pub instances: Vec<FoliageInstance>,
}

/// The foliage produced by evaluating a DetailGraph for one chunk.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ChunkFoliage {
    /// Tier-1 paint layers.
    pub paint: Vec<PaintLayer>,
    /// Tier-2/3 scatter buckets.
    pub scatter: Vec<ScatterBucket>,
}

impl ChunkFoliage {
    /// True if no paint or scatter was produced.
    pub fn is_empty(&self) -> bool {
        self.paint.is_empty() && self.scatter.is_empty()
    }

    /// Merge another chunk's foliage into this one. Used to union per-biome
    /// results (each biome writes disjoint columns/instances, so paint overlays
    /// only non-empty texels and scatter buckets concatenate).
    pub fn merge(&mut self, other: ChunkFoliage) {
        for layer in other.paint {
            match self.paint.iter_mut().find(|l| l.layer_id == layer.layer_id) {
                Some(existing) => {
                    for i in 0..AREA {
                        if layer.texels[i].species != 0 || layer.texels[i].density != 0 {
                            existing.texels[i] = layer.texels[i];
                        }
                    }
                }
                None => self.paint.push(layer),
            }
        }
        for bucket in other.scatter {
            match self.scatter.iter_mut().find(|b| b.type_id == bucket.type_id) {
                Some(existing) => existing.instances.extend(bucket.instances),
                None => self.scatter.push(bucket),
            }
        }
    }
}
