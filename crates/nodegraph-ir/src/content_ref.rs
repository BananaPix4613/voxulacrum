//! References in graph content: a name, plus the id it resolves to.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use voxel_core::MaterialId;

/// Something graph content names, as `granite` or `namespace:granite`: a
/// material, a library, a prefab.
///
/// The name is what a graph file stores. The id is filled in where content
/// loads (for materials, `nodegraph_hotreload::resolve_materials`) and never
/// serialized, so a graph file can only ever name what it references. Two
/// references are equal when their names are, so a graph equals its own round
/// trip through JSON.
#[derive(Clone, Debug)]
pub struct ContentRef<Id> {
    name: String,
    id: Option<Id>,
}

/// A material named by graph content.
pub type MaterialRef = ContentRef<MaterialId>;

/// A library graph named by graph content.
pub type LibraryGraphRef = ContentRef<crate::library::LibraryGraphId>;

/// A prefab named by graph content. Its id is the app's `PrefabId`, which this
/// crate carries as a `u32`.
pub type PrefabRef = ContentRef<u32>;

impl<Id: Copy> ContentRef<Id> {
    /// An unresolved reference to `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), id: None }
    }

    /// A reference already resolved to `id`, for graphs built in memory (tests
    /// and tools) that never pass through the hot-reload boundary.
    pub fn with_id(name: impl Into<String>, id: Id) -> Self {
        Self { name: name.into(), id: Some(id) }
    }

    /// The referenced name as content writes it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename the reference. Clears the resolved id, which belonged to the old
    /// name.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.id = None;
    }

    /// The resolved id, or `None` before resolution.
    pub fn id(&self) -> Option<Id> {
        self.id
    }

    /// Record the id this reference resolved to.
    pub fn set_id(&mut self, id: Id) {
        self.id = Some(id);
    }
}

impl<Id> PartialEq for ContentRef<Id> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<Id> Serialize for ContentRef<Id> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name)
    }
}

impl<'de, Id> Deserialize<'de> for ContentRef<Id> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|name| Self { name, id: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_its_name_alone() {
        let r = MaterialRef::with_id("granite", MaterialId(2));
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"granite\"");
        let back: MaterialRef = serde_json::from_str("\"granite\"").unwrap();
        assert_eq!(back, r, "equal by name");
        assert_eq!(back.id(), None, "the id never travels through a file");
    }

    #[test]
    fn a_number_is_refused() {
        assert!(serde_json::from_str::<MaterialRef>("6").is_err());
    }

    #[test]
    fn renaming_clears_the_id() {
        let mut r = MaterialRef::with_id("granite", MaterialId(2));
        r.set_name("soil");
        assert_eq!(r.id(), None);
    }
}
