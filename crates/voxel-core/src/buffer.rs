//! [`ChunkBuffer`]: the generic three-variant chunk storage container.

use crate::error::{VoxelCoreError, VoxelCoreResult};
use crate::palette::{Palettable, PaletteStorage};

/// Reports which internal storage variant a [`ChunkBuffer`] is currently using.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum StorageKind {
    /// Single value covers the entire chunk.
    Uniform,
    /// Bit-packed palette.
    Palette,
    /// Full dense array of length `N^3`.
    Dense,
}

/// Generic chunk storage of `N^3` values of type `T`.
///
/// See the crate-level docs for the variant transition rules and indexing
/// convention.
#[derive(Clone)]
pub struct ChunkBuffer<T: Palettable, const N: usize = 32> {
    inner: Inner<T, N>,
}

#[derive(Clone)]
enum Inner<T: Palettable, const N: usize> {
    Uniform(T),
    Palette(PaletteStorage<T, N>),
    Dense(Box<[T]>),
}

impl<T: Palettable, const N: usize> ChunkBuffer<T, N> {
    /// Volume `N^3`.
    pub const VOLUME: usize = N * N * N;

    /// Edge length `N`.
    pub const EDGE: usize = N;

    /// Construct a uniform buffer filled with `value`.
    pub fn uniform(value: T) -> Self {
        Self { inner: Inner::Uniform(value) }
    }

    /// Construct a dense buffer from a flat slice. Returns
    /// [`VoxelCoreError::WrongLength`] if `data.len() != N^3`.
    pub fn from_dense(data: Box<[T]>) -> VoxelCoreResult<Self> {
        if data.len() != Self::VOLUME {
            return Err(VoxelCoreError::WrongLength {
                got: data.len(),
                expected: Self::VOLUME,
            });
        }
        Ok(Self { inner: Inner::Dense(data) })
    }

    /// Construct a palette-backed buffer from a flat dense slice.
    pub fn from_dense_palette(data: &[T]) -> VoxelCoreResult<Self> {
        if data.len() != Self::VOLUME {
            return Err(VoxelCoreError::WrongLength {
                got: data.len(),
                expected: Self::VOLUME,
            });
        }
        Ok(Self { inner: Inner::Palette(PaletteStorage::from_dense(data)) })
    }

    /// Report current storage variant.
    pub fn kind(&self) -> StorageKind {
        match &self.inner {
            Inner::Uniform(_) => StorageKind::Uniform,
            Inner::Palette(_) => StorageKind::Palette,
            Inner::Dense(_) => StorageKind::Dense,
        }
    }

    /// Flat index for local coords. Order: `x + y*N + z*N^2`.
    /// Matches `Chunk::voxel_index` in `voxulacrum-app`.
    #[inline]
    pub const fn index(x: usize, y: usize, z: usize) -> usize {
        x + y * N + z * N * N
    }

    /// Decode a flat index back into local `(x, y, z)`.
    #[inline]
    pub const fn decode(i: usize) -> (usize, usize, usize) {
        let x = i % N;
        let y = (i / N) % N;
        let z = i / (N * N);
        (x, y, z)
    }

    /// Read value at local coords. Bounds-checked in debug builds.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> T {
        debug_assert!(x < N && y < N && z < N);
        self.get_index(Self::index(x, y, z))
    }

    /// Read value at flat index.
    #[inline]
    pub fn get_index(&self, i: usize) -> T {
        debug_assert!(i < Self::VOLUME);
        match &self.inner {
            Inner::Uniform(v) => *v,
            Inner::Palette(p) => p.get_index(i),
            Inner::Dense(d) => d[i],
        }
    }

    /// Read at local signed coords. Returns `None` if out of `[0, N)`.
    #[inline]
    pub fn get_local(&self, x: i32, y: i32, z: i32) -> Option<T> {
        let n = N as i32;
        if x < 0 || y < 0 || z < 0 || x >= n || y >= n || z >= n {
            None
        } else {
            Some(self.get(x as usize, y as usize, z as usize))
        }
    }

    /// Read at signed coords, falling back to a caller-supplied closure for
    /// out-of-range coordinates. Use this for ad-hoc cross-chunk queries.
    /// For bulk border-sampling (meshing) use [`crate::NeighborView`] instead.
    #[inline]
    pub fn get_with_neighbors<F>(&self, x: i32, y: i32, z: i32, neighbor: F) -> T where
        F: FnOnce(glam::IVec3) -> T,
    {
        match self.get_local(x, y, z) {
            Some(v) => v,
            None => neighbor(glam::IVec3::new(x, y, z)),
        }
    }

    /// Write value at local coords, promoting storage variant as needed.
    pub fn set(&mut self, x: usize, y: usize, z: usize, value: T) {
        debug_assert!(x < N && y < N && z < N);
        self.set_index(Self::index(x, y, z), value);
    }

    /// Write at flat index, promoting storage variant as needed.
    pub fn set_index(&mut self, i: usize, value: T) {
        debug_assert!(i < Self::VOLUME);
        match &mut self.inner {
            Inner::Uniform(existing) if *existing == value => {
                // No-op write to the same value.
            }
            Inner::Uniform(existing) => {
                let prev = *existing;
                let mut palette = PaletteStorage::<T, N>::new(prev);
                palette.set_index(i, value);
                self.inner = Inner::Palette(palette);
            }
            Inner::Palette(p) => {
                p.set_index(i, value);
                // Promote palette -> dense if it exceeded the u16 index ceiling.
                if p.palette_len() > u16::MAX as usize {
                    let dense = p.to_dense().into_boxed_slice();
                    self.inner = Inner::Dense(dense);
                }
            }
            Inner::Dense(d) => {
                d[i] = value;
            }
        }
    }

    /// Materialize as a contiguous dense `Box<[T]>` of length `N^3`. If the
    /// buffer is already dense the box is returned by clone.
    pub fn to_dense(&self) -> Box<[T]> {
        match &self.inner {
            Inner::Uniform(v) => vec![*v; Self::VOLUME].into_boxed_slice(),
            Inner::Palette(p) => p.to_dense().into_boxed_slice(),
            Inner::Dense(d) => d.clone(),
        }
    }

    /// Force conversion to dense storage. No-op if already dense.
    pub fn make_dense(&mut self) {
        if matches!(self.inner, Inner::Dense(_)) {
            return;
        }
        let dense = self.to_dense();
        self.inner = Inner::Dense(dense);
    }

    /// Attempt to collapse back to [`StorageKind::Uniform`] if every entry
    /// is identical. Returns `true` if collapsed.
    pub fn try_collapse(&mut self) -> bool {
        let uniform_value = match &self.inner {
            Inner::Uniform(_) => return false,
            Inner::Palette(p) => p.try_uniform(),
            Inner::Dense(d) => {
                let first = d[0];
                if d.iter().all(|v| *v == first) { Some(first) } else { None }
            }
        };
        if let Some(v) = uniform_value {
            self.inner = Inner::Uniform(v);
            true
        } else {
            false
        }
    }

    /// Yield every voxel as `(x, y, z, value)`. Order is index order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, usize, usize, T)> + '_ {
        (0..Self::VOLUME).map(move |i| {
            let (x, y, z) = Self::decode(i);
            (x, y, z, self.get_index(i))
        })
    }

    /// Walk every voxel. Cheaper than `iter()` for Palette storage because
    /// the bit-shift constants and palette pointer get hoisted.
    pub fn for_each<F: FnMut(usize, usize, usize, T)>(&self, mut f: F) {
        match &self.inner {
            Inner::Uniform(v) => {
                let value = *v;
                for i in 0..Self::VOLUME {
                    let (x, y, z) = Self::decode(i);
                    f(x, y, z, value);
                }
            }
            Inner::Palette(p) => {
                p.for_each(|i, v| {
                    let (x, y, z) = Self::decode(i);
                    f(x, y, z, v);
                });
            }
            Inner::Dense(d) => {
                for (i, v) in d.iter().enumerate() {
                    let (x, y, z) = Self::decode(i);
                    f(x, y, z, *v);
                }
            }
        }
    }

    /// Estimate heap usage of the buffer's internal storage in bytes.
    pub fn memory_bytes(&self) -> usize {
        match &self.inner {
            Inner::Uniform(_) => 0,
            Inner::Palette(p) => p.memory_bytes(),
            Inner::Dense(d) => d.len() * std::mem::size_of::<T>(),
        }
    }
}

impl<T: Palettable + Default, const N: usize> Default for ChunkBuffer<T, N> {
    fn default() -> Self {
        Self::uniform(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Buf = ChunkBuffer<u16, 4>;

    #[test]
    fn index_decode_roundtrip() {
        for i in 0..Buf::VOLUME {
            let (x, y, z) = Buf::decode(i);
            assert_eq!(Buf::index(x, y, z), i);
        }
    }

    #[test]
    fn starts_uniform_and_reads_back() {
        let b = Buf::uniform(7);
        assert_eq!(b.kind(), StorageKind::Uniform);
        for (_, _, _, v) in b.iter() {
            assert_eq!(v, 7);
        }
    }

    #[test]
    fn first_divergent_write_promotes_to_palette() {
        let mut b = Buf::uniform(0);
        b.set(1, 2, 3, 42);
        assert_eq!(b.kind(), StorageKind::Palette);
        assert_eq!(b.get(1, 2, 3), 42);
        assert_eq!(b.get(0, 0, 0), 0);
    }

    #[test]
    fn collapse_back_to_uniform() {
        let mut b = Buf::uniform(0);
        b.set(0, 0, 0, 9);
        b.set(0, 0, 0, 0);
        assert!(b.try_collapse());
        assert_eq!(b.kind(), StorageKind::Uniform);
    }

    #[test]
    fn make_dense_preserves_values() {
        let mut b = Buf::uniform(3);
        b.set(2, 2, 2, 17);
        b.make_dense();
        assert_eq!(b.kind(), StorageKind::Dense);
        assert_eq!(b.get(2, 2, 2), 17);
        assert_eq!(b.get(0, 0, 0), 3);
    }

    #[test]
    fn wrong_length_dense_rejected() {
        let too_short = vec![0u16; 10].into_boxed_slice();
        assert!(matches!(
            Buf::from_dense(too_short),
            Err(VoxelCoreError::WrongLength { .. })
        ));
    }

    #[test]
    fn get_with_neighbors_falls_back() {
        let b = Buf::uniform(5u16);
        let v = b.get_with_neighbors(-1, 0, 0, |_| 99);
        assert_eq!(v, 99);
        let v = b.get_with_neighbors(0, 0, 0, |_| panic!("should not be called"));
        assert_eq!(v, 5);
    }
}
