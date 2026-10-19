//! The engine testbed: a flat world with a handful of test materials, built on
//! the `voxulacrum` facade alone. It demonstrates engine features without game
//! content, and doubles as the isolated preview scene.

use std::path::PathBuf;

fn main() {
    // Anchored to this crate at compile time, like the engine's own asset root,
    // so the testbed finds its content from any working directory.
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("content");
    voxulacrum::run_with(voxulacrum::RunOptions::new().package(content));
}
