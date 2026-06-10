//! # nodegraph-editor
//!
//! Visual node-graph editor for [`nodegraph_ir::Graph`], built on
//! [`egui_snarl::Snarl`]. The editor's source of truth is
//! `Snarl<NodeKind>`; conversion to/from `Graph` happens at save/load and
//! at every push to the live preview runtime.
//!
//! ## Embedding
//!
//! ```ignore
//! let mut state = EditorState::from_graph(graph);
//! // each frame:
//! state.show(ui);
//! if state.consume_dirty() {
//!     runtime.replace_graph(state.build_graph());
//! }
//! ```

#![warn(missing_docs)]

mod bridge;
mod colors;
mod params;
mod state;
mod style;
mod viewer;

pub use bridge::{graph_to_snarl, snarl_to_graph};
pub use colors::{category_fill, pin_color};
pub use state::{EditorState, UndoLabel};
pub use viewer::{DiagnosticIndex, GraphViewer};
