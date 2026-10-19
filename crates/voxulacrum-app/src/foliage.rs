//! Data-driven foliage registry: Tier-1 detail paint layers with the species
//! within each, and the scatter types Tier-2/3 instances are bucketed by.
//!
//! Authored in `foliage.ron` beside `materials.ron`, RON-primary with a
//! hand-authored fallback so the engine never fails to start over foliage.
//! Names are bare in the file and qualified with the content's namespace at
//! load, as the other registries do. Layer and species ids stay authored
//! because saved paint edits store them; scatter type ids are not saved but are
//! authored the same way.

use std::path::PathBuf;
use std::sync::Arc;

use nodegraph_ir::{Graph, NodeKind, ScatterTypeRef};
use serde::Deserialize;
use voxel_core::QualifiedName;

use crate::world::layers::DetailLayerId;

/// Species slots per layer in the shader's color table: every `u8` species id.
pub const SPECIES_PER_LAYER: usize = 256;

/// Color of a table entry no species defines: visibly wrong, as the material
/// color table's padding is.
const UNDEFINED_COLOR: [f32; 4] = [1.0, 0.0, 1.0, 1.0];

/// One species: a variant within a detail layer.
#[derive(Clone, PartialEq, Debug)]
pub struct SpeciesDef {
    /// Id within the layer, `1..=255`; `0` is "no species" in a texel.
    pub id: u8,
    /// Identity, qualified with the namespace its content loads under.
    pub name: QualifiedName,
    /// Base color (linear RGB).
    pub color: [f32; 3],
}

/// One detail paint layer and the species it defines.
#[derive(Clone, PartialEq, Debug)]
pub struct DetailLayerDef {
    /// Stable id (contiguous from 0), authoritative over file order.
    pub id: u16,
    /// Identity, qualified with the namespace its content loads under.
    pub name: QualifiedName,
    /// Species in id order.
    pub species: Vec<SpeciesDef>,
}

/// One scatter type: a named bucket Tier-2/3 instances are grouped by.
#[derive(Clone, PartialEq, Debug)]
pub struct ScatterTypeDef {
    /// Stable id (contiguous from 0).
    pub id: u16,
    /// Identity, qualified with the namespace its content loads under.
    pub name: QualifiedName,
}

/// Detail layers (index == `DetailLayerId`) and scatter types (index == id).
#[derive(Clone, Debug)]
pub struct FoliageRegistry {
    layers: Vec<DetailLayerDef>,
    scatter_types: Vec<ScatterTypeDef>,
}

impl FoliageRegistry {
    /// All detail layers in id order.
    pub fn layers(&self) -> &[DetailLayerDef] {
        &self.layers
    }

    /// All scatter types in id order.
    pub fn scatter_types(&self) -> &[ScatterTypeDef] {
        &self.scatter_types
    }

    /// The shader's slot for layer `id`: its index in the table, which the
    /// contiguity rule makes equal to the id. `None` for an undefined layer.
    pub fn layer_slot(&self, id: DetailLayerId) -> Option<u32> {
        ((id.0 as usize) < self.layers.len()).then_some(id.0 as u32)
    }

    /// Species colors for the shader, `SPECIES_PER_LAYER` per layer, indexed
    /// `layer_slot * SPECIES_PER_LAYER + species`. Undefined entries are
    /// magenta. At least one layer's worth, because a storage binding cannot be
    /// empty; with no layers that one is all magenta and no slot reaches it.
    pub fn species_color_table(&self) -> Vec<[f32; 4]> {
        let layers = self.layers.len().max(1);
        let mut table = vec![UNDEFINED_COLOR; layers * SPECIES_PER_LAYER];
        for (slot, layer) in self.layers.iter().enumerate() {
            for s in &layer.species {
                let [r, g, b] = s.color;
                table[slot * SPECIES_PER_LAYER + s.id as usize] = [r, g, b, 1.0];
            }
        }
        table
    }

    /// Set the id of every foliage reference in `graph` from its name, read in
    /// `namespace`: each `PaintDensity`'s layer and its species (looked up in
    /// that layer), and the scatter type of each `ScatterPlace` and
    /// `PlaceStructure` canopy. Ids are always rewritten, like material ids.
    /// Every node is visited; the error lists each name that is malformed or
    /// not defined.
    pub fn resolve_refs(&self, graph: &mut Graph, namespace: &str) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        for (_, node) in graph.nodes.iter_mut() {
            match &mut node.kind {
                NodeKind::PaintDensity(p) => {
                    match find(&self.layers, |l| &l.name, p.layer.name(), namespace, "detail layer") {
                        Ok(layer) => {
                            p.layer.set_id(layer.id);
                            let what = format!("species in layer {}", layer.name);
                            match find(&layer.species, |s| &s.name, p.species.name(), namespace, &what) {
                                Ok(species) => p.species.set_id(species.id),
                                Err(e) => problems.push(e),
                            }
                        }
                        Err(e) => problems.push(e),
                    }
                }
                NodeKind::ScatterPlace(p) => {
                    self.resolve_scatter_type(&mut p.scatter_type, namespace, &mut problems)
                }
                NodeKind::PlaceStructure(p) => {
                    if let Some(canopy) = p.canopy.as_mut() {
                        self.resolve_scatter_type(&mut canopy.scatter_type, namespace, &mut problems);
                    }
                }
                _ => {}
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems) }
    }

    fn resolve_scatter_type(
        &self,
        reference: &mut ScatterTypeRef,
        namespace: &str,
        problems: &mut Vec<String>,
    ) {
        match find(&self.scatter_types, |t| &t.name, reference.name(), namespace, "scatter type") {
            Ok(t) => reference.set_id(t.id),
            Err(e) => problems.push(e),
        }
    }

    /// Load from a RON file, naming entries in `namespace`.
    pub fn load_from_ron(path: &std::path::Path, namespace: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron_str(&text, namespace)
    }

    /// Parse and validate: layer and scatter type ids contiguous from 0,
    /// species ids unique in `1..=255`, every name valid and unique within its
    /// list.
    fn from_ron_str(text: &str, namespace: &str) -> Result<Self, String> {
        let file: FoliageFileRon = ron::from_str(text).map_err(|e| e.to_string())?;
        Self::from_entries(file.layers, file.scatter_types, namespace)
    }

    fn from_entries(
        mut layer_entries: Vec<LayerEntryRon>,
        mut type_entries: Vec<ScatterTypeEntryRon>,
        namespace: &str,
    ) -> Result<Self, String> {
        layer_entries.sort_by_key(|l| l.id);
        let mut layers: Vec<DetailLayerDef> = Vec::with_capacity(layer_entries.len());
        for (i, entry) in layer_entries.into_iter().enumerate() {
            if entry.id as usize != i {
                return Err(format!(
                    "detail layer ids must be contiguous from 0; expected {i}, found {}",
                    entry.id
                ));
            }
            let name = QualifiedName::new(namespace, &entry.id_name)
                .map_err(|e| format!("detail layer id {}: {e}", entry.id))?;
            if layers.iter().any(|l| l.name == name) {
                return Err(format!("duplicate detail layer id_name: {:?}", entry.id_name));
            }
            let mut species: Vec<SpeciesDef> = Vec::with_capacity(entry.species.len());
            for s in entry.species {
                if s.id == 0 {
                    return Err(format!("layer {name}: species id 0 is reserved for no species"));
                }
                let species_name = QualifiedName::new(namespace, &s.id_name)
                    .map_err(|e| format!("layer {name}, species id {}: {e}", s.id))?;
                if species.iter().any(|d| d.id == s.id) {
                    return Err(format!("layer {name}: duplicate species id {}", s.id));
                }
                if species.iter().any(|d| d.name == species_name) {
                    return Err(format!("layer {name}: duplicate species id_name: {:?}", s.id_name));
                }
                species.push(SpeciesDef { id: s.id, name: species_name, color: s.color });
            }
            species.sort_by_key(|s| s.id);
            layers.push(DetailLayerDef { id: entry.id, name, species });
        }

        type_entries.sort_by_key(|t| t.id);
        let mut scatter_types: Vec<ScatterTypeDef> = Vec::with_capacity(type_entries.len());
        for (i, entry) in type_entries.into_iter().enumerate() {
            if entry.id as usize != i {
                return Err(format!(
                    "scatter type ids must be contiguous from 0; expected {i}, found {}",
                    entry.id
                ));
            }
            let name = QualifiedName::new(namespace, &entry.id_name)
                .map_err(|e| format!("scatter type id {}: {e}", entry.id))?;
            if scatter_types.iter().any(|t| t.name == name) {
                return Err(format!("duplicate scatter type id_name: {:?}", entry.id_name));
            }
            scatter_types.push(ScatterTypeDef { id: entry.id, name });
        }
        Ok(Self { layers, scatter_types })
    }

    /// Hand-authored fallback, kept in sync with `assets/foliage.ron` (locked by
    /// a test), named in `namespace` like a loaded table.
    pub fn load_initial(namespace: &str) -> Self {
        let species = |id: u8, id_name: &str, color: [f32; 3]| SpeciesEntryRon {
            id,
            id_name: id_name.to_string(),
            color,
        };
        let scatter_type = |id: u16, id_name: &str| ScatterTypeEntryRon {
            id,
            id_name: id_name.to_string(),
        };
        let layers = vec![LayerEntryRon {
            id: 0,
            id_name: "grass".to_string(),
            species: vec![
                species(1, "meadow", [0.20, 0.55, 0.22]),
                species(2, "dry", [0.45, 0.52, 0.20]),
            ],
        }];
        let types = vec![scatter_type(0, "tufts"), scatter_type(1, "bushes"), scatter_type(2, "rocks")];
        Self::from_entries(layers, types, namespace).expect("the built-in foliage table is valid")
    }
}

/// The entry of `entries` whose name is `text` read in `namespace`, or a message
/// naming `what` was not found.
fn find<'a, T>(
    entries: &'a [T],
    name_of: impl Fn(&T) -> &QualifiedName,
    text: &str,
    namespace: &str,
    what: &str,
) -> Result<&'a T, String> {
    let name = QualifiedName::parse(text, namespace).map_err(|e| format!("{what} {e}"))?;
    entries
        .iter()
        .find(|e| *name_of(e) == name)
        .ok_or_else(|| format!("{what} {name} is not defined"))
}

/// On-disk schema for one species.
#[derive(Deserialize)]
struct SpeciesEntryRon {
    id: u8,
    id_name: String,
    color: [f32; 3],
}

/// On-disk schema for one detail layer.
#[derive(Deserialize)]
struct LayerEntryRon {
    id: u16,
    id_name: String,
    #[serde(default)]
    species: Vec<SpeciesEntryRon>,
}

/// On-disk schema for one scatter type.
#[derive(Deserialize)]
struct ScatterTypeEntryRon {
    id: u16,
    id_name: String,
}

/// On-disk schema of `foliage.ron`.
#[derive(Deserialize)]
struct FoliageFileRon {
    layers: Vec<LayerEntryRon>,
    #[serde(default)]
    scatter_types: Vec<ScatterTypeEntryRon>,
}

/// Path to the authored foliage table.
pub fn foliage_path() -> PathBuf {
    crate::paths::content_root().join("foliage.ron")
}

/// Load the shared foliage registry: RON-primary, `load_initial` fallback.
///
/// Never fails - a missing or invalid RON file is logged at error level and the
/// hand-authored fallback table is used instead.
pub fn load_foliage_registry() -> Arc<FoliageRegistry> {
    let path = foliage_path();
    let namespace = crate::paths::namespace();
    match FoliageRegistry::load_from_ron(&path, namespace) {
        Ok(reg) => {
            log::info!(
                "Loaded {} detail layers and {} scatter types from {}",
                reg.layers.len(),
                reg.scatter_types.len(),
                path.display()
            );
            Arc::new(reg)
        }
        Err(e) => {
            log::error!(
                "Failed to load foliage from {}: {e}. Falling back to built-in table.",
                path.display()
            );
            Arc::new(FoliageRegistry::load_initial(namespace))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::{DetailLayerRef, DetailSpeciesRef, PaintDensityParams, PrefabRef, ScatterPlaceParams};
    use voxel_core::ENGINE_NAMESPACE;

    fn parse(text: &str) -> Result<FoliageRegistry, String> {
        FoliageRegistry::from_ron_str(text, ENGINE_NAMESPACE)
    }

    #[test]
    fn the_shipped_grass_keeps_the_colors_the_shader_hard_coded() {
        // The values `base_color_for` in detail_paint.wgsl returned before this
        // table replaced it: species 1 (and anything unknown) green, 2 golden.
        let reg = parse(include_str!("../../../assets/foliage.ron")).expect("foliage.ron is valid");
        let table = reg.species_color_table();
        assert_eq!(table[1], [0.20, 0.55, 0.22, 1.0], "meadow");
        assert_eq!(table[2], [0.45, 0.52, 0.20, 1.0], "dry");
        assert_eq!(table[3], UNDEFINED_COLOR, "an undefined species is visibly wrong");
        assert_eq!(table.len(), SPECIES_PER_LAYER);
    }

    #[test]
    fn fallback_matches_shipped_ron() {
        let from_file = parse(include_str!("../../../assets/foliage.ron")).unwrap();
        let initial = FoliageRegistry::load_initial(ENGINE_NAMESPACE);
        assert_eq!(from_file.layers, initial.layers, "foliage.ron and load_initial() layers drifted");
        assert_eq!(
            from_file.scatter_types, initial.scatter_types,
            "foliage.ron and load_initial() scatter types drifted"
        );
    }

    #[test]
    fn an_empty_table_still_yields_a_bindable_color_table() {
        let reg = parse("(layers: [])").unwrap();
        assert_eq!(reg.layer_slot(DetailLayerId(0)), None);
        assert!(reg.scatter_types().is_empty(), "scatter_types may be omitted");
        let table = reg.species_color_table();
        assert_eq!(table.len(), SPECIES_PER_LAYER);
        assert!(table.iter().all(|c| *c == UNDEFINED_COLOR));
    }

    #[test]
    fn the_slot_is_the_layer_id() {
        let reg = FoliageRegistry::load_initial(ENGINE_NAMESPACE);
        assert_eq!(reg.layer_slot(DetailLayerId(0)), Some(0));
        assert_eq!(reg.layer_slot(DetailLayerId(1)), None);
    }

    #[test]
    fn non_contiguous_layer_ids_rejected() {
        let err = parse(r#"(layers: [(id: 0, id_name: "a"), (id: 2, id_name: "b")])"#).unwrap_err();
        assert!(err.contains("contiguous"), "{err}");
    }

    #[test]
    fn non_contiguous_scatter_type_ids_rejected() {
        let err = parse(r#"(layers: [], scatter_types: [(id: 1, id_name: "a")])"#).unwrap_err();
        assert!(err.contains("scatter type ids must be contiguous"), "{err}");
    }

    #[test]
    fn species_zero_rejected() {
        let err = parse(
            r#"(layers: [(id: 0, id_name: "a", species: [(id: 0, id_name: "s", color: (0.0, 0.0, 0.0))])])"#,
        )
            .unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn duplicate_species_rejected() {
        let err = parse(
            r#"(layers: [(id: 0, id_name: "a", species: [
                (id: 1, id_name: "s", color: (0.0, 0.0, 0.0)),
                (id: 2, id_name: "s", color: (0.0, 0.0, 0.0)),
            ])])"#,
        )
            .unwrap_err();
        assert!(err.contains("duplicate species id_name"), "{err}");
    }

    #[test]
    fn a_name_that_is_not_a_name_is_rejected() {
        let err = parse(r#"(layers: [(id: 0, id_name: "Tall Grass")])"#).unwrap_err();
        assert!(err.contains("Tall Grass"), "{err}");
    }

    fn paint(layer: &str, species: &str) -> NodeKind {
        NodeKind::PaintDensity(PaintDensityParams {
            layer: DetailLayerRef::new(layer),
            species: DetailSpeciesRef::new(species),
            density: 255,
            tint: 0,
        })
    }

    #[test]
    fn paint_names_resolve_to_layer_and_species_ids() {
        let reg = FoliageRegistry::load_initial(ENGINE_NAMESPACE);
        let mut g = Graph::new();
        let n = g.add_node(paint("grass", "dry"));
        reg.resolve_refs(&mut g, ENGINE_NAMESPACE).unwrap();
        let NodeKind::PaintDensity(p) = &g.nodes[n].kind else { unreachable!() };
        assert_eq!((p.layer.id(), p.species.id()), (Some(0), Some(2)));
    }

    #[test]
    fn a_scatter_type_name_resolves_to_its_id() {
        let reg = FoliageRegistry::load_initial(ENGINE_NAMESPACE);
        let mut g = Graph::new();
        let n = g.add_node(NodeKind::ScatterPlace(ScatterPlaceParams {
            seed: 0,
            scatter_type: ScatterTypeRef::new("bushes"),
            prefab: PrefabRef::new("bush"),
        }));
        reg.resolve_refs(&mut g, ENGINE_NAMESPACE).unwrap();
        let NodeKind::ScatterPlace(p) = &g.nodes[n].kind else { unreachable!() };
        assert_eq!(p.scatter_type.id(), Some(1));
    }

    #[test]
    fn every_unknown_foliage_name_is_reported() {
        let reg = FoliageRegistry::load_initial(ENGINE_NAMESPACE);
        let mut g = Graph::new();
        g.add_node(paint("ferns", "meadow")); // unknown layer
        g.add_node(paint("grass", "tufts")); // a scatter type, not a grass species
        g.add_node(paint("", "")); // the palette default
        let problems = reg.resolve_refs(&mut g, ENGINE_NAMESPACE).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[1].contains("species in layer voxulacrum:grass"), "{problems:?}");
    }
}
