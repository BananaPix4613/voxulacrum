//! Batched neighbor-chunk view for cross-chunk lookups.
//!
//! The single-shot closure form [`ChunkBuffer::get_with_neighbors`] is fine
//! for ad-hoc queries. Meshing walks every border voxel and would pay a heavy
//! closure-call cost - [`NeighborView`] holds the adjacent chunk references
//! once and offers direct indexed reads.

use crate::buffer::ChunkBuffer;
use crate::palette::Palettable;

/// Which axis a border lookup crossed. Used as a coarse classification by
/// callers that handle face/edge/corner cases separately.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BorderAxis {
    /// Inside the chunk; no border crossed.
    Inside,
    /// One axis crossed: a face neighbor.
    Face,
    /// Two axes crossed: an edge neighbor.
    Edge,
    /// Three axes crossed: a corner neighbor.
    Corner,
}

/// A read-only view over the 27 chunks centered on a self-chunk, indexed
/// in the same `(dx+1)*9 + (dy+1)*3 + (dz+1)` convention used by the
/// existing `ChunkNeighbors` in `voxulacrum-app/src/world/chunk.rs:103`.
///
/// Indices `[13]` is the self-chunk. Any neighbor slot may be `None` (e.g.
/// at world edges); the caller decides how to treat missing data - see
/// [`NeighborView::get_or`].
pub struct NeighborView<'a, T: Palettable, const N: usize = 32> {
    /// 27 entries; `chunks[13]` must be `Some(self)`. `Option` so world
    /// edges and yet-to-load chunks are representable.
    pub chunks: [Option<&'a ChunkBuffer<T, N>>; 27],
}

impl<'a, T: Palettable, const N: usize> NeighborView<'a, T, N> {
    /// Construct from a `self` buffer and 26 optional neighbors.
    /// `self_chunk` is placed at slot 13.
    pub fn new(self_chunk: &'a ChunkBuffer<T, N>) -> Self {
        let mut chunks: [Option<&'a ChunkBuffer<T, N>>; 27] = [None; 27];
        chunks[13] = Some(self_chunk);
        Self { chunks }
    }
    
    /// Set a neighbor in the `(dx, dy, dz) ∈ {-1, 0, 1}³` slot.
    pub fn set(&mut self, dx: i32, dy: i32, dz: i32, chunk: Option<&'a ChunkBuffer<T, N>>) {
        let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
        self.chunks[idx] = chunk;
    }
    
    /// Look up a voxel at signed local coords `(x, y, z)` that may lie in a
    /// neighboring chunk. Returns `None` if the target neighbor is absent.
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<T> {
        let n = N as i32;
        let (cx, lx) = wrap(x, n);
        let (cy, ly) = wrap(y, n);
        let (cz, lz) = wrap(z, n);
        let idx = ((cx + 1) * 9 + (cy + 1) * 3 + (cz + 1)) as usize;
        self.chunks[idx].map(|buf| buf.get(lx as usize, ly as usize, lz as usize))
    }
    
    /// Look up with a fallback value for missing neighbors.
    pub fn get_or(&self, x: i32, y: i32, z: i32, fallback: T) -> T {
        self.get(x, y, z).unwrap_or(fallback)
    }
    
    /// Classify a coordinate by how many axes leave `[0, N)`.
    pub fn classify(x: i32, y: i32, z: i32) -> BorderAxis {
        let n = N as i32;
        let crossed = [x, y, z]
            .iter()
            .filter(|&&v| v < 0 || v >= n)
            .count();
        match crossed {
            0 => BorderAxis::Inside,
            1 => BorderAxis::Face,
            2 => BorderAxis::Edge,
            _ => BorderAxis::Corner,
        }
    }
}

/// Decompose a signed local coord into `(neighbor_offset_in_-1..=1, local_in_0..N)`.
#[inline]
fn wrap(v: i32, n: i32) -> (i32, i32) {
    if v < 0 { (-1, v + n) }
    else if v >= n { (1, v - n) }
    else { (0, v) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::ChunkBuffer;
    
    #[test]
    fn inside_lookup_uses_self_chunk() {
        let mut me: ChunkBuffer<u16, 4> = ChunkBuffer::uniform(1);
        me.set(2, 2, 2, 7);
        let view = NeighborView::new(&me);
        assert_eq!(view.get(2, 2, 2), Some(7));
    }
    
    #[test]
    fn missing_neighbor_returns_none() {
        let me: ChunkBuffer<u16, 4> = ChunkBuffer::uniform(1);
        let view = NeighborView::new(&me);
        assert_eq!(view.get(-1, 0, 0), None);
        assert_eq!(view.get_or(-1, 0, 0, 99), 99);
    }
    
    #[test]
    fn face_neighbor_resolves() {
        let me: ChunkBuffer<u16, 4> = ChunkBuffer::uniform(1);
        let east: ChunkBuffer<u16, 4> = ChunkBuffer::uniform(2);
        let mut view = NeighborView::new(&me);
        view.set(1, 0, 0, Some(&east));
        // x=4 wraps to (dx=1, 1x=0) of the east neighbor.
        assert_eq!(view.get(4, 2, 2), Some(2));
    }
    
    #[test]
    fn classify_axes() {
        type V<'a> = NeighborView<'a, u16, 4>;
        assert_eq!(V::classify(0, 0, 0), BorderAxis::Inside);
        assert_eq!(V::classify(-1, 0, 0), BorderAxis::Face);
        assert_eq!(V::classify(-1, -1, 0), BorderAxis::Edge);
        assert_eq!(V::classify(-1, -1, -1), BorderAxis::Corner);
    }
}
