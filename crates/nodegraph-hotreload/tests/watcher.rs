//! Smoke test: writing a `.json` file in a watched dir produces a poll event.
//!
//! Filesystem-watcher tests are inherently mildly flaky on slow CI; the 5 s
//! timeout is generous to compensate. Locally this passes in <200 ms.

use std::time::{Duration, Instant};

use nodegraph_hotreload::GraphWatcher;

#[test]
fn write_triggers_event() {
    let dir = std::env::temp_dir().join(format!("voxulacrum_hr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let watcher = GraphWatcher::new(&dir).unwrap();

    // notify needs a moment to arm.
    std::thread::sleep(Duration::from_millis(100));

    let target = dir.join("dummy.json");
    std::fs::write(&target, b"{}").unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() >= deadline {
            let _ = std::fs::remove_dir_all(&dir);
            panic!("expected a poll event within 5s");
        }
        let changes = watcher.poll_changes();
        if changes.iter().any(|p| p == &target) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&dir);
}
