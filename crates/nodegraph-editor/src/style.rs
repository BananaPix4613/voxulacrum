//! Canvas-wide [`SnarlStyle`] for the editor. Per-node visuals (header band,
//! body) and per-pin visuals (shape, fill) live in `viewer.rs`; this module
//! holds only the global baseline shared by every node and wire.

use egui::{Color32, Frame, Stroke, vec2};
use egui_snarl::ui::{BackgroundPattern, Grid, SnarlStyle};

/// The editor's canvas style. Cheap to build, so constructed fresh each
/// frame from snarl's all-`None` baseline ([`SnarlStyle::new`]) with only
/// the fields we customize set.
pub fn editor_snarl_style() -> SnarlStyle {
    SnarlStyle {
        // Thin dark rim around every pin marker (overridable per-pin, but no
        // pin overrides it today) so bright type colors stay legible against
        // node bodies and wires.
        pin_stroke: Some(Stroke::new(1.0, Color32::from_gray(30))),
        // Slightly thicker than snarl's default (`pin_size * 0.1`, ~1px) so
        // connections read clearly against the grid. Colored per-pin (set in
        // the viewer) and Bezier5 by snarl default; both left untouched.
        wire_width: Some(2.5),
        // Axis-aligned grid (snarl's default is rotated ~1 rad) with dimmed
        // lines so the pattern recedes behind the nodes.
        bg_pattern: Some(BackgroundPattern::Grid(Grid::new(vec2(50.0, 50.0), 0.0))),
        bg_pattern_stroke: Some(Stroke::new(1.0, Color32::from_gray(40))),
        // Backdrop slightly darker than the node bodies (~gray 27) so nodes
        // lift off the canvas.
        bg_frame: Some(Frame::new().fill(Color32::from_gray(18))),
        ..SnarlStyle::new()
    }
}
