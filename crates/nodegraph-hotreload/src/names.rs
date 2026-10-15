//! Content references by name at the hot-reload boundary.

use voxel_core::QualifiedName;

use crate::error::{HotReloadError, HotReloadResult};

/// The file name a blueprint or species reference names, for a reference
/// written in content that loads under `namespace`.
/// 
/// Only `namespace`'s own files are on disk until packages load several
/// namespaces at once, so a reference into any other namespace is an error that
/// says so, rather than a missing file.
pub(crate) fn local_name(text: &str, namespace: &str, kind: &str) -> HotReloadResult<String> {
    let name = QualifiedName::parse(text, namespace)
        .map_err(|e| HotReloadError::Blueprint(format!("{kind} {e}")))?;
    if name.namespace() != namespace {
        return Err(HotReloadError::Blueprint(format!(
            "{kind} {name} is in namespace `{}`, which is not loaded",
            name.namespace()
        )));
    }
    Ok(name.name().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn a_bare_or_own_namespace_reference_names_a_local_file() {
        assert_eq!(local_name("oak", "voxulacrum", "species").unwrap(), "oak");
        assert_eq!(local_name("voxulacrum:oak", "voxulacrum", "species").unwrap(), "oak");
    }
    
    #[test]
    fn a_reference_into_another_namespace_says_it_is_not_loaded() {
        let err = local_name("testbed:oak", "voxulacrum", "species").unwrap_err().to_string();
        assert!(err.contains("not loaded"), "{err}");
    }
    
    #[test]
    fn a_malformed_reference_is_refused() {
        assert!(local_name("Oak Tree", "voxulacrum", "species").is_err());
    }
}
