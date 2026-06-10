//! # voxel-core
//!
//! Voxel and chunk primitives for the voxulacrum world-generation engine.
//!
//! ## Scope
//!
//! This crate owns the **material/shape primitive** only: the [`Voxel`] struct
//! and the [`ChunkBuffer`] storage container. It deliberately does *not* own
//! lighting, moisture/temperature simulation, flora, or any rendering-side
//! sidecar state. Those are layered on top in the application crate as
//! parallel SoA fields keyed by the same chunk-local indexing.
//!
//! ## Voxel format
//!
//! ```text
//! shape    : ShapeId  (3 bits logical - 3 named variants)
//! material : MaterialId (16 bits)
//! flags    : u8       (8 bits - light source, water-logged, etc.)
//! ----------------------------------
//! total    : 27 bits  -> packs into a u32 for serialization / GPU upload
//! ```
//!
//! In memory the struct is `#[repr(C)]` with fields ordered `material, shape,
//! flags` — a tight 4 bytes with no padding. Use [`Voxel::pack`] /
//! [`Voxel::unpack`] for the 27-bit packed form.
//!
//! ## Chunk dimensions
//!
//! [`ChunkBuffer`] is generic over both element type `T` and edge length `N`
//! (default 32). Volume is `N^3`. Dense index order is `x + y*N + z*N^2` -
//! matches the existing engine's `Chunk::voxel_index` so the eventual
//! migration is a one-line bridge.
//!
//! ## Storage variants
//!
//! - **Uniform**: a single value, costs -`size_of::<T>()`.
//! - **Palette**: bit-packed `u16` indices into a `Vec<T>` palette.
//!   Auto-promotes from Uniform on first divergent write. Palette widths
//!   grow 1 -> 2 -> 4 -> 8 -> 16 bits as unique values accumulate.
//! - **Dense**: `Box<[T]>` of length `N^3`. Promoted to from Palette when
//!   the palette exceeds 65 536 entries, or explicitly via [`ChunkBuffer::to_dense`]
//!   for hot read paths.
//!
//! Use [`ChunkBuffer::try_collapse`] to walk back toward Uniform after bulk
//! deletes or overwrites.

#![warn(missing_docs)]

mod buffer;
mod error;
mod material;
mod material_registry;
mod neighbor;
mod palette;
mod shape;
mod voxel;

pub use buffer::{ChunkBuffer, StorageKind};
pub use error::{VoxelCoreError, VoxelCoreResult};
pub use material::MaterialId;
pub use material_registry::{MaterialDef, MaterialRegistry};
pub use neighbor::{NeighborView, BorderAxis};
pub use palette::{Palettable, PaletteStorage};
pub use shape::ShapeId;
pub use voxel::Voxel;

/// Edge length of a chunk along each axis — the single source of truth shared
/// by the engine and the evaluator.
pub const CHUNK_DIM: usize = 32;
