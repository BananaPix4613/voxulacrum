//! Stable material registry: numeric [`MaterialId`]s plus snake_case name
//! resolution. The table is data-driven: `packages/sandbox/materials.ron` is its only
//! source, loaded via [`MaterialRegistry::load_from_ron`] or
//! [`MaterialRegistry::from_ron_str`]. This crate keeps no copy of it.
//!
//! The file names each material with a bare `id_name`; the loader qualifies it
//! with the namespace its content loads under, so `granite` in the sandbox
//! package is `sandbox:granite`.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{VoxelCoreError, VoxelCoreResult};
use crate::material::MaterialId;
use crate::name::QualifiedName;

/// Authored definition of one material.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    /// Identity used by content and [`MaterialRegistry::resolve`].
    pub name: QualifiedName,
    /// Human-readable label, for UI only.
    pub display_name: String,
    /// Linear RGB base color.
    pub color: [f32; 3],
    /// Edge-sharpness hint (0..=1).
    pub sharpness: f32,
    /// Mining hardness (0..=1).
    pub hardness: f32,
    /// Whether fluids / roots pass through.
    pub permeable: bool,
    /// Whether flora can root on top.
    pub supports_flora: bool,
    /// Another render pass owns this material's appearance, so the terrain
    /// mesher must not emit faces for it.
    ///
    /// Occupancy and rendering are separate: a render-delegated material still
    /// collides, is still mined, and still occludes - it is simply drawn by
    /// something that knows its real shape. Wood is the case this exists for,
    /// because a tree drawn as cubes is the exact failure the tapered-capsule
    /// impostor pass was chosen to avoid.
    pub render_delegated: bool,
}

/// Ordered material table. The index into `entries` is the numeric
/// [`MaterialId`]; ids are stable across runs (Air=0 … Leaves=10).
pub struct MaterialRegistry {
    entries: Vec<MaterialDef>,
    by_name: HashMap<QualifiedName, MaterialId>,
    /// `render_delegated` per id, indexed by [`MaterialId`]. Built once here so
    /// the mesher can test a material with one slice index rather than a
    /// registry lookup per voxel - a 34^3 snapshot is 39,304 of them, up to 32
    /// times a frame.
    delegated: Vec<bool>,
}

impl MaterialRegistry {
    fn from_entries(entries: Vec<MaterialDef>) -> Self {
        let mut by_name = HashMap::with_capacity(entries.len());
        for (i, def) in entries.iter().enumerate() {
            let id = MaterialId(i as u16);
            if by_name.insert(def.name.clone(), id).is_some() {
                panic!("duplicate material name: {}", def.name);
            }
        }
        let delegated = entries.iter().map(|d| d.render_delegated).collect();
        Self { entries, by_name, delegated }
    }

    /// `render_delegated` per material id, for the snapshot extractor.
    ///
    /// Derived from `entries` rather than stored alongside them, so there is one
    /// copy of the fact, and it cannot drift from the table it describes.
    pub fn render_delegated_mask(&self) -> &[bool] {
        &self.delegated
    }

    /// Resolve a name to its stable numeric id.
    pub fn resolve(&self, name: &QualifiedName) -> Option<MaterialId> {
        self.by_name.get(name).copied()
    }

    /// Resolve a material name as content writes it: a bare name resolves in
    /// `namespace`, the namespace of the content holding the reference. The
    /// error says whether the text was malformed or names no material.
    pub fn resolve_str(&self, text: &str, namespace: &str) -> Result<MaterialId, String> {
        let name = QualifiedName::parse(text, namespace)?;
        self.resolve(&name)
            .ok_or_else(|| format!("material {name} is not in the registry"))
    }

    /// Look up the definition for a numeric id.
    pub fn get(&self, id: MaterialId) -> Option<&MaterialDef> {
        self.entries.get(id.0 as usize)
    }

    /// Number of registered materials.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the registry holds no materials.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load the material table from a RON file (`packages/sandbox/materials.ron`),
    /// naming its entries in `namespace`.
    ///
    /// Each material declares an explicit stable numeric `id`. The loader
    /// validates that ids are unique and form a contiguous range `0..len`, and
    /// that `id_name's are valid names and unique, then orders the table by id
    /// so the positional [`MaterialId`] equals the declared id.
    pub fn load_from_ron(path: &Path, namespace: &str) -> VoxelCoreResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            VoxelCoreError::MaterialRonRead { path: path.display().to_string(), source: e }
        })?;
        Self::from_ron_str(&text, namespace)
    }

    /// Parse + validate a RON document already in memory. Factored out of
    /// [`Self::load_from_ron`] so validation is unit-testable without the
    /// filesystem, and so a build can hold a copy of the shipped file.
    #[cfg(feature = "serde")]
    pub fn from_ron_str(text: &str, namespace: &str) -> VoxelCoreResult<Self> {
        let file: MaterialFileRon = ron::from_str(text)
            .map_err(|e| VoxelCoreError::MaterialRonParse(e.to_string()))?;

        let mut entries = file.materials;
        // Order by declared id so the Vec index lines up with the stable id.
        entries.sort_by_key(|e| e.id);

        // Ids must be a contiguous range 0..len (this also rejects duplicates,
        // which break contiguity after sorting).
        for (expected, e) in entries.iter().enumerate() {
            if e.id as usize != expected {
                return Err(VoxelCoreError::MaterialRonValidation(format!(
                    "material ids must be a contiguous range from 0; expected id {expected}, \
                     found {} (id_name {:?})",
                    e.id, e.id_name
                )));
            }
        }

        // Each id_names must be a valid name, and unique.
        let mut seen = std::collections::HashSet::with_capacity(entries.len());
        let mut defs = Vec::with_capacity(entries.len());
        for e in entries {
            let name = QualifiedName::new(namespace, &e.id_name).map_err(|err| {
                VoxelCoreError::MaterialRonValidation(format!("material id {}: {err}", e.id))
            })?;
            if !seen.insert(name.clone()) {
                return Err(VoxelCoreError::MaterialRonValidation(format!(
                    "duplicate material id_name: {:?}",
                    e.id_name
                )));
            }
            defs.push(MaterialDef {
                name,
                display_name: e.display_name,
                color: e.color,
                sharpness: e.sharpness,
                hardness: e.hardness,
                permeable: e.permeable,
                supports_flora: e.supports_flora,
                render_delegated: e.render_delegated,
            });
        }

        Ok(Self::from_entries(defs))
    }

    /// Stub for builds without the `serde` feature (RON loading needs serde
    /// derives). The engine always builds with `serde` (the default feature).
    #[cfg(not(feature = "serde"))]
    pub fn load_from_ron(_path: &Path, _namespace: &str) -> VoxelCoreResult<Self> {
        Err(VoxelCoreError::MaterialRonParse(
            "voxel-core built without the `serde` feature; RON loading unavailable".into(),
        ))
    }
}

/// On-disk schema for one material entry in `materials.ron`. Carries an explicit
/// stable `id` so file ordering can't silently renumber materials.
#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize)]
struct MaterialEntryRon {
    id: u16,
    id_name: String,
    display_name: String,
    color: [f32; 3],
    sharpness: f32,
    hardness: f32,
    permeable: bool,
    supports_flora: bool,
    /// Optional so the overwhelming majority of entries - everything the mesher
    /// draws normally - say nothing at all.
    #[serde(default)]
    render_delegated: bool,
}

/// Top-level schema of `materials.ron`.
#[cfg(feature = "serde")]
#[derive(Debug, serde::Deserialize)]
struct MaterialFileRon {
    materials: Vec<MaterialEntryRon>,
}

/// The shipped table, for this crate's tests. `packages/sandbox/materials.ron` is the only
/// source of the table, so tests read it rather than keeping a copy of their own.
#[cfg(all(test, feature = "serde"))]
pub(crate) fn shipped_for_tests() -> MaterialRegistry {
    MaterialRegistry::from_ron_str(
        include_str!("../../../packages/sandbox/materials.ron"),
        crate::name::ENGINE_NAMESPACE,
    )
    .expect("the shipped packages/sandbox/materials.ron is valid")
}

#[cfg(all(test, feature = "serde"))]
mod ron_tests {
    use super::*;
    use crate::name::ENGINE_NAMESPACE;

    #[test]
    fn the_shipped_table_loads_and_air_is_id_zero() {
        let r = shipped_for_tests();
        assert!(!r.is_empty(), "the shipped table has materials");
        assert_eq!(
            r.resolve_str("air", ENGINE_NAMESPACE),
            Ok(MaterialId(0)),
            "id 0 is reserved for air"
        );
    }

    #[test]
    fn the_delegated_mask_tracks_the_table() {
        let r = shipped_for_tests();
        let mask = r.render_delegated_mask();
        assert_eq!(mask.len(), r.len(), "mask and table must be the same length");
        for (i, def) in r.entries.iter().enumerate() {
            assert_eq!(mask[i], def.render_delegated, "mask disagrees at id {i}");
        }
    }

    #[test]
    fn wood_is_render_delegated_and_terrain_is_not() {
        // The invariant the tree work rests on: wood occupies voxels but is
        // drawn by the capsule impostor pass, never by the terrain mesher.
        let r = shipped_for_tests();
        let wood = r.resolve_str("wood", ENGINE_NAMESPACE).expect("wood is registered");
        assert!(r.render_delegated_mask()[wood.0 as usize], "wood must be delegated");
        for name in ["granite", "soil", "grass_soil", "sand", "gravel"] {
            if let Ok(id) = r.resolve_str(name, ENGINE_NAMESPACE) {
                assert!(
                    !r.render_delegated_mask()[id.0 as usize],
                    "{name} must be drawn by the mesher",
                );
            }
        }
    }

    #[test]
    fn names_are_qualified_with_the_load_namespace() {
        let r = MaterialRegistry::from_ron_str(
            include_str!("../../../packages/sandbox/materials.ron"),
            "testbed",
        )
            .unwrap();
        let granite = r.resolve_str("granite", "testbed").expect("bare name in its namespace");
        assert_eq!(r.resolve_str("testbed:granite", ENGINE_NAMESPACE), Ok(granite));
        assert_eq!(r.get(granite).unwrap().name.to_string(), "testbed:granite");
        let err = r.resolve_str("granite", ENGINE_NAMESPACE).expect_err("other namespace");
        assert!(err.contains("voxulacrum:granite"), "{err}");
    }

    /// Each `rejects_` test asserts the *validation* error, not just any error:
    /// these fixtures once wrote `color` as `[..]`, which RON refuses to parse,
    /// so every one of them passed without reaching the check it names.
    fn validation_error(ron: &str) -> String {
        match MaterialRegistry::from_ron_str(ron, ENGINE_NAMESPACE) {
            Err(VoxelCoreError::MaterialRonValidation(msg)) => msg,
            Err(other) => panic!("expected a validation error, got: {other}"),
            Ok(_) => panic!("expected a validation error, the table loaded"),
        }
    }

    #[test]
    fn rejects_noncontiguous_ids() {
        let ron = r#"(materials: [
            (id: 0, id_name: "a", display_name: "A", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 2, id_name: "b", display_name: "B", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(validation_error(ron).contains("contiguous"));
    }

    #[test]
    fn rejects_duplicate_id() {
        let ron = r#"(materials: [
            (id: 0, id_name: "a", display_name: "A", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 0, id_name: "b", display_name: "B", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(validation_error(ron).contains("contiguous"));
    }

    #[test]
    fn rejects_duplicate_id_name() {
        let ron = r#"(materials: [
            (id: 0, id_name: "dup", display_name: "A", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 1, id_name: "dup", display_name: "B", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(validation_error(ron).contains("duplicate"));
    }

    #[test]
    fn rejects_an_id_name_that_is_not_a_name() {
        let ron = r#"(materials: [
            (id: 0, id_name: "Bad Name", display_name: "A", color: (0.0, 0.0, 0.0), sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(validation_error(ron).contains("Bad Name"));
    }
}
