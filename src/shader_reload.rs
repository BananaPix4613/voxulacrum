use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub struct ShaderWatcher {
    rx: mpsc::Receiver<notify::Result<Event>>,
    _watcher: notify::RecommendedWatcher,
}

impl ShaderWatcher {
    pub fn new(shader_dir: &Path) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .expect("Failed to create file watcher");
        
        watcher
            .watch(shader_dir, RecursiveMode::NonRecursive)
            .expect("Failed to watch shader directory");
        
        log::info!("Watching shader directory: {}", shader_dir.display());
        
        Self {
            rx,
            _watcher: watcher,
        }
    }
    
    /// Poll for changed `.wgsl` files. Returns deduplicated list of changed paths.
    pub fn poll_changes(&self) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        while let Ok(Ok(event)) = self.rx.try_recv() {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                for path in event.paths {
                    if path.extension().map_or(false, |ext| ext == "wgsl") {
                        if !changed.contains(&path) {
                            changed.push(path);
                        }
                    }
                }
            }
        }
        changed
    }
}