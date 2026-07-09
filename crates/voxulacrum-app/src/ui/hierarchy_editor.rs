//! The embedded editor plus the multi-graph hierarchy it edits.

use std::collections::HashMap;

use nodegraph_editor::EditorState;
use nodegraph_ir::{Graph, NodeId};

use crate::world::world_generator::GraphSlot;

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
}

impl HierarchyEditor {
    /// Empty editor (no graphs). Populate via [`HierarchyEditor::load`]
    pub fn new() -> Self {
        Self {
            editor: EditorState::new(),
            slots: Vec::new(),
            selected: GraphSlot::Biome(0),
            set: HashMap::new(),
        }
    }

    /// Replace the hierarchy with a manifest-loaded graph set, opening on the
    /// primary biome (biome 0) if present, else the first entry.
    pub fn load(&mut self, graphs: Vec<(GraphSlot, String, Graph)>) {
        self.slots = graphs.iter().map(|(s, l, _)| (*s, l.clone())).collect();
        let default = GraphSlot::Biome(0);
        self.selected = if graphs.iter().any(|(s, _, _)| *s == default) {
            default
        } else {
            graphs.first().map(|(s, _, _)| *s).unwrap_or(default)
        };
        self.set = graphs.into_iter().map(|(s, _, g)| (s, g)).collect();
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
        let current = self
            .slots
            .iter()
            .find(|(s, _)| *s == self.selected)
            .map(|(_, l)| l.clone())
            .unwrap_or_default();
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
        self.editor.show(ui);
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

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::GraphKind;
    
    #[test]
    fn load_opens_primary_biome_then_switches() {
        let mut h = HierarchyEditor::new();
        h.load(vec![
            (GraphSlot::World, "World".to_string(), Graph::of_kind(GraphKind::World)),
            (GraphSlot::Zone, "Zone".to_string(), Graph::of_kind(GraphKind::Zone)),
            (GraphSlot::Biome(0), "Meadow".to_string(), Graph::of_kind(GraphKind::Biome)),
            (GraphSlot::Biome(1), "Rocky".to_string(), Graph::of_kind(GraphKind::Biome)),
        ]);
        assert_eq!(h.selected(), GraphSlot::Biome(0)); // opens on the primary biome
        h.select(GraphSlot::Biome(1));
        assert_eq!(h.selected(), GraphSlot::Biome(1));
        h.select(GraphSlot::World);
        assert_eq!(h.selected(), GraphSlot::World);
    }
}
