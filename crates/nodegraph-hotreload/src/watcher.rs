//! File watcher for `*.json` graph files.

use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex};

use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::error::HotReloadResult;

/// Watches a directory for changes to `*.json` files. Mirrors the pattern of
/// `voxulacrum-app::shader_reload::ShaderWatcher`.
pub struct GraphWatcher {
    rx: Mutex<mpsc::Receiver<notify::Result<Event>>>,
    _watcher: notify::RecommendedWatcher,
}

impl GraphWatcher {
    /// Begin watching `dir` (non-recursive). The dir must already exist.
    pub fn new(dir: &Path) -> HotReloadResult<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        log::info!("watching graph directory: {}", dir.display());
        Ok(Self { rx: Mutex::new(rx), _watcher: watcher })
    }

    /// Drain pending events: return deduplicated `.json` paths that were
    /// created or modified since the last poll. Empty `Vec` if nothing
    /// changed.
    pub fn poll_changes(&self) -> Vec<PathBuf> {
        let rx = self.rx.lock().expect("GraphWatcher mutex poisoned");
        let mut changed = Vec::new();
        while let Ok(Ok(event)) = rx.try_recv() {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in event.paths {
                    if path.extension().is_some_and(|ext| ext == "json")
                        && !changed.contains(&path)
                    {
                        changed.push(path);
                    }
                }
            }
        }
        changed
    }
}
