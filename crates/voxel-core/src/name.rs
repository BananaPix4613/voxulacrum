//! Namespaced content identity: `namespace:name`.
//!
//! Every registry entry (materials, prefabs, libraries, blueprints, species)
//! is identified by a [`QualifiedName`]. Content files write bare names, which
//! resolve in the namespace of the content holding them; a qualified name
//! reaches into another namespace. Numeric ids are assigned at load.

use std::fmt;

/// Namespace of the content shipped with the engine: the reference world, and
/// the throwaway content engine tests use.
pub const ENGINE_NAMESPACE: &str = "voxulacrum";

/// A registry entry's identity: a namespace and a name within it. Each part is
/// one or more of `a-z`, `0-9`, `_` and `-`. Displays as `namespace:name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QualifiedName {
    namespace: String,
    name: String,
}

impl QualifiedName {
    /// Build a name from its two parts, validating each.
    pub fn new(namespace: &str, name: &str) -> Result<Self, String> {
        validate_part(namespace, "namespace")?;
        validate_part(name, "name")?;
        Ok(Self { namespace: namespace.to_string(), name: name.to_string() })
    }

    /// Parse `text` as content writes it. A bare name (`granite`) takes
    /// `default_namespace`, the namespace of the content holding the
    /// reference; `testbed:granite` names its own.
    pub fn parse(text: &str, default_namespace: &str) -> Result<Self, String> {
        let parsed = match text.split_once(':') {
            Some((namespace, name)) => Self::new(namespace, name),
            None => Self::new(default_namespace, text),
        };
        parsed.map_err(|e| format!("{text:?}: {e}"))
    }

    /// Check that `namespace` is a valid namespace, for code that takes one
    /// before any name exists in it.
    pub fn validate_namespace(namespace: &str) -> Result<(), String> {
        validate_part(namespace, "namespace")
    }

    /// The namespace part.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The name within the namespace.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// This name as content in `namespace` writes it: bare when `namespace` is
    /// this name's own, qualified otherwise. [`parse`](Self::parse) with the
    /// same namespace reads it back to an equal name.
    pub fn relative_to(&self, namespace: &str) -> String {
        if self.namespace == namespace {
            self.name.clone()
        } else {
            self.to_string()
        }
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.name)
    }
}

/// Check one part of a name. Crate-visible so blueprint names, which become
/// file names, are held to the same rule.
pub(crate) fn validate_part(part: &str, what: &str) -> Result<(), String> {
    if part.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if let Some(c) = part.chars().find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '_' | '-')) {
        return Err(format!("{what} {part:?} contains {c:?}; use a-z, 0-9, _ and -"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_takes_the_default_namespace() {
        let n = QualifiedName::parse("granite", "voxulacrum").unwrap();
        assert_eq!((n.namespace(), n.name()), ("voxulacrum", "granite"));
    }

    #[test]
    fn a_qualified_name_keeps_its_namespace() {
        let n = QualifiedName::parse("testbed:test_light", "voxulacrum").unwrap();
        assert_eq!((n.namespace(), n.name()), ("testbed", "test_light"));
    }

    #[test]
    fn displays_as_namespace_colon_name() {
        let n = QualifiedName::new("voxulacrum", "grass_soil").unwrap();
        assert_eq!(n.to_string(), "voxulacrum:grass_soil");
    }

    #[test]
    fn relative_to_round_trips_through_parse() {
        let n = QualifiedName::new("testbed", "chunk-o-land").unwrap();
        for namespace in ["testbed", "voxulacrum"] {
            let written = n.relative_to(namespace);
            assert_eq!(QualifiedName::parse(&written, namespace).unwrap(), n, "{written}");
        }
        assert_eq!(n.relative_to("testbed"), "chunk-o-land");
        assert_eq!(n.relative_to("voxulacrum"), "testbed:chunk-o-land");
    }

    #[test]
    fn rejects_malformed_names() {
        for text in ["", ":granite", "granite:", "Granite", "grass soil", "a/b", "..", "a:b:c"] {
            assert!(QualifiedName::parse(text, "voxulacrum").is_err(), "{text:?} was accepted");
        }
    }

    #[test]
    fn rejects_a_malformed_default_namespace() {
        assert!(QualifiedName::parse("granite", "Voxulacrum").is_err());
        assert!(QualifiedName::validate_namespace("my world").is_err());
        assert!(QualifiedName::validate_namespace("testbed").is_ok());
    }
}
