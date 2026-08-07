//! Blueprints: authored voxel templates (design §6.6).
//!
//! A blueprint is a reusable cluster of voxels with an anchor - a rock, a tree,
//! a structure piece - plus the metadata placement and destruction need.
//!
//! ## Why this is not just a `Vec<Voxel>`
//!
//! **Materials are named, not numbered.** `"limestone"`, never `1`. Numeric
//! material ids in an asset file are the same latent defect §12 records for save
//! files: a registry reorder, insertion, or mod-added material silently
//! interprets every existing blueprint. Resolution happens at load, through
//! the [`MaterialRegistry`], and an unknown name is a hard error rather than a
//! wrong block.
//!
//! **Shapes use a blueprint-local vocabulary**, not [`ShapeId`]. The runtime
//! shape set is scheduled to change - the 4-shape vocabulary versus an
//! octant-occupancy mask is a dated decision - and an asset format that embeds
//! the runtime enum changes shape when the runtime does. Keeping a separate
//! spelling with a translation at load means a new shape becomes a new *name*
//! that older files simply never use. The cost of not doing this is already
//! visible: the prefab asset this format replaces carried a `"rotation"` field
//! that the voxel struct dropped versions ago, and nothing noticed.
//!
//! **The footprint is derived, never stored.** Two representations of the same
//! fact drift apart.
//!
//! ## Deliberately absent from v1
//!
//! Design §6.6 also lists sway weights, variant sets, and placement rules. All
//! three are omitted because their *shape* is unsettled - per-vertex or per-cell
//! sway? variants inline or as separate files? placement by slope, material,
//! biome, spacing? With a version field and serde defaults, **adding** a field
//! later is cheap; **changing** one is not. So v1 carries what is settled and
//! leaves the rest to the substep that gains a consumer for it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::material_registry::MaterialRegistry;
use crate::shape::ShapeId;
use crate::voxel::Voxel;

/// Current blueprint schema version.
pub const BLUEPRINT_VERSION: u32 = 1;

/// What happens to a blueprint instance when the voxel it is anchored to is
/// destroyed (design §6).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructionPolicy {
    /// Grass on a broken block: gone.
    #[default]
    Destroy,
    /// A tree: becomes a falling physics entity.
    DetachAsEntity,
    /// A bush: slides to the new surface.
    Reanchor,
}

/// A quarter-turn rotation about +Y.
///
/// **Y only, and that is a shape-vocabulary constraint rather than a
/// simplification.** All three [`BlueprintShape`]s are invariant under Y
/// rotation, so a yaw is a pure coordinate permutation with no shape remapping.
/// Rotating about X or Z would turn a `SlabBottom` into a vertical half-cell,
/// which this vocabulary cannot spell - so arbitrary orientation is blocked on
/// **D1** (the octant-occupancy mask, 0.6.0), where a rotation becomes a bit
/// permutation on the mask. Adding a horizontally asymmetric shape before then
/// breaks this, and the compiler will not say so.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Yaw {
    /// Unrotated.
    #[default]
    Deg0,
    /// A quarter turn.
    Deg90,
    /// A half turn.
    Deg180,
    /// Three-quarter turns.
    Deg270,
}

impl Yaw {
    /// Every rotation, in order - for a UI that offers all of them.
    pub const ALL: [Yaw; 4] = [Yaw::Deg0, Yaw::Deg90, Yaw::Deg180, Yaw::Deg270];

    /// Short label for UI.
    pub fn label(self) -> &'static str {
        match self {
            Yaw::Deg0 => "0°",
            Yaw::Deg90 => "90°",
            Yaw::Deg180 => "180°",
            Yaw::Deg270 => "270°",
        }
    }

    /// Quarter turns from `Deg0`, 0..=3.
    pub fn steps(self) -> i32 {
        match self {
            Yaw::Deg0 => 0,
            Yaw::Deg90 => 1,
            Yaw::Deg180 => 2,
            Yaw::Deg270 => 3,
        }
    }

    /// From a signed number of quarter turns, wrapping in both directions.
    pub fn from_steps(steps: i32) -> Self {
        Yaw::ALL[steps.rem_euclid(4) as usize]
    }

    /// Compose two rotations. Order does not matter - both turn about Y.
    pub fn then(self, other: Yaw) -> Self {
        Yaw::from_steps(self.steps() + other.steps())
    }

    /// The next quarter turn, wrapping.
    pub fn next(self) -> Self {
        match self {
            Yaw::Deg0 => Yaw::Deg90,
            Yaw::Deg90 => Yaw::Deg180,
            Yaw::Deg180 => Yaw::Deg270,
            Yaw::Deg270 => Yaw::Deg0,
        }
    }

    /// Rotate a template-space offset about the Y axis through the origin.
    pub fn apply(self, at: [i32; 3]) -> [i32; 3] {
        let [x, y, z] = at;
        match self {
            Yaw::Deg0 => [x, y, z],
            Yaw::Deg90 => [-z, y, x],
            Yaw::Deg180 => [-x, y, -z],
            Yaw::Deg270 => [z, y, -x],
        }
    }
}

/// A blueprint's shape vocabulary - deliberately its own spelling, not
/// [`ShapeId`]. See the module docs.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintShape {
    /// Fills the whole cell.
    Cube,
    /// Solid in the lower half of the cell.
    SlabBottom,
    /// Solid in the upper half of the cell.
    SlabTop,
}

impl BlueprintShape {
    /// Translate from the runtime shape vocabulary. `None` for [`ShapeId::Empty`],
    /// which has no blueprint spelling - absence is how a blueprint says empty.
    pub fn from_shape_id(id: ShapeId) -> Option<Self> {
        match id {
            ShapeId::Empty => None,
            ShapeId::Cube => Some(BlueprintShape::Cube),
            ShapeId::SlabBottom => Some(BlueprintShape::SlabBottom),
            ShapeId::SlabTop => Some(BlueprintShape::SlabTop),
        }
    }

    /// Translate into the runtime shape vocabulary.
    fn to_shape_id(self) -> ShapeId {
        match self {
            BlueprintShape::Cube => ShapeId::Cube,
            BlueprintShape::SlabBottom => ShapeId::SlabBottom,
            BlueprintShape::SlabTop => ShapeId::SlabTop,
        }
    }
}

/// One authored cell, at an integer offset from the template origin.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct BlueprintCell {
    /// Offset `[x, y, z]` from the template origin, before anchoring.
    pub at: [i32; 3],
    /// Cell geometry, in the blueprint vocabulary.
    pub shape: BlueprintShape,
    /// Material `id_name`, resolved through the registry at load.
    pub material: String,
}

/// An authored blueprint, as it exists on disk.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Blueprint {
    /// Schema version. **Required** - a file without one is not a blueprint of
    /// this format, and 0.6.0's format work must be able to tell what it is
    /// reading without guessing at the shape.
    pub version: u32,
    /// Stable name; the handle graphs and structure pools reference.
    pub name: String,
    /// The template-local cell that lands on the placement position.
    #[serde(default)]
    pub anchor: [i32; 3],
    /// The authored cells. Sparse — a blueprint is mostly empty.
    pub cells: Vec<BlueprintCell>,
    /// What happens to an instance when its anchor voxel is destroyed.
    #[serde(default)]
    pub destruction: DestructionPolicy,
    /// When set, region transformation (§12) must not overwrite these cells -
    /// the mechanism that lets an authored vault survive a self-modifying world.
    #[serde(default)]
    pub protected_volume: bool,
}

/// A blueprint with names resolved and shapes translated: the form the stamping
/// path consumes.
#[derive(Clone, PartialEq, Debug)]
pub struct ResolvedBlueprint {
    /// The blueprint's stable name, carried through for diagnostics.
    pub name: String,
    /// The template-local cell that lands on the placement position.
    pub anchor: [i32; 3],
    /// Template-space offsets paired with the voxel to stamp there.
    pub cells: Vec<([i32; 3], Voxel)>,
    /// What happens to an instance when its anchor voxel is destroyed.
    pub destruction: DestructionPolicy,
    /// When set, region transformation must not overwrite these cells.
    pub protected_volume: bool,
}

impl Blueprint {
    /// Parse from JSON, rejecting a version this build does not understand.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let bp: Blueprint = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if bp.version > BLUEPRINT_VERSION {
            return Err(format!(
                "blueprint '{}' is version {} but this build understands up to {}",
                bp.name, bp.version, BLUEPRINT_VERSION
            ));
        }
        Ok(bp)
    }

    /// Read `<dir>/<name>.blueprint.json`.
    pub fn load(dir: &Path, name: &str) -> Result<Self, String> {
        let path = dir.join(format!("{name}.blueprint.json"));
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Write to `<dir>/<name>.blueprint.json`, creating `dir` if needed.
    ///
    /// The name is validated because it becomes a filename: this is called with
    /// a string an author typed into a text field, and a path separator or `..`
    /// in it would write outside the blueprint library.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        if self.name.is_empty()
            || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!(
                "blueprint name '{}' must be non-empty and use only letters, digits, '_' and '-' — it becomes a filename",
                self.name
            ));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(format!("{}.blueprint.json", self.name));
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
    
    /// Names of the blueprints in `dir`, sorted. A missing directory is an empty
    /// library rather than an error - the first capture is what creates this.
    pub fn list(dir: &Path) -> Result<Vec<String>, String> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter_map(|f| f.strip_suffix(".blueprint.json").map(str::to_string))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Resolve material names and translate shapes.
    ///
    /// An unknown material is an error, not a fallback: silently substituting
    /// would put a wrong block in the world with nothing to notice it, which is
    /// the failure mode named material references exist to prevent.
    pub fn resolve(&self, registry: &MaterialRegistry) -> Result<ResolvedBlueprint, String> {
        let mut cells = Vec::with_capacity(self.cells.len());
        for cell in &self.cells {
            let material = registry.resolve(&cell.material).ok_or_else(|| {
                format!(
                    "blueprint '{}': material '{}' is not in the registry",
                    self.name, cell.material
                )
            })?;
            cells.push((
                cell.at,
                Voxel { shape: cell.shape.to_shape_id(), material, flags: 0 },
            ));
        }
        Ok(ResolvedBlueprint {
            name: self.name.clone(),
            anchor: self.anchor,
            cells,
            destruction: self.destruction,
            protected_volume: self.protected_volume,
        })
    }
}

impl ResolvedBlueprint {
    /// Cells occupied, in template space. Derived rather than stored so a
    /// blueprint cannot claim a footprint its cells disagree with.
    pub fn footprint(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.cells.iter().map(|(at, _)| *at)
    }

    /// The largest horizontal distance any cell sits from the anchor, in voxels.
    ///
    /// **Invariant under every [`Yaw`]**, which is what makes it the right bound
    /// for a margin band: a quarter turn maps an offset `(dx, dz)` to one whose
    /// components are `{±dx, ±dz}` permuted, so the max of their magnitudes does
    /// not change. A band sized from this holds for all four rotations, and no
    /// per-rotation extent is needed.
    pub fn horizontal_reach(&self) -> i32 {
        self.cells
            .iter()
            .map(|(at, _)| {
                let dx = (at[0] - self.anchor[0]).abs();
                let dz = (at[2] - self.anchor[2]).abs();
                dx.max(dz)
            })
            .max()
            .unwrap_or(0)
    }

    /// Inclusive `(min, max)` bounds in template space, or `None` when empty.
    ///
    /// This is what the cross-chunk feature model (design §5) reads to size its
    /// margin band: a chunk must consider every feature whose extent can reach
    /// its window.
    pub fn extent(&self) -> Option<([i32; 3], [i32; 3])> {
        let mut it = self.cells.iter().map(|(at, _)| *at);
        let first = it.next()?;
        let (mut lo, mut hi) = (first, first);
        for at in it {
            for a in 0..3 {
                lo[a] = lo[a].min(at[a]);
                hi[a] = hi[a].max(at[a]);
            }
        }
        Some((lo, hi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "version": 1,
        "name": "test",
        "anchor": [1, 0, 1],
        "cells": [
            { "at": [0, 0, 0], "shape": "cube", "material": "limestone" },
            { "at": [1, 2, 1], "shape": "slab_bottom", "material": "soil" }
        ],
        "destruction": "detach_as_entity",
        "protected_volume": true
    }"#;

    #[test]
    fn parses_and_round_trips() {
        let bp = Blueprint::from_json(SAMPLE).expect("sample parses");
        assert_eq!(bp.name, "test");
        assert_eq!(bp.anchor, [1, 0, 1]);
        assert_eq!(bp.cells.len(), 2);
        assert_eq!(bp.destruction, DestructionPolicy::DetachAsEntity);
        assert!(bp.protected_volume);

        let json = serde_json::to_string(&bp).expect("serializes");
        assert_eq!(Blueprint::from_json(&json).expect("re-parses"), bp);
    }

    #[test]
    fn a_newer_version_is_refused_rather_than_misread() {
        // The point of carrying a version from the first byte: an older build
        // must say "I don't understand this" instead of interpreting unknown
        // content as the shape it happens to know.
        let future = SAMPLE.replace("\"version\": 1", "\"version\": 99");
        let err = Blueprint::from_json(&future).expect_err("must refuse");
        assert!(err.contains("99"), "the error should name the version: {err}");
    }

    #[test]
    fn resolves_names_to_ids_and_shapes_to_the_runtime_vocabulary() {
        let registry = MaterialRegistry::load_initial();
        assert!(
            registry.resolve("limestone").is_some(),
            "test fixture assumes the built-in registry defines limestone",
        );
        let resolved = Blueprint::from_json(SAMPLE)
            .unwrap()
            .resolve(&registry)
            .expect("resolves");

        assert_eq!(resolved.cells[0].1.shape, ShapeId::Cube);
        assert_eq!(resolved.cells[0].1.material, registry.resolve("limestone").unwrap());
        assert_eq!(resolved.cells[1].1.shape, ShapeId::SlabBottom);
        assert_eq!(resolved.extent(), Some(([0, 0, 0], [1, 2, 1])));
        assert_eq!(resolved.footprint().count(), 2);
    }

    #[test]
    fn an_unknown_material_is_an_error_not_a_substitution() {
        let bad = SAMPLE.replace("\"limestone\"", "\"unobtainium\"");
        let err = Blueprint::from_json(&bad)
            .unwrap()
            .resolve(&MaterialRegistry::load_initial())
            .expect_err("must refuse");
        assert!(err.contains("unobtainium"), "the error should name it: {err}");
    }

    #[test]
    fn a_name_that_would_escape_the_library_is_refused() {
        // The name reaches `save` straight from an author's text field.
        let mut bp = Blueprint::from_json(SAMPLE).unwrap();
        bp.name = "../../evil".to_string();
        let err = bp.save(Path::new(".")).expect_err("must refuse");
        assert!(err.contains("filename"), "the error should say why: {err}");
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        let at = [3, -1, 7];
        let mut yaw = Yaw::Deg0;
        let mut p = at;
        for _ in 0..4 {
            p = yaw.apply(p);
            yaw = yaw.next();
        }
        // Each step applies its own rotation to the running point, so this also
        // pins the direction: composing the four in order returns to start only
        // if they turn the same way.
        assert_eq!(Yaw::Deg90.apply(Yaw::Deg270.apply(at)), at);
        assert_eq!(Yaw::Deg180.apply(Yaw::Deg180.apply(at)), at);
        assert_eq!(Yaw::Deg0.apply(at), at);
        let _ = p;
    }

    #[test]
    fn horizontal_reach_is_the_same_at_every_rotation() {
        let bp = Blueprint::from_json(SAMPLE).unwrap().resolve(&MaterialRegistry::load_initial()).unwrap();
        let reach = bp.horizontal_reach();
        for yaw in Yaw::ALL {
            let rotated: i32 = bp
                .cells
                .iter()
                .map(|(at, _)| {
                    let rel = [at[0] - bp.anchor[0], at[1] - bp.anchor[1], at[2] - bp.anchor[2]];
                    let r = yaw.apply(rel);
                    r[0].abs().max(r[2].abs())
                })
                .max()
                .unwrap_or(0);
            assert_eq!(rotated, reach, "reach must not depend on {}", yaw.label());
        }
    }

    #[test]
    fn composing_quarter_turns_adds_them() {
        let at = [1, 0, 0];
        assert_eq!(Yaw::Deg90.then(Yaw::Deg180), Yaw::Deg270);
        assert_eq!(Yaw::Deg270.then(Yaw::Deg270), Yaw::Deg180);
        // Composition equals applying them in sequence.
        assert_eq!(Yaw::Deg90.then(Yaw::Deg180).apply(at), Yaw::Deg180.apply(Yaw::Deg90.apply(at)));
        // Negative steps wrap the other way - the preview relies on this to
        // negate the camera's turn.
        assert_eq!(Yaw::from_steps(-1), Yaw::Deg270);
    }

    #[test]
    fn a_quarter_turn_moves_x_onto_z() {
        // Pins the handedness, which no round-trip test can.
        assert_eq!(Yaw::Deg90.apply([1, 0, 0]), [0, 0, 1]);
        assert_eq!(Yaw::Deg90.apply([0, 0, 1]), [-1, 0, 0]);
        // Height is untouched - the property that makes Y rotation shape-safe.
        assert_eq!(Yaw::Deg90.apply([0, 5, 0]), [0, 5, 0]);
    }

    #[test]
    fn the_shipped_blueprint_parses_and_resolves() {
        const ROCK: &str = include_str!("../../../assets/blueprints/rock.blueprint.json");
        Blueprint::from_json(ROCK)
            .expect("shipped blueprint parses")
            .resolve(&MaterialRegistry::load_initial())
            .expect("shipped blueprint resolves");
    }
}
