//! Editor state: the Snarl model, undo/redo stack, and dirty tracking.

use std::collections::HashMap;

use egui_snarl::{NodeId as SnarlNodeId, Snarl};
use nodegraph_ir::{Graph, GraphBoundary, GraphKind, NodeId, NodeKind};

use crate::params::GraphCatalogs;
use crate::bridge::{graph_to_snarl, snarl_to_graph};
use crate::viewer::{DiagnosticIndex, GraphViewer};

/// Maximum entries kept in each of the undo and redo stacks.
pub const UNDO_DEPTH: usize = 100;

/// A short, human-readable label for an undoable action.
pub type UndoLabel = String;

struct UndoEntry {
    label: UndoLabel,
    snapshot: Graph,
}

/// All editor state owned by the host app.
pub struct EditorState {
    /// The canvas model - egui-snarl's tree of nodes + wires.
    pub snarl: Snarl<NodeKind>,
    /// The role this graph plays in the hierarchy, and the boundary it
    /// declares. Neither is representable as a node, so the canvas cannot hold
    /// either one - they are carried here and restamped onto every graph built
    /// from the canvas. Without this, saving any graph rewrote it as a `Biome`
    /// with an empty boundary, which is why `world.graph.json` and
    /// `zone.graph.json` shipped labeled `Biome`.
    kind: GraphKind,
    boundary: GraphBoundary,
    /// Set whenever the canvas changes between frames.
    dirty: bool,
    /// True if there are unsaved changes vs the last `mark_saved` call.
    modified: bool,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    /// Transient toast: a short message + frames remaining to display.
    toast: Option<(String, u32)>,
    /// Monotonic counter bumped whenever the editor's inspectable state
    /// changes - a graph edit *or* a change in canvas selection. Derived
    /// views (e.g. the field probe) poll it to know when to recompute.
    revision: u64,
    /// First node currently selected on the canvas, captured during `show`.
    /// `None` when nothing is selected.
    selected_node: Option<SnarlNodeId>,
    /// Pre-edit graph snapshot captured at the first change of a parameter
    /// interaction (drag or text entry). Committed as one undo entry when the
    /// interaction ends, so a whole drag coalesces into a single undo.
    param_baseline: Option<Graph>,
    /// Label for the pending param undo entry (e.g. "Edit Layer").
    param_edit_label: Option<String>,
}

impl EditorState {
    /// Empty editor with no nodes.
    pub fn new() -> Self {
        Self {
            snarl: Snarl::new(),
            kind: GraphKind::default(),
            boundary: GraphBoundary::default(),
            dirty: false,
            modified: false,
            undo: Vec::new(),
            redo: Vec::new(),
            toast: None,
            revision: 0,
            selected_node: None,
            param_baseline: None,
            param_edit_label: None,
        }
    }

    /// Build from an existing `Graph` (e.g. JSON-loaded on startup).
    pub fn from_graph(graph: &Graph) -> Self {
        Self {
            snarl: graph_to_snarl(graph),
            kind: graph.kind,
            boundary: graph.boundary.clone(),
            ..Self::new()
        }
    }

    /// Serialize the canvas to a `Graph`, restamping the identity the canvas
    /// cannot carry. **Every** path from `Snarl` back to `Graph` goes through
    /// here, so `kind` and `boundary` cannot be preserved on the save path and
    /// dropped on the validation path.
    fn canvas_graph(&self) -> (Graph, HashMap<SnarlNodeId, NodeId>) {
        let (mut graph, id_map) = snarl_to_graph(&self.snarl);
        graph.kind = self.kind;
        graph.boundary = self.boundary.clone();
        (graph, id_map)
    }

    /// Serialize the current canvas to a fresh `Graph` (id-map discarded).
    pub fn build_graph(&self) -> Graph {
        self.canvas_graph().0
    }
    
    /// The current revision (see the `revision` field). Increments on every
    /// graph edit and on every change in canvas selection.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    
    /// Build the live graph together with the IR id of the node currently
    /// selected on the canvas, if any. The id is valid within the returned
    /// graph. Used by the field probe to evaluate and display whichever node
    /// the author has selected.
    pub fn build_graph_with_selection(&self) -> (Graph, Option<NodeId>) {
        let (graph, id_map) = self.canvas_graph();
        let selected = self.selected_node.and_then(|s| id_map.get(&s).copied());
        (graph, selected)
    }

    /// Render the editor canvas into `ui`, folding this frame's structural
    /// edits into a single undo entry. Parameter drags/entries coalesce into
    /// one undo entry per interaction (captured on first change, committed when
    /// the pointer is released and no widget holds focus).
    pub fn show(&mut self, ui: &mut egui::Ui, catalogs: &GraphCatalogs) {
        // Via `canvas_graph`, so per-`GraphKind` validation rules (e.g. a
        // DetailGraph needing a paint or scatter terminal) apply on the canvas
        // and not only after a save-and-reload.
        let (pre_graph, id_map) = self.canvas_graph();
        let diagnostics = DiagnosticIndex::from_diagnostics(&pre_graph.validate());
        
        let mut actions: Vec<UndoLabel> = Vec::new();
        let mut dirty_param = false;
        let mut param_label: Option<String> = None;
        let mut clicked: Option<SnarlNodeId> = None;
        let mut viewer = GraphViewer {
            id_map: &id_map,
            diagnostics: &diagnostics,
            actions: &mut actions,
            dirty_param: &mut dirty_param,
            param_label: &mut param_label,
            catalogs,
            selected: self.selected_node,
            clicked: &mut clicked,
            kind: self.kind,
        };
        self.snarl.show(
            &mut viewer,
            &crate::style::editor_snarl_style(),
            egui::Id::new("nodegraph-editor-canvas"),
            ui,
        );
        
        if !actions.is_empty() {
            // A structural edit landed this frame. Commit any param edit that was
            // in flight first, then fold the structural actions into one entry.
            self.commit_param_edit();
            let label = actions.join(", ");
            self.push_undo(label, pre_graph);
        } else if dirty_param {
            // First change of an interaction: capture the pre-edit baseline.
            if self.param_baseline.is_none() {
                self.param_baseline = Some(pre_graph);
                self.param_edit_label = param_label.map(|n| format!("Edit {n}"));
            }
            self.mark_dirty(); // live re-eval while the value is being dragged
        } else {
            // No change this frame. Commit the coalesced param edit once the
            // interaction is truly over (pointer released, nothing focused), so a
            // paused mid-drag doesn't fragment one drag into several undo entries.
            let interacting = ui.ctx().is_using_pointer()
                || ui.ctx().memory(|m| m.focused().is_some());
            if !interacting {
                self.commit_param_edit();
            }
        }

        if let Some(node) = clicked {
            if Some(node) != self.selected_node {
                self.selected_node = Some(node);
                self.revision = self.revision.wrapping_add(1);
            }
        }
    }

    /// Flush any pending parameter-edit baseline into a single undo entry.
    /// No-op when no param interaction is in flight.
    fn commit_param_edit(&mut self) {
        if let Some(base) = self.param_baseline.take() {
            let label = self
                .param_edit_label
                .take()
                .unwrap_or_else(|| "Edit parameter".to_string());
            self.push_undo(label, base);
        }
    }

    /// Push a labeled undo entry whose `pre_snapshot` is the graph state
    /// *before* the user action took place. Clears the redo stack. Also
    /// marks the editor dirty + modified, so the host can pick up the new
    /// graph via [`Self::consume_dirty`].
    pub fn push_undo(&mut self, label: impl Into<UndoLabel>, pre_snapshot: Graph) {
        if self.undo.len() >= UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(UndoEntry {
            label: label.into(),
            snapshot: pre_snapshot,
        });
        self.redo.clear();
        self.dirty = true;
        self.modified = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Mark that the canvas changed this frame (e.g. param drag). Does not
    /// push undo; small drags are coalesced into a single undo entry only
    /// when the user starts a *new* drag (call `push_undo` then).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.modified = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Pop a dirty flag (returns `true` once per actual change).
    pub fn consume_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    /// True if there are unsaved edits since the last `mark_saved`.
    pub fn is_modified(&self) -> bool { self.modified }

    /// Clear the modified flag (call after a successful Save).
    pub fn mark_saved(&mut self) { self.modified = false; }

    /// Undo the most recent action. Returns the label that was undone.
    pub fn undo(&mut self) -> Option<UndoLabel> {
        let entry = self.undo.pop()?;
        let current = self.build_graph();
        self.redo.push(UndoEntry { label: entry.label.clone(), snapshot: current });
        self.snarl = graph_to_snarl(&entry.snapshot);
        self.param_baseline = None;
        self.param_edit_label = None;
        self.dirty = true;
        self.modified = true;
        self.revision = self.revision.wrapping_add(1);
        Some(entry.label)
    }

    /// Redo the last undone action. Returns the label that was redone.
    pub fn redo(&mut self) -> Option<UndoLabel> {
        let entry = self.redo.pop()?;
        let current = self.build_graph();
        self.undo.push(UndoEntry { label: entry.label.clone(), snapshot: current });
        self.snarl = graph_to_snarl(&entry.snapshot);
        self.param_baseline = None;
        self.param_edit_label = None;
        self.dirty = true;
        self.modified = true;
        self.revision = self.revision.wrapping_add(1);
        Some(entry.label)
    }

    /// Label of the action the next undo would revert (for the menu).
    pub fn peek_undo(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }
    /// Label of the action the next redo would re-apply.
    pub fn peek_redo(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Briefly show a status message in the editor footer.
    pub fn toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), 180)); // -3 s at 60 fps
    }

    /// Render the active toast and tick its lifetime. Call once per frame
    /// below the canvas.
    pub fn show_toast(&mut self, ui: &mut egui::Ui) {
        if let Some((msg, ttl)) = &mut self.toast {
            ui.colored_label(egui::Color32::YELLOW, msg.as_str());
            if *ttl == 0 {
                self.toast = None;
            } else {
                *ttl -= 1;
            }
        }
    }
}

impl Default for EditorState {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodegraph_ir::{BoundaryPort, ConstantParams, PinType};

    #[test]
    fn canvas_round_trip_preserves_kind_and_boundary() {
        let mut source = Graph::of_kind(GraphKind::World);
        source.boundary.outputs.push(BoundaryPort::new("climate", PinType::SurfaceField));
        source.add_node(NodeKind::Constant(ConstantParams::default()));

        let saved = EditorState::from_graph(&source).build_graph();

        assert_eq!(
            saved.kind,
            GraphKind::World,
            "saving a World graph must not relabel it Biome",
        );
        assert_eq!(
            saved.boundary, source.boundary,
            "a Library graph's declared inputs are not derivable from its nodes, \
             so dropping the boundary on save would destroy them",
        );
    }

    #[test]
    fn undo_preserves_kind() {
        // Undo restores the canvas from a snapshot; identity lives beside the
        // canvas, so it must survive that path too.
        let mut editor = EditorState::from_graph(&Graph::of_kind(GraphKind::Detail));
        let snapshot = editor.build_graph();
        editor.push_undo("test", snapshot);
        editor.undo().expect("one undo entry");
        assert_eq!(editor.build_graph().kind, GraphKind::Detail);
    }
}
