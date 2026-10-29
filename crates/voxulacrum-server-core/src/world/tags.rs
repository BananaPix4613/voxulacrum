//! Per-chunk identity tags (design doc §3, §16).
//!
//! [`ChunkTags`] records which Zone, Biome(s), and Library graphs contributed to
//! a chunk. Its purpose is targeted invalidation: when a graph edit lands, the
//! engine walks chunk tags to find exactly the chunks that must regenerate,
//! rather than rescanning the whole world (design doc §16). The data-model
//! `Chunk` carries one `ChunkTags`.
//!
//! Phase 3 ships ONE zone and ONE biome, so every chunk is trivially tagged
//! `ZoneId(0)` + `[BiomeId(0)]` via [`ChunkTags::single_biome`]. `library_refs`
//! stays empty until the graph hierarchy lands. The tag-driven invalidation
//! consumer and the on-disk encoding (Substep 7) arrive later.

use smallvec::SmallVec;

/// Identifies a Zone (the top-level region graph) that produced a chunk.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct ZoneId(pub u16);

/// Identifies a Biome graph contributing to a chunk.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct BiomeId(pub u16);

/// Identifies a Library graph referenced during a chunk's generation.
/// Re-exported from `nodegraph-ir` so chunk tags and graph nodes share one
/// `LibraryGraphId` type.
pub use nodegraph_ir::LibraryGraphId;

/// Per-chunk identity used for targeted regeneration. Defaults to empty so
/// absent saves load forward-compatibly; generation populates it explicitly.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ChunkTags {
    /// Zone(s) whose columns appear in this chunk.
    ///
    /// A **set**, not a representative. A chunk straddling a zone border belongs
    /// to both, and recording only the first would make an edit to the other
    /// zone skip it - invisibly, since the chunk still matches a tag it carries.
    /// This is also what satisfies design §4's "or within Z's fade range"
    /// clause: membership is recorded structurally rather than reconstructed
    /// from a radius.
    pub zones: SmallVec<[ZoneId; 2]>,
    /// Biome(s) blended into this chunk.
    pub biomes: SmallVec<[BiomeId; 4]>,
    /// Library graphs referenced while generating this chunk. Not yet populated
    /// - see Substep 6.
    pub library_refs: SmallVec<[LibraryGraphId; 8]>,
}

impl ChunkTags {
    /// Tags for a chunk produced by a single zone and a single biome, with no
    /// library references.
    pub fn single_biome(zone: ZoneId, biome: BiomeId) -> Self {
        let mut zones = SmallVec::new();
        zones.push(zone);
        let mut biomes = SmallVec::new();
        biomes.push(biome);
        Self {
            zones,
            biomes,
            library_refs: SmallVec::new(),
        }
    }
}
