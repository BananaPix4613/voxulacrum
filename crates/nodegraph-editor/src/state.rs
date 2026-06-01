//! Editor state: the Snarl model, undo/redo stack, and dirty tracking.

use egui_snarl::Snarl;
use nodegraph_ir::{Graph, NodeKind};

use crate::bridge::{graph_to_snarl, snarl_to_graph};

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
    /// Set whenever the canvas changes between frames.
    dirty: bool,
    /// True if there are unsaved changes vs the last `mark_saved` call.
    modified: bool,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    /// Transient toast: a short message + frames remaining to display.
    toast: Option<(String, u32)>,
}

impl EditorState {
    /// Empty editor with no nodes.
    pub fn new() -> Self {
        Self {
            snarl: Snarl::new(),
            dirty: false,
            modified: false,
            undo: Vec::new(),
            redo: Vec::new(),
            toast: None,
        }
    }

    /// Build from an existing `Graph` (e.g. JSON-loaded on startup).
    pub fn from_graph(graph: &Graph) -> Self {
        Self {
            snarl: graph_to_snarl(graph),
            ..Self::new()
        }
    }

    /// Serialize the current canvas to a fresh `Graph` (id-map discarded).
    pub fn build_graph(&self) -> Graph {
        snarl_to_graph(&self.snarl).0
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
    }

    /// Mark that the canvas changed this frame (e.g. param drag). Does not
    /// push undo; small drags are coalesced into a single undo entry only
    /// when the user starts a *new* drag (call `push_undo` then).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.modified = true;
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
        self.dirty = true;
        self.modified = true;
        Some(entry.label)
    }

    /// Redo the last undone action. Returns the label that was redone.
    pub fn redo(&mut self) -> Option<UndoLabel> {
        let entry = self.redo.pop()?;
        let current = self.build_graph();
        self.undo.push(UndoEntry { label: entry.label.clone(), snapshot: current });
        self.snarl = graph_to_snarl(&entry.snapshot);
        self.dirty = true;
        self.modified = true;
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
