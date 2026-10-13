//! The embedded editor plus the multi-graph hierarchy it edits.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use std::sync::Arc;

use nodegraph_editor::{EditorState, GraphChoice, GraphCatalogs, LibraryChoice};
use nodegraph_ir::{Graph, GraphRefTarget, NodeId};

use crate::libraries::LoadedLibraries;

use crate::world::world_generator::{
    self, BiomeManifestEntry, GraphSlot, WorldManifest, ZoneManifestEntry,
};

/// The active [`EditorState`] plus the world's graph hierarchy: an in-memory
/// graph set keyed by [`GraphSlot`], a selector, and the slot currently shown.
/// Switching slots saves the current canvas back into the set first, so unsaved
/// edits survive the switch.
pub struct HierarchyEditor {
    editor: EditorState,
    /// Selector entries `(slot, display label)`, in display order.
    slots: Vec<(GraphSlot, String)>,
    /// The slot currently shown on the canvas.
    selected: GraphSlot,
    /// In-memory graph per slot.
    set: HashMap<GraphSlot, Graph>,
    /// Slots whose in-memory graph differs from its file.
    dirty: HashSet<GraphSlot>,
    /// On-disk path per slot, for saving.
    paths: HashMap<GraphSlot, PathBuf>,
    /// The world manifest path, for adding/removing biome entries.
    manifest_path: Option<PathBuf>,
    /// The manifest itself, held so its fields are editable and so this editor
    /// is its single writer. A form that edited a copy while `create_biome` did
    /// its own read-modify-write on disk would let the two diverge, and the
    /// loser would be whichever wrote last.
    manifest: Option<WorldManifest>,
    /// Whether `manifest` differs from its file.
    manifest_dirty: bool,
    /// Name entry for the "New biome" action.
    new_name: String,
    /// Name entry for the "New zone" action.
    new_zone_name: String,
    /// Name entry for the "Rename" action.
    new_rename: String,
    /// The library set, for the reference-node menus. Loaded from the same
    /// place the generator loads it, so the two cannot disagree about which
    /// libraries exist.
    libraries: Option<Arc<LoadedLibraries>>,
    /// Name entry for the "add biome parameter" action.
    new_param: String,
    /// Transient status line under the toolbar.
    status: Option<String>,
}

impl HierarchyEditor {
    /// Empty editor (no graphs). Populate via [`HierarchyEditor::load`]
    pub fn new() -> Self {
        Self {
            editor: EditorState::new(),
            slots: Vec::new(),
            selected: GraphSlot::Biome(0),
            set: HashMap::new(),
            dirty: HashSet::new(),
            paths: HashMap::new(),
            manifest_path: None,
            manifest: None,
            manifest_dirty: false,
            new_name: String::new(),
            new_zone_name: String::new(),
            new_rename: String::new(),
            libraries: None,
            new_param: String::new(),
            status: None,
        }
    }

    /// Replace the hierarchy with a manifest-loaded graph set, opening on the
    /// primary biome (biome 0) if present, else the first entry.
    pub fn load(
        &mut self,
        manifest_path: PathBuf,
        graphs: Vec<(GraphSlot, String, PathBuf, Graph)>
    ) {
        self.slots = graphs.iter().map(|(s, l, _, _)| (*s, l.clone())).collect();
        self.paths = graphs.iter().map(|(s, _, p, _)| (*s, p.clone())).collect();
        self.manifest = WorldManifest::read(&manifest_path).ok();
        self.manifest_dirty = false;
        self.manifest_path = Some(manifest_path);
        self.libraries = Some(crate::libraries::load_libraries());
        let default = GraphSlot::Biome(0);
        self.selected = if graphs.iter().any(|(s, _, _, _)| *s == default) {
            default
        } else {
            graphs.first().map(|(s, _, _, _)| *s).unwrap_or(default)
        };
        self.set = graphs.into_iter().map(|(s, _, _, g)| (s, g)).collect();
        self.dirty.clear();
        if let Some(g) = self.set.get(&self.selected) {
            self.editor = EditorState::from_graph(g);
        }
    }
    
    /// Switch the active graph to `slot`, saving the current canvas first so
    /// unsaved edits survive.
    pub fn select(&mut self, slot: GraphSlot) {
        if slot == self.selected {
            return;
        }
        // Record before the canvas is rebuilt - rebuilding clears the flag.
        if self.editor.is_modified() {
            self.dirty.insert(self.selected);
        }
        self.set.insert(self.selected, self.editor.build_graph());
        if let Some(g) = self.set.get(&slot) {
            self.editor = EditorState::from_graph(g);
            self.selected = slot;
        }
    }
    
    /// Draw the selector dropdown above the canvas, then the canvas itself.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.selected;
        let current = self.active_label();
        egui::ComboBox::from_label("Graph")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (slot, label) in &self.slots {
                    // Via slot_is_modified, not `dirty` directly: the selected
                    // slot's unsaved state lives on the live canvas, and only
                    // moves into `dirty` when you switch away from it.
                    let marked = if self.slot_is_modified(*slot) {
                        format!("● {label}")
                    } else {
                        label.clone()
                    };
                    ui.selectable_value(&mut chosen, *slot, marked);
                }
            });
        if chosen != self.selected {
            self.select(chosen);
        }
        ui.separator();
        // Both catalogs come from the slot list, which *is* the manifest as
        // loaded - so a band picker can only offer a zone or biome that has a
        // graph. Detail slots are deliberately in neither.
        let libraries: Vec<LibraryChoice> = self
            .libraries
            .as_ref()
            .map(|libs| {
                libs.entries()
                    .iter()
                    .filter_map(|(id, name)| {
                        libs.registry().get(*id).map(|g| LibraryChoice {
                            id: *id,
                            name: name.clone(),
                            boundary: g.boundary.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // World only: `GraphRefTarget::Zone` is still a unit variant (ambiguous
        // once several zones exist) and `Biome(_)` resolves to nothing in either
        // resolution path. Offering a target that produces a broken node is
        // worse than offering one that works. Grows when `Zone(id)` lands.
        let graphs: Vec<GraphChoice> = self
            .set
            .get(&GraphSlot::World)
            .map(|w| {
                vec![GraphChoice {
                    target: GraphRefTarget::World,
                    name: "World".to_string(),
                    boundary: w.boundary.clone(),
                }]
            })
            .unwrap_or_default();
        let catalogs = GraphCatalogs {
            libraries,
            graphs,
            // Read per frame rather than cached: a listing of a handful of files
            // behind a panel that is hidden by default, versus a cache whose
            // invalidation edge comes from a different panel (capture happens in
            // the blueprint panel). Cache it if it ever appears in the `params`
            // span, with the measurement in hand.
            blueprints: voxel_core::Blueprint::list(&crate::world::blueprint::blueprint_dir())
                .unwrap_or_default(),
            zones: self
                .slots
                .iter()
                .filter_map(|(s, l)| match s {
                    GraphSlot::Zone(id) => Some((*id, l.clone())),
                    _ => None,
                })
                .collect(),
            biomes: self
                .slots
                .iter()
                .filter_map(|(s, l)| match s {
                    GraphSlot::Biome(id) => Some((*id, l.clone())),
                    _ => None,
                })
                .collect(),
        };
        self.editor.show(ui, &catalogs);
    }
    
    /// The slot currently shown.
    #[allow(dead_code)] // selection accessor
    pub fn selected(&self) -> GraphSlot {
        self.selected
    }

    /// True if there is anything to save: unsaved canvas edits (live or carried
    /// over from before a slot switch), or pending manifest changes.
    pub fn is_modified(&self) -> bool {
        self.graph_is_modified() || self.manifest_dirty
    }

    /// Unsaved edits to the *active graph* specifically.
    fn graph_is_modified(&self) -> bool {
        self.editor.is_modified() || self.dirty.contains(&self.selected)
    }

    /// Whether `slot` has unsaved edits, for marking the selector.
    pub fn slot_is_modified(&self, slot: GraphSlot) -> bool {
        if slot == self.selected {
            // Graph-only: a pending manifest edit is not this slot's dot.
            self.graph_is_modified()
        } else {
            self.dirty.contains(&slot)
        }
    }

    /// Write whatever is pending: the manifest if its fields changed, and the
    /// active graph if its canvas did. One button, because "what did Save just
    /// write" should not depend on which widget you touched last.
    pub fn save_active(&mut self) {
        let mut saved: Vec<String> = Vec::new();

        if self.manifest_dirty {
            match self.write_manifest() {
                Ok(name) => saved.push(name),
                Err(e) => {
                    self.status = Some(format!("Save failed: {e}"));
                    return;
                }
            }
        }

        if self.graph_is_modified() {
            let slot = self.selected;
            let graph = self.editor.build_graph();
            let Some(path) = self.paths.get(&slot).cloned() else {
                self.status = Some("No file path for this graph".into());
                return;
            };
            match graph.to_json_pretty().map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()))
            {
                Ok(()) => {
                    self.set.insert(slot, graph);
                    self.editor.mark_saved();
                    self.dirty.remove(&slot);
                    saved.push(path.display().to_string());
                }
                Err(e) => {
                    self.status = Some(format!("Save failed: {e}"));
                    return;
                }
            }
        }

        if !saved.is_empty() {
            self.status = Some(format!("Saved {}", saved.join(", ")));
        }
    }

    /// Write the in-memory manifest to disk and clear its dirty flag.
    fn write_manifest(&mut self) -> Result<String, String> {
        let (Some(path), Some(manifest)) = (&self.manifest_path, &self.manifest) else {
            return Err("no manifest loaded".into());
        };
        manifest.write(path)?;
        self.manifest_dirty = false;
        Ok(path.display().to_string())
    }

    /// The manifest, its dirty flag, and the "add parameter" name buffer, for
    /// the world-settings form. Three disjoint fields, handed out together
    /// because egui widgets need `&mut` at the value and the form must be able
    /// to record that it changed something.
    pub fn manifest_form(&mut self) -> Option<(&mut WorldManifest, &mut bool, &mut String)> {
        let manifest = self.manifest.as_mut()?;
        Some((manifest, &mut self.manifest_dirty, &mut self.new_param))
    }

    /// The biome the selection belongs to - a biome slot, or its detail slot.
    pub fn selected_biome_id(&self) -> Option<u16> {
        match self.selected {
            GraphSlot::Biome(id) | GraphSlot::Detail(id) => Some(id),
            _ => None,
        }
    }


    /// The world seed, as the loaded manifest declares it.
    ///
    /// The field probe evaluates the graph this editor is showing, so it must
    /// use the seed this editor knows about - not a copy kept elsewhere that
    /// could disagree after a manifest edit.
    pub fn world_seed(&self) -> u64 {
        self.manifest
            .as_ref()
            .map_or(world_generator::DEFAULT_SEED, |m| m.seed)
    }

    /// Refuse a structural change while anything is unsaved.
    ///
    /// A structural change rewrites the manifest and re-reads the whole
    /// hierarchy from disk, which would discard unsaved canvas edits. Refusing
    /// beats the alternatives: patching the slot list by hand duplicates
    /// `load_world_graphs`, and saving on the author's behalf writes files they
    /// did not ask to write.
    fn require_saved(&mut self, action: &str) -> bool {
        if self.is_modified() {
            self.status = Some(format!("Save before {action}"));
            return false;
        }
        true
    }

    /// Re-read the whole hierarchy from disk, then select `slot`.
    ///
    /// Startup and every structural change take this one path, so a slot that
    /// appears at launch cannot fail to appear after an edit - the bug class
    /// hand-maintained slot lists produce.
    fn reload_and_select(&mut self, slot: GraphSlot) {
        let Some(path) = self.manifest_path.clone() else {
            return;
        };
        match world_generator::load_world_graphs(&path) {
            Ok(graphs) => {
                self.load(path, graphs);
                self.select(slot);
            }
            Err(e) => self.status = Some(format!("Reload failed: {e}")),
        }
    }

    /// The next unused zone id (max existing + 1, min 1).
    fn next_zone_id(&self) -> u16 {
        self.slots
            .iter()
            .filter_map(|(s, _)| match s {
                GraphSlot::Zone(id) => Some(*id),
                _ => None,
            })
            .max()
            .map_or(1, |m| m + 1)
    }

    /// Write `graph` to `path`, failing if the file already exists.
    fn write_new_graph(&mut self, path: &PathBuf, graph: &Graph) -> bool {
        if path.exists() {
            self.status = Some(format!("{} already exists", path.display()));
            return false;
        }
        match graph
            .to_json_pretty()
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(path, json).map_err(|e| e.to_string()))
        {
            Ok(()) => true,
            Err(e) => {
                self.status = Some(format!("Create failed: {e}"));
                false
            }
        }
    }

    /// The next unused biome id (max existing + 1, min 1).
    fn next_biome_id(&self) -> u16 {
        self.slots
            .iter()
            .filter_map(|(s, _)| match s {
                GraphSlot::Biome(id) => Some(*id),
                _ => None,
            })
            .max()
            .map_or(1, |m| m + 1)
    }

    /// Create a new biome graph from the template, register it in the manifest,
    /// and switch to it. `name` seeds the file name and label.
    pub fn create_biome(&mut self, name: &str) {
        if !self.require_saved("adding a biome") {
            return;
        }
        let slug = slugify(name);
        if slug.is_empty() {
            self.status = Some("Enter a biome name first".into());
            return;
        }
        let id = self.next_biome_id();
        let file_name = format!("biome_{slug}.graph.json");
        let path = world_generator::graphs_dir_public().join(&file_name);
        let graph = world_generator::new_biome_graph();
        if !self.write_new_graph(&path, &graph) {
            return;
        }
        let Some(m) = self.manifest.as_mut() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        m.biomes.push(BiomeManifestEntry {
            id,
            graph: file_name,
            detail: None,
            params: HashMap::new(),
        });
        if let Err(e) = self.write_manifest() {
            self.status = Some(format!("Create failed: {e}"));
            return;
        }
        self.reload_and_select(GraphSlot::Biome(id));
        self.status = Some(format!("Created {}", path.display()));
    }

    /// Create a new zone graph from the template and register it in the
    /// manifest. The template reads the World graph's climate output, so a new
    /// zone participates in the hierarchy rather than re-deriving climate
    /// (design §4).
    pub fn create_zone(&mut self, name: &str) {
        if !self.require_saved("adding a zone") {
            return;
        }
        let slug = slugify(name);
        if slug.is_empty() {
            self.status = Some("Enter a zone name first".into());
            return;
        }
        let Some(world) = self.set.get(&GraphSlot::World) else {
            self.status = Some("No World graph loaded".into());
            return;
        };
        let graph = world_generator::new_zone_graph(&world.boundary);
        let id = self.next_zone_id();
        let file_name = format!("zone_{slug}.graph.json");
        let path = world_generator::graphs_dir_public().join(&file_name);
        if !self.write_new_graph(&path, &graph) {
            return;
        }
        let Some(m) = self.manifest.as_mut() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        m.zones.push(ZoneManifestEntry { id, graph: file_name });
        if let Err(e) = self.write_manifest() {
            self.status = Some(format!("Create failed: {e}"));
            return;
        }
        self.reload_and_select(GraphSlot::Zone(id));
        self.status = Some(format!(
            "Created zone {id}. Assign it a climate band on the World graph's \
             World Output node."
        ));
    }

    /// True when the selection belongs to a biome that has no detail graph.
    pub fn active_biome_lacks_detail(&self) -> bool {
        match self.selected_biome_id() {
            Some(id) => !self.slots.iter().any(|(s, _)| *s == GraphSlot::Detail(id)),
            None => false,
        }
    }

    /// Create a DetailGraph for the selected biome and attach it in the
    /// manifest. Until this existed, a biome created in-engine could never have
    /// foliage - `create_biome` writes `detail: None` and nothing could change it.
    pub fn attach_detail(&mut self) {
        if !self.require_saved("attaching a detail graph") {
            return;
        }
        let Some(id) = self.selected_biome_id() else {
            self.status = Some("Select a biome first".into());
            return;
        };
        let Some(m) = self.manifest.as_ref() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        let Some(entry) = m.biomes.iter().find(|b| b.id == id) else {
            self.status = Some(format!("Biome {id} is not in the manifest"));
            return;
        };
        if entry.detail.is_some() {
            self.status = Some("That biome already has a detail graph".into());
            return;
        }
        // Named after the biome's own file, so the pair reads as a pair on disk.
        let stem = entry.graph.strip_suffix(".graph.json").unwrap_or(&entry.graph);
        let file_name = format!("{stem}.detail.json");
        let path = world_generator::graphs_dir_public().join(&file_name);
        let graph = world_generator::new_detail_graph();
        if !self.write_new_graph(&path, &graph) {
            return;
        }
        let Some(m) = self.manifest.as_mut() else {
            return;
        };
        if let Some(entry) = m.biomes.iter_mut().find(|b| b.id == id) {
            entry.detail = Some(file_name);
        }
        if let Err(e) = self.write_manifest() {
            self.status = Some(format!("Attach failed: {e}"));
            return;
        }
        self.reload_and_select(GraphSlot::Detail(id));
        self.status = Some(format!("Attached {}", path.display()));
    }


    /// True when the selection is a biome or zone - the slots whose display name
    /// comes from their file name.
    pub fn active_is_renameable(&self) -> bool {
        matches!(self.selected, GraphSlot::Biome(_) | GraphSlot::Zone(_))
    }

    /// Rename the selected biome or zone: move its graph file, move its detail
    /// file if it has one, repoint the manifest, and reload.
    ///
    /// Exists because a display name here *is* a file name, which made renaming
    /// the one authoring operation that still required hand-editing the
    /// manifest — and the content gate forbids exactly that. The detail file
    /// moves with its biome, or the pair ends up mismatched on disk with nothing
    /// to notice it.
    pub fn rename_active(&mut self, name: &str) {
        if !self.require_saved("renaming") {
            return;
        }
        let slug = slugify(name);
        if slug.is_empty() {
            self.status = Some("Enter a new name first".into());
            return;
        }
        let (prefix, is_biome, id) = match self.selected {
            GraphSlot::Biome(id) => ("biome", true, id),
            GraphSlot::Zone(id) => ("zone", false, id),
            _ => {
                self.status = Some("Only a biome or zone can be renamed".into());
                return;
            }
        };

        let dir = world_generator::graphs_dir_public();
        let new_graph = format!("{prefix}_{slug}.graph.json");
        let new_path = dir.join(&new_graph);
        if new_path.exists() {
            self.status = Some(format!("{new_graph} already exists"));
            return;
        }

        // Read the current file names before mutating anything.
        let Some(m) = self.manifest.as_ref() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        let (old_graph, old_detail) = if is_biome {
            match m.biomes.iter().find(|b| b.id == id) {
                Some(b) => (b.graph.clone(), b.detail.clone()),
                None => {
                    self.status = Some(format!("Biome {id} is not in the manifest"));
                    return;
                }
            }
        } else {
            match m.zones.iter().find(|z| z.id == id) {
                Some(z) => (z.graph.clone(), None),
                None => {
                    self.status = Some(format!("Zone {id} is not in the manifest"));
                    return;
                }
            }
        };
        let new_detail = old_detail
            .as_ref()
            .map(|_| format!("{prefix}_{slug}.detail.json"));

        if let Err(e) = std::fs::rename(dir.join(&old_graph), &new_path) {
            self.status = Some(format!("Rename failed: {e}"));
            return;
        }
        if let (Some(od), Some(nd)) = (&old_detail, &new_detail) {
            // A missing detail file is not an error - the manifest entry is what
            // the engine reads, and the reload below will report a broken one.
            let _ = std::fs::rename(dir.join(od), dir.join(nd));
        }

        let Some(m) = self.manifest.as_mut() else {
            return;
        };
        if is_biome {
            if let Some(b) = m.biomes.iter_mut().find(|b| b.id == id) {
                b.graph = new_graph;
                if let Some(nd) = new_detail {
                    b.detail = Some(nd);
                }
            }
        } else if let Some(z) = m.zones.iter_mut().find(|z| z.id == id) {
            z.graph = new_graph;
        }
        if let Err(e) = self.write_manifest() {
            self.status = Some(format!("Rename failed: {e}"));
            return;
        }

        let slot = self.selected;
        self.reload_and_select(slot);
        self.status = Some(format!("Renamed to {slug}"));
    }

    /// Mutable handle to the "rename" name buffer.
    pub fn new_rename_mut(&mut self) -> &mut String {
        &mut self.new_rename
    }

    /// Delete the active biome graph: remove its manifest entry, delete its graph file
    /// and its detail file if it has one, and drop both from the selector. The
    /// primary biome (id 0) is protected.
    pub fn delete_active(&mut self) {
        if !self.require_saved("removing a biome") {
            return;
        }
        let id = match self.selected {
            GraphSlot::Biome(id) if id != 0 => id,
            _ => {
                self.status = Some("Only a non-primary biome can be deleted".into());
                return;
            }
        };
        // Both files, captured before the reload drops the path map. The old
        // path deleted only the biome graph and orphaned its detail file.
        let files: Vec<PathBuf> = [GraphSlot::Biome(id), GraphSlot::Detail(id)]
            .iter()
            .filter_map(|s| self.paths.get(s).cloned())
            .collect();
        let Some(m) = self.manifest.as_mut() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        m.biomes.retain(|b| b.id != id);
        if let Err(e) = self.write_manifest() {
            self.status = Some(format!("Delete failed: {e}"));
            return;
        }
        for p in files {
            // A missing file is fine - the manifest entry is what mattered.
            let _ = std::fs::remove_file(p);
        }
        self.reload_and_select(GraphSlot::Biome(0));
        self.status = Some(format!("Deleted biome {id}"));
    }

    /// The current selection's display label.
    pub fn active_label(&self) -> String {
        self.slots
            .iter()
            .find(|(s, _)| *s == self.selected)
            .map(|(_, l)| l.clone())
            .unwrap_or_default()
    }

    /// True when the active graph is a deletable (non-primary) biome.
    pub fn active_is_deletable_biome(&self) -> bool {
        matches!(self.selected, GraphSlot::Biome(id) if id != 0)
    }

    /// Mutable handle to the "new biome" name buffer (for the toolbar text box).
    pub fn new_name_mut(&mut self) -> &mut String {
        &mut self.new_name
    }

    /// Mutable handle to the "new zone" name buffer.
    pub fn new_zone_name_mut(&mut self) -> &mut String {
        &mut self.new_zone_name
    }

    /// Take the transient status line, if any (shown then cleared by the panel).
    pub fn take_status(&mut self) -> Option<String> {
        self.status.take()
    }
    
    /// The active editor's revision (for the field probe).
    pub fn revision(&self) -> u64 {
        self.editor.revision()
    }
    
    /// The active graph + the IR id of its selected node (for the field probe).
    pub fn build_graph_with_selection(&self) -> (Graph, Option<NodeId>) {
        self.editor.build_graph_with_selection()
    }
    
    /// Mutable access to the active editor (for the undo/redo toolbar).
    pub fn editor_mut(&mut self) -> &mut EditorState {
        &mut self.editor
    }
}

impl Default for HierarchyEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// Lowercase, alphanumerics and underscores only, spaces -> underscores.
fn slugify(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::GraphKind;
    
    #[test]
    fn load_opens_primary_biome_then_switches() {
        let mut h = HierarchyEditor::new();
        h.load(
            PathBuf::from("world.manifest.json"),
            vec![
                (GraphSlot::World, "World".to_string(), "world.graph.json".into(), Graph::of_kind(GraphKind::World)),
                (GraphSlot::Zone(0), "Zone".to_string(), "zone.graph.json".into(), Graph::of_kind(GraphKind::Zone)),
                (GraphSlot::Biome(0), "Meadow".to_string(), "m.graph.json".into(), Graph::of_kind(GraphKind::Biome)),
                (GraphSlot::Detail(0), "Meadow Detail".to_string(), "m.detail.json".into(), Graph::of_kind(GraphKind::Detail)),
                (GraphSlot::Biome(1), "Rocky".to_string(), "r.graph.json".into(), Graph::of_kind(GraphKind::Biome)),
            ],
        );
        assert_eq!(h.selected(), GraphSlot::Biome(0)); // opens on the primary biome
        h.select(GraphSlot::Biome(1));
        assert_eq!(h.selected(), GraphSlot::Biome(1));
        h.select(GraphSlot::World);
        assert_eq!(h.selected(), GraphSlot::World);
        // A detail graph is a slot of its own, and switching to it must carry
        // its `GraphKind` - the Detail validation rule depends on it.
        h.select(GraphSlot::Detail(0));
        assert_eq!(h.selected(), GraphSlot::Detail(0));
        assert_eq!(h.build_graph_with_selection().0.kind, GraphKind::Detail);
    }
}
