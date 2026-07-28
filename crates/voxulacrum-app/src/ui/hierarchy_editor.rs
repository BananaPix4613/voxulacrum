//! The embedded editor plus the multi-graph hierarchy it edits.

use std::collections::HashMap;
use std::path::PathBuf;

use nodegraph_editor::EditorState;
use nodegraph_ir::{Graph, NodeId};

use crate::world::world_generator::{self, GraphSlot};

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
    /// On-disk path per slot, for saving.
    paths: HashMap<GraphSlot, PathBuf>,
    /// The world manifest path, for adding/removing biome entries.
    manifest_path: Option<PathBuf>,
    /// Name entry for the "New biome" action.
    new_name: String,
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
            paths: HashMap::new(),
            manifest_path: None,
            new_name: String::new(),
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
        self.manifest_path = Some(manifest_path);
        let default = GraphSlot::Biome(0);
        self.selected = if graphs.iter().any(|(s, _, _, _)| *s == default) {
            default
        } else {
            graphs.first().map(|(s, _, _, _)| *s).unwrap_or(default)
        };
        self.set = graphs.into_iter().map(|(s, _, _, g)| (s, g)).collect();
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
        self.set.insert(self.selected, self.editor.build_graph());
        if let Some(g) = self.set.get(&slot) {
            self.editor = EditorState::from_graph(g);
            self.selected = slot;
        }
    }
    
    /// Update the stored graph for `slot` (e.g. from hot-reload). Refreshes the
    /// canvas if that slot is currently shown.
    pub fn refresh(&mut self, slot: GraphSlot, graph: Graph) {
        if self.selected == slot {
            self.editor = EditorState::from_graph(&graph);
        }
        self.set.insert(slot, graph);
    }
    
    /// Draw the selector dropdown above the canvas, then the canvas itself.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let mut chosen = self.selected;
        let current = self.active_label();
        egui::ComboBox::from_label("Graph")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (slot, label) in &self.slots {
                    ui.selectable_value(&mut chosen, *slot, label);
                }
            });
        if chosen != self.selected {
            self.select(chosen);
        }
        ui.separator();
        // Biome (id, name) catalog for ZoneOutput's per-band biome picker.
        let biomes: Vec<(u16, String)> = self
            .slots
            .iter()
            .filter_map(|(s, l)| match s {
                GraphSlot::Biome(id) => Some((*id, l.clone())),
                _ => None,
            })
            .collect();
        self.editor.show(ui, &biomes);
    }
    
    /// The slot currently shown.
    #[allow(dead_code)] // selection accessor
    pub fn selected(&self) -> GraphSlot {
        self.selected
    }
    
    /// If the active canvas changed this frame, the `(slot, graph)` to
    /// regenerate from.
    pub fn consume_dirty(&mut self) -> Option<(GraphSlot, Graph)> {
        if self.editor.consume_dirty() {
            Some((self.selected, self.editor.build_graph()))
        } else {
            None
        }
    }
    
    /// True if the active canvas has unsaved edits.
    pub fn is_modified(&self) -> bool {
        self.editor.is_modified()
    }

    /// Write the active graph to its slot's file and clear the modified flag.
    pub fn save_active(&mut self) {
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
                self.status = Some(format!("Saved {}", path.display()));
            }
            Err(e) => self.status = Some(format!("Save failed: {e}")),
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

    /// Create a new biome graph from the template, write its file, register it
    /// in the manifest, and switch to it. `name` seeds the file name + label.
    pub fn create_biome(&mut self, name: &str) {
        let Some(manifest) = self.manifest_path.clone() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        let slug = slugify(name);
        if slug.is_empty() {
            self.status = Some("Enter a biome name first".into());
            return;
        }
        let id = self.next_biome_id();
        let file_name = format!("biome_{slug}.graph.json");
        let dir = world_generator::graphs_dir_public();
        let path = dir.join(&file_name);
        if path.exists() {
            self.status = Some(format!("{file_name} already exists"));
            return;
        }
        let graph = world_generator::new_biome_graph();
        let write = graph
            .to_json_pretty()
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()))
            .and_then(|()| world_generator::add_biome_entry(&manifest, id, &file_name));
        match write {
            Ok(()) => {
                let slot = GraphSlot::Biome(id);
                let label = capitalize(&slug);
                // Fold the current canvas back first so switching doesn't lose it.
                self.set.insert(self.selected, self.editor.build_graph());
                self.slots.push((slot, label));
                self.paths.insert(slot, path.clone());
                self.set.insert(slot, graph.clone());
                self.editor = EditorState::from_graph(&graph);
                self.selected = slot;
                self.status = Some(format!("Created {}", path.display()));
            }
            Err(e) => self.status = Some(format!("Create failed: {e}")),
        }
    }

    /// Delete the active biome graph: remove its manifest entry, delete the file,
    /// and drop it from the selector. World, Zone, and the primary biome (id 0)
    /// are protected.
    pub fn delete_active(&mut self) {
        let slot = self.selected;
        let id = match slot {
            GraphSlot::Biome(id) if id != 0 => id,
            _ => {
                self.status = Some("World, Zone, and the primary biome can't be deleted".into());
                return;
            }
        };
        let Some(manifest) = self.manifest_path.clone() else {
            self.status = Some("No manifest loaded".into());
            return;
        };
        let path = self.paths.get(&slot).cloned();
        let result = world_generator::remove_biome_entry(&manifest, id).and_then(|()| {
            if let Some(p) = &path {
                // Missing file is fine - the manifest entry is what matters.
                let _ = std::fs::remove_file(p);
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                self.slots.retain(|(s, _)| *s != slot);
                self.paths.remove(&slot);
                self.set.remove(&slot);
                self.selected = GraphSlot::Biome(0);
                if let Some(g) = self.set.get(&self.selected) {
                    self.editor = EditorState::from_graph(g);
                }
                self.status = Some(format!("Deleted biome {id}"));
            }
            Err(e) => self.status = Some(format!("Delete failed: {e}")),
        }
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

/// "rocky_hills" -> "Rocky_hills" (display label for a new biome).
fn capitalize(slug: &str) -> String {
    let mut chars = slug.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::GraphKind;
    
    #[test]
    fn load_opens_primary_biome_then_switches() {
        let mut h = HierarchyEditor::new();
        h.load(
            std::path::PathBuf::from("world.manifest.json"),
            vec![
                (GraphSlot::World, "World".to_string(), "world.graph.json".into(), Graph::of_kind(GraphKind::World)),
                (GraphSlot::Zone, "Zone".to_string(), "zone.graph.json".into(), Graph::of_kind(GraphKind::Zone)),
                (GraphSlot::Biome(0), "Meadow".to_string(), "m.graph.json".into(), Graph::of_kind(GraphKind::Biome)),
                (GraphSlot::Biome(1), "Rocky".to_string(), "r.graph.json".into(), Graph::of_kind(GraphKind::Biome)),
            ],
        );
        assert_eq!(h.selected(), GraphSlot::Biome(0)); // opens on the primary biome
        h.select(GraphSlot::Biome(1));
        assert_eq!(h.selected(), GraphSlot::Biome(1));
        h.select(GraphSlot::World);
        assert_eq!(h.selected(), GraphSlot::World);
    }
}
