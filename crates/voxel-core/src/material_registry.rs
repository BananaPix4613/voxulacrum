//! Stable material registry: numeric [`MaterialId`]s plus snake_case name
//! resolution. The table is data-driven — authored in `assets/materials.ron`
//! and loaded via [`MaterialRegistry::load_from_ron`] — with
//! [`MaterialRegistry::load_initial`] kept as a built-in fallback.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{VoxelCoreError, VoxelCoreResult};
use crate::material::MaterialId;

/// Authored definition of one material.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    /// Stable snake_case id used by graphs and [`MaterialRegistry::resolve`].
    pub id_name: String,
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
    by_id_name: HashMap<String, MaterialId>,
    /// `render_delegated` per id, indexed by [`MaterialId`]. Built once here so
    /// the mesher can test a material with one slice index rather than a
    /// registry lookup per voxel - a 34^3 snapshot is 39,304 of them, up to 32
    /// times a frame.
    delegated: Vec<bool>,
}

impl MaterialRegistry {
    /// Hand-authored Phase 0 registry (moved from the engine's old
    /// `MATERIAL_TABLE`). Panics if two entries share an `id_name`.
    pub fn load_initial() -> Self {
        let entries = vec![
            MaterialDef { id_name: "air".into(), display_name: "Air".into(),
                color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "limestone".into(), display_name: "Limestone".into(),
                color: [0.95, 0.90, 0.82], sharpness: 0.9, hardness: 0.9, permeable: false, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "granite".into(), display_name: "Granite".into(),
                color: [0.50, 0.50, 0.53], sharpness: 0.85, hardness: 0.95, permeable: false, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "soil".into(), display_name: "Soil".into(),
                color: [0.40, 0.22, 0.10], sharpness: 0.3, hardness: 0.2, permeable: false, supports_flora: true, render_delegated: false },
            MaterialDef { id_name: "clay".into(), display_name: "Clay".into(),
                color: [0.62, 0.36, 0.20], sharpness: 0.4, hardness: 0.4, permeable: false, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "sand".into(), display_name: "Sand".into(),
                color: [0.90, 0.82, 0.55], sharpness: 0.15, hardness: 0.1, permeable: true, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "grass_soil".into(), display_name: "Grass Soil".into(),
                color: [0.30, 0.55, 0.18], sharpness: 0.35, hardness: 0.2, permeable: false, supports_flora: true, render_delegated: false },
            MaterialDef { id_name: "water".into(), display_name: "Water".into(),
                color: [0.2, 0.35, 0.6], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "gravel".into(), display_name: "Gravel".into(),
                color: [0.52, 0.49, 0.45], sharpness: 0.6, hardness: 0.5, permeable: true, supports_flora: false, render_delegated: false },
            MaterialDef { id_name: "wood".into(), display_name: "Wood".into(),
                color: [0.55, 0.44, 0.30], sharpness: 0.5, hardness: 0.6, permeable: false, supports_flora: false, render_delegated: true },
            MaterialDef { id_name: "leaves".into(), display_name: "Leaves".into(),
                color: [0.24, 0.46, 0.18], sharpness: 0.2, hardness: 0.1, permeable: true, supports_flora: false, render_delegated: false },
        ];
        Self::from_entries(entries)
    }

    fn from_entries(entries: Vec<MaterialDef>) -> Self {
        let mut by_id_name = HashMap::with_capacity(entries.len());
        for (i, def) in entries.iter().enumerate() {
            let id = MaterialId(i as u16);
            if by_id_name.insert(def.id_name.clone(), id).is_some() {
                panic!("duplicate material id_name: {:?}", def.id_name);
            }
        }
        let delegated = entries.iter().map(|d| d.render_delegated).collect();
        Self { entries, by_id_name, delegated }
    }

    /// `render_delegated` per material id, for the snapshot extractor.
    ///
    /// Derived from `entries` rather than stored alongside them, so there is one
    /// copy of the fact, and it cannot drift from the table it describes.
    pub fn render_delegated_mask(&self) -> &[bool] {
        &self.delegated
    }

    /// Resolve a snake_case `id_name` to its stable numeric id.
    pub fn resolve(&self, id_name: &str) -> Option<MaterialId> {
        self.by_id_name.get(id_name).copied()
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

    /// Load the material table from a RON file (`assets/materials.ron`).
    ///
    /// Each material declares an explicit stable numeric `id`. The loader
    /// validates that ids are unique and form a contiguous range `0..len`, and
    /// that `id_name's are unique, then orders the table by id so the positional
    /// [`MaterialId`] equals the declared id.
    pub fn load_from_ron(path: &Path) -> VoxelCoreResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            VoxelCoreError::MaterialRonRead { path: path.display().to_string(), source: e }
        })?;
        Self::from_ron_str(&text)
    }

    /// Parse + validate a RON document already in memory. Factored out of
    /// [`Self::load_from_ron`] so validation is unit-testable without the
    /// filesystem.
    #[cfg(feature = "serde")]
    pub(crate) fn from_ron_str(text: &str) -> VoxelCoreResult<Self> {
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

        // id_names must be unique.
        let mut seen = std::collections::HashSet::with_capacity(entries.len());
        for e in &entries {
            if !seen.insert(e.id_name.as_str()) {
                return Err(VoxelCoreError::MaterialRonValidation(format!(
                    "duplicate material id_name: {:?}",
                    e.id_name
                )));
            }
        }

        let defs = entries
            .into_iter()
            .map(|e| MaterialDef {
                id_name: e.id_name,
                display_name: e.display_name,
                color: e.color,
                sharpness: e.sharpness,
                hardness: e.hardness,
                permeable: e.permeable,
                supports_flora: e.supports_flora,
                render_delegated: e.render_delegated,
            })
            .collect();

        Ok(Self::from_entries(defs))
    }

    /// Stub for builds without the `serde` feature (RON loading needs serde
    /// derives). The engine always builds with `serde` (the default feature).
    #[cfg(not(feature = "serde"))]
    pub fn load_from_ron(_path: &Path) -> VoxelCoreResult<Self> {
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

#[cfg(all(test, feature = "serde"))]
mod ron_tests {
    use super::*;

    /// `assets/materials.ron` lives at the workspace root; this crate dir is
    /// `<workspace>/crates/voxel-core`.
    fn materials_ron_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..").join("..").join("assets").join("materials.ron")
    }

    #[test]
    fn ron_matches_load_initial() {
        let from_ron = MaterialRegistry::load_from_ron(&materials_ron_path())
            .expect("assets/materials.ron must load");
        let initial = MaterialRegistry::load_initial();
        assert_eq!(
            from_ron.entries, initial.entries,
            "materials.ron drifted from load_initial(); they must stay byte-equivalent"
        );
    }

    #[test]
    fn the_delegated_mask_tracks_the_table() {
        let r = MaterialRegistry::load_initial();
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
        let r = MaterialRegistry::load_initial();
        let wood = r.resolve("wood").expect("wood is registered");
        assert!(r.render_delegated_mask()[wood.0 as usize], "wood must be delegated");
        for name in ["granite", "soil", "grass_soil", "sand", "gravel"] {
            if let Some(id) = r.resolve(name) {
                assert!(
                    !r.render_delegated_mask()[id.0 as usize],
                    "{name} must be drawn by the mesher",
                );
            }
        }
    }

    #[test]
    fn rejects_noncontiguous_ids() {
        let ron = r#"(materials: [
            (id: 0, id_name: "a", display_name: "A", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 2, id_name: "b", display_name: "B", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(MaterialRegistry::from_ron_str(ron).is_err());
    }

    #[test]
    fn rejects_duplicate_id() {
        let ron = r#"(materials: [
            (id: 0, id_name: "a", display_name: "A", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 0, id_name: "b", display_name: "B", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(MaterialRegistry::from_ron_str(ron).is_err());
    }

    #[test]
    fn rejects_duplicate_id_name() {
        let ron = r#"(materials: [
            (id: 0, id_name: "dup", display_name: "A", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
            (id: 1, id_name: "dup", display_name: "B", color: [0.0, 0.0, 0.0], sharpness: 0.0, hardness: 0.0, permeable: true, supports_flora: false),
        ])"#;
        assert!(MaterialRegistry::from_ron_str(ron).is_err());
    }
}
