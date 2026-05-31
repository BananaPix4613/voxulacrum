//! `voxulacrum-preview` - standalone live heatmap window.
//!
//! Watches a directory of `nodegraph-ir` JSON graphs; evaluates the chosen
//! graph for chunk `(0, 0, 0)`; repaints a heatmap when the file changes.

#![warn(missing_docs)]

mod app;
mod colormap;
mod runtime;

use std::path::PathBuf;

use crate::app::PreviewApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/graphs"));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_title("Voxulacrum Preview"),
        ..Default::default()
    };

    eframe::run_native(
        "voxulacrum-preview",
        options,
        Box::new(move |_cc| {
            let app = PreviewApp::new(dir.clone())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            Ok(Box::new(app))
        }),
    )
}
