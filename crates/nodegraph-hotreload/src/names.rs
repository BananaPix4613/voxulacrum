//! Content references by name at the hot-reload boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use voxel_core::QualifiedName;

use crate::error::{HotReloadError, HotReloadResult};

/// Where each loaded namespace keeps one kind of authored file, such as its
/// blueprints or its species, so a name resolves into the namespace it names.
#[derive(Clone, Debug, Default)]
pub struct ContentDirs {
    dirs: BTreeMap<String, PathBuf>,
}

impl ContentDirs {
    /// No namespaces yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `namespace`, whose files of this kind are in `dir`.
    pub fn with(mut self, namespace: impl Into<String>, dir: impl Into<PathBuf>) -> Self {
        self.dirs.insert(namespace.into(), dir.into());
        self
    }

    /// The name `text` names, read in `namespace` (the namespace of the content
    /// holding the reference), and the directory its file is in. A name in a
    /// namespace that is not loaded is an error that says so, rather than a
    /// missing file.
    pub(crate) fn locate(
        &self,
        text: &str,
        namespace: &str,
        kind: &str,
    ) -> HotReloadResult<(QualifiedName, &Path)> {
        let name = QualifiedName::parse(text, namespace)
            .map_err(|e| HotReloadError::Blueprint(format!("{kind} {e}")))?;
        match self.dirs.get(name.namespace()) {
            Some(dir) => Ok((name, dir.as_path())),
            None => Err(HotReloadError::Blueprint(format!(
                "{kind} {name} is in namespace `{}`, which is not loaded",
                name.namespace()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> ContentDirs {
        ContentDirs::new()
            .with("sandbox", "packages/sandbox/species")
            .with("trees", "packages/trees/species")
    }

    #[test]
    fn a_bare_or_own_namespace_reference_names_a_local_file() {
        let dirs = dirs();
        for text in ["oak", "sandbox:oak"] {
            let (name, dir) = dirs.locate(text, "sandbox", "species").unwrap();
            assert_eq!((name.name(), dir), ("oak", Path::new("packages/sandbox/species")));
        }
    }

    #[test]
    fn a_reference_into_a_loaded_namespace_names_that_namespace_s_file() {
        let dirs = dirs();
        let (name, dir) = dirs.locate("trees:birch", "sandbox", "species").unwrap();
        assert_eq!(name.to_string(), "trees:birch");
        assert_eq!(dir, Path::new("packages/trees/species"));
    }

    #[test]
    fn a_reference_into_another_namespace_says_it_is_not_loaded() {
        let err = dirs().locate("testbed:oak", "sandbox", "species").unwrap_err().to_string();
        assert!(err.contains("not loaded"), "{err}");
    }

    #[test]
    fn a_malformed_reference_is_refused() {
        assert!(dirs().locate("Oak Tree", "sandbox", "species").is_err());
    }
}
