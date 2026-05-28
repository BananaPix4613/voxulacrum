//! Material identifier newtype.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Material identifier. Stable across runs; the meaning of each ID is
/// defined by the application (e.g. an external material registry).
///
/// `MaterialId(0)` is reserved for "air" by convention but `voxel-core`
/// imposes no such requirement - application code should provide its own
/// constants if it wants reserved values.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct MaterialId(pub u16);

impl MaterialId {
    /// The conventional "no material" ID. Application code may treat this
    /// as air, void, or any sentinel it likes.
    pub const AIR: Self = Self(0);
    
    /// Raw u16 value.
    #[inline]
    pub const fn raw(self) -> u16 {
        self.0
    }
}

impl From<u16> for MaterialId {
    #[inline]
    fn from(v: u16) -> Self { Self(v) }
}

impl From<MaterialId> for u16 {
    #[inline]
    fn from(m: MaterialId) -> Self { m.0 }
}
