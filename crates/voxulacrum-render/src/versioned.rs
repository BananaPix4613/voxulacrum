//! The handoff from the main world to the renderer. The main world writes a
//! `Versioned` value and every write moves its version; a pass keeps an
//! `Uploaded` mark of the version it last copied to the GPU and uploads again
//! only when the two differ.

/// A value the main world hands to the renderer, with a version that moves on
/// every write.
#[derive(Default)]
pub struct Versioned<T> {
    value: T,
    version: u64,
}

impl<T> Versioned<T> {
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Replace the value. The renderer uploads it on its next pass.
    pub fn set(&mut self, value: T) {
        self.value = value;
        self.version += 1;
    }
}

/// The version of a `Versioned` value a pass last uploaded. Both start at 0, so
/// the starting value counts as uploaded: the pass's empty initial state stands
/// in for it, and nothing uploads before the first `set`.
#[derive(Default)]
pub struct Uploaded(u64);

impl Uploaded {
    /// The value, when its version moved since the last call, recording the new
    /// version. `None` when the GPU already holds it.
    pub fn changed<'a, T>(&mut self, source: &'a Versioned<T>) -> Option<&'a T> {
        if self.0 == source.version {
            return None;
        }
        self.0 = source.version;
        Some(&source.value)
    }
}
