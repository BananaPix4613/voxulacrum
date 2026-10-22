//! Cross-machine determinism manifest (roadmap §4.5, T7).
//!
//! `--determinism-manifest <file>` writes, as JSON, a hash of every Tier-1
//! result this machine produces (design §12, "Determinism tiers"). CI writes one
//! one each of its runners and compares them. `--verify-generation` compares a
//! run only with itself, so it cannot see a result that differs between
//! machines.
//!
//! Every hash is `seahash` over one layer's persistence encoding
//! (`world::persistence`), which is fixed-endian and key-sorted, so equal content
//! hashes equally on every platform and in every process, whatever seeds that
//! process's hash maps drew.
//!
//! **The generated base** is the chunk set `--verify-generation` samples, each
//! chunk hashed layer by layer. A comparison proves nothing about content the
//! sample does not hold, so the run also counts each `ChunkKind` and fails,
//! after writing the manifest, if any kind has no chunk.
//!
//! Like `--verify-generation`, this runs from the normal binary before the event
//! loop exists, so it needs no window or GPU, and it generates on rayon's global
//! pool outside the frame schedule. R3c moves it to the headless server binary.

use std::collections::BTreeMap;
use std::path::Path;

use glam::IVec3;
use nodegraph_eval::feature::StampCounts;
use rayon::prelude::*;
use serde::Serialize;
use voxel_core::LocalPos;

use crate::world::chunk::CHUNK_SIZE;
use crate::world::persistence;
use crate::world::storage::ChunkStorage;
use crate::world::world_generator::{world_manifest_path, GeneratedChunk, WorldGenerator};

/// Version of the manifest's shape. The compare tool refuses two manifests of
/// different formats rather than comparing fields that mean different things.
const FORMAT: u32 = 1;

/// Handle `--determinism-manifest <file>`.
///
/// Returns `Some(exit_code)` when this was a manifest run and the process should
/// exit, or `None` to fall through and start the engine normally.
pub fn run_from_args(args: &[String]) -> Option<i32> {
    let pos = args.iter().position(|a| a == "--determinism-manifest")?;
    match args.get(pos + 1).filter(|a| !a.starts_with("--")) {
        Some(path) => Some(write_manifest(Path::new(path))),
        None => {
            eprintln!("determinism-manifest: FAILED - give an output file: --determinism-manifest <file>");
            Some(2)
        }
    }
}

/// A kind of content the generated base must hold at least one chunk of, so a
/// comparison covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkKind {
    FluidBearing,
    TreeBearing,
    StructureBearing,
    MultiBiome,
    CaveCarved,
}

impl ChunkKind {
    const ALL: [Self; 5] = [
        Self::FluidBearing,
        Self::TreeBearing,
        Self::StructureBearing,
        Self::MultiBiome,
        Self::CaveCarved,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::FluidBearing => "fluid-bearing",
            Self::TreeBearing => "tree-bearing",
            Self::StructureBearing => "structure-bearing",
            Self::MultiBiome => "multi-biome",
            Self::CaveCarved => "cave-carved",
        }
    }

    fn holds(self, chunk: &GeneratedChunk) -> bool {
        match self {
            Self::FluidBearing => !chunk.fluids.cells.is_empty(),
            Self::TreeBearing => chunk.stamp_counts.trees > 0,
            Self::StructureBearing => chunk.stamp_counts.blueprints > 0,
            Self::MultiBiome => chunk.tags.biomes.len() >= 2,
            Self::CaveCarved => {
                chunk.stamp_counts == StampCounts::default() && has_roofed_air(&chunk.storage)
            }
        }
    }

    /// How [`Self::holds`] decides, and where to look when no chunk qualifies.
    fn detection(self) -> &'static str {
        match self {
            Self::FluidBearing => {
                "a chunk with explicit generated fluid cells: a pond or a shoreline. A fully \
                 submerged chunk does not count, since its fluid layer is one fill mode. Check \
                 the sea level and the biomes' fluid outputs."
            }
            Self::TreeBearing => {
                "a chunk a tree feature wrote wood into (stage 8's stamp counts). Check the zone \
                 graphs' PlaceTree sources and their biome filters."
            }
            Self::StructureBearing => {
                "a chunk a blueprint feature wrote cells into (stage 8's stamp counts). Check the \
                 zone graphs' PlaceStructure sources, their density and their cell size."
            }
            Self::MultiBiome => {
                "a chunk whose tags name two or more biomes. Check that the sample still crosses \
                 a biome border."
            }
            Self::CaveCarved => {
                "a chunk no feature wrote into that holds an air voxel directly under a solid \
                 one. A height-field density makes no such voxel, so it comes from 3D density: \
                 in the shipped world, the cave carve (voxulacrum:standard_cave_noise). Check the \
                 carve's threshold and the biomes that use it."
            }
        }
    }
}

/// Whether any air voxel in `storage` has a solid voxel directly above it in the
/// same chunk.
fn has_roofed_air(storage: &ChunkStorage) -> bool {
    let n = CHUNK_SIZE as u8;
    (0..n).any(|z| {
        (0..n).any(|x| {
            (0..n - 1).any(|y| {
                let below = LocalPos::new_unchecked(x, y, z).to_index();
                let above = LocalPos::new_unchecked(x, y + 1, z).to_index();
                !storage.voxel(below).is_solid() && storage.voxel(above).is_solid()
            })
        })
    })
}

#[derive(Serialize)]
struct Manifest {
    format: u32,
    /// The world's package namespace and seed, so the compare tool can refuse
    /// manifests of two different worlds rather than report them as mismatches.
    namespace: String,
    seed: u64,
    /// `os-arch` of the machine that wrote it. Informational.
    platform: String,
    /// How many chunks of the generated base hold each `ChunkKind`.
    kinds: BTreeMap<&'static str, usize>,
    sections: Vec<Section>,
}

/// One group of hashed results: the generated base, or a scenario.
#[derive(Serialize)]
struct Section {
    name: &'static str,
    records: Vec<Record>,
}

#[derive(Serialize)]
struct Record {
    /// What was hashed, for the compare tool to name.
    at: String,
    /// The `ChunkKind`s a generated chunk holds. Left out when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kinds: Vec<&'static str>,
    layers: Vec<LayerHash>,
}

#[derive(Serialize)]
struct LayerHash {
    layer: &'static str,
    hash: String,
}

impl LayerHash {
    /// Hash the bytes `write` encodes.
    fn of(layer: &'static str, write: impl FnOnce(&mut Vec<u8>)) -> Self {
        let mut buf = Vec::new();
        write(&mut buf);
        Self { layer, hash: format!("{:#018x}", seahash::hash(&buf)) }
    }
}

/// Every layer of one generated chunk, and the kinds it holds.
fn generated_record(pos: IVec3, chunk: &GeneratedChunk) -> Record {
    Record {
        at: format!("chunk ({}, {}, {})", pos.x, pos.y, pos.z),
        kinds: ChunkKind::ALL
            .into_iter()
            .filter(|kind| kind.holds(chunk))
            .map(ChunkKind::name)
            .collect(),
        layers: vec![
            LayerHash::of("voxels", |b| persistence::write_voxels(b, &chunk.storage)),
            LayerHash::of("fluid", |b| persistence::write_fluid_layer(b, &chunk.fluids)),
            LayerHash::of("scatter", |b| persistence::write_scatter_store(b, &chunk.scatter)),
            LayerHash::of("detail", |b| persistence::write_detail_layers(b, &chunk.detail_layers)),
            LayerHash::of("tags", |b| persistence::write_tags(b, &chunk.tags)),
        ],
    }
}

fn write_manifest(path: &Path) -> i32 {
    let world = world_manifest_path();
    let generator = match WorldGenerator::from_manifest(&world) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("determinism-manifest: FAILED to load world manifest: {e}");
            return 2;
        }
    };
    let count = crate::verify::DEFAULT_COUNT;
    println!("determinism-manifest: {count} chunks of {}", world.display());

    // `collect` preserves order, so the records follow the sample.
    let coords = crate::verify::positions(count);
    let records: Vec<Record> = coords
        .par_iter()
        .map(|&pos| generated_record(pos, &generator.generate_chunk(pos)))
        .collect();

    let counts: Vec<(ChunkKind, usize)> = ChunkKind::ALL
        .into_iter()
        .map(|kind| (kind, records.iter().filter(|r| r.kinds.contains(&kind.name())).count()))
        .collect();

    let manifest = Manifest {
        format: FORMAT,
        namespace: crate::paths::namespace().to_string(),
        seed: generator.world_seed(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        kinds: counts.iter().map(|&(kind, n)| (kind.name(), n)).collect(),
        sections: vec![Section { name: "generation", records }],
    };
    let json = serde_json::to_string_pretty(&manifest).expect("the manifest serializes");
    if let Err(e) = std::fs::write(path, json) {
        eprintln!("determinism-manifest: FAILED to write {}: {e}", path.display());
        return 2;
    }
    println!("  wrote {}", path.display());

    let mut missing = 0usize;
    for (kind, n) in counts {
        println!("  {:<18} {n} chunks", kind.name());
        if n == 0 {
            missing += 1;
            eprintln!("  MISSING: no {} chunk in the sample. Detected as {}", kind.name(), kind.detection());
        }
    }
    if missing > 0 {
        eprintln!(
            "determinism-manifest: FAILED - the sample holds no chunk of {missing} kind(s), \
             so a comparison would prove nothing about them",
        );
        return 1;
    }
    println!("determinism-manifest: ok");
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::{MaterialId, Voxel};

    #[test]
    fn roofed_air_is_found_under_an_overhang_and_not_under_open_sky() {
        let stone = Voxel::cube(MaterialId(1));
        let mut storage = ChunkStorage::new_air();
        // A height-field column: solid from the bottom up, air above.
        for y in 0..=4 {
            storage.set_voxel(LocalPos::new_unchecked(3, y, 3).to_index(), stone);
        }
        assert!(!has_roofed_air(&storage), "a height field roofs nothing");

        // An overhang: a solid voxel with air under it.
        storage.set_voxel(LocalPos::new_unchecked(3, 8, 3).to_index(), stone);
        assert!(has_roofed_air(&storage), "air under the overhang is roofed");
    }

    #[test]
    fn args_are_parsed_and_ignored_when_absent() {
        let none: Vec<String> = vec!["--some-other-flag".into()];
        assert!(run_from_args(&none).is_none(), "a non-manifest run falls through to the engine");

        let no_path: Vec<String> = vec!["--determinism-manifest".into(), "--seed".into()];
        assert_eq!(run_from_args(&no_path), Some(2), "a missing output path is refused");
    }
}
