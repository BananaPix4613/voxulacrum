//! Per-biome scalar parameter sidecar.
//! 
//! A biome carries named scalar parameters alongside its graph - the
//! "biome-param sidecar". Declared in the world manifest, threaded into
//! biome-graph evaluation (read by `BiomeParam` nodes), and read directly by
//! engine passes such as slab smoothing (Substep 6). Free-form: a name with no
//! entry falls back to a caller-supplied default.

use std::collections::HashMap;

/// A biome's named scalar parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeParams {
    entries: HashMap<String, f32>,
}

impl BiomeParams {
    /// Empty parameter set.
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Build from a name -> value map.
    pub fn from_entries(entries: HashMap<String, f32>) -> Self {
        Self { entries }
    }
    
    /// The value of `name`, or `None` if unset.
    pub fn get(&self, name: &str) -> Option<f32> {
        self.entries.get(name).copied()
    }
    
    /// The value of `name`, or `default` if unset.
    pub fn get_or(&self, name: &str, default: f32) -> f32 {
        self.entries.get(name).copied().unwrap_or(default)
    }
    
    /// Insert or replace a parameter.
    pub fn set(&mut self, name: impl Into<String>, value: f32) {
        self.entries.insert(name.into(), value);
    }
    
    /// True if no parameters are set.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
