//! Named scalar parameter sidecars.
//! 
//! A graph tier carries named scalar parameters alongside its graph. Declared in
//! the world manifest, threaded into evaluation (read by `BiomeParam` and
//! `WorldParam` nodes), and read directly by engine passes such as slab
//! smoothing. Free-form: a name with no entry falls back to a caller-supplied
//! default.
//! 
//! One type, two aliases. The mechanism is "named scalar attached to a thing",
//! and a world-level sidecar called `BiomeParams` would name it after the first
//! thing that used it.

use std::collections::HashMap;

/// A named scalar parameter set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScalarParams {
    entries: HashMap<String, f32>,
}

/// A biome's named scalar parameters (the biome-param sidecar).
pub type BiomeParams = ScalarParams;

/// The world's named scalar parameters (the world-param sidecar).
pub type WorldParams = ScalarParams;

impl ScalarParams {
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
