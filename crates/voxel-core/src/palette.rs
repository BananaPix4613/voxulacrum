//! Generic palette-compressed storage.
//!
//! Stores values of type `T` as `u16` indices into a `Vec<T>` palette,
//! with the indices bit-packed (1, 2, 4, 8, or 16 bits per entry). Entries
//! do not span `u64` word boundaries - this matches Minecraft's post-1.16
//! format and the existing `PalettedBitArray` in `voxulacrum-app`.

use std::hash::Hash;

/// Trait for types storable in a [`PaletteStorage`].
///
/// Implemented for any `Copy + Eq + Hash + 'static` by request - but kept
/// as an explicit trait (rather than a blanket impl) so we can layer extra
/// bounds later (e.g. `Default`, `bytemuck::Pod`) without a breaking charge.
pub trait Palettable: Copy + Eq + Hash + 'static {}

impl Palettable for u8 {}
impl Palettable for u16 {}
impl Palettable for u32 {}
impl Palettable for i32 {}
impl Palettable for crate::Voxel {}
impl Palettable for crate::MaterialId {}

/// Palette-compressed storage of `N^3` values.
#[derive(Clone)]
pub struct PaletteStorage<T: Palettable, const N: usize> {
    palette: Vec<T>,
    bits_per_entry: u8,
    data: Vec<u64>,
}

impl<T: Palettable, const N: usize> PaletteStorage<T, N> {
    /// Volume of the buffer (`N^3`).
    pub const VOLUME: usize = N * N * N;

    /// Create a single-entry palette filled with `default_value`.
    pub fn new(default_value: T) -> Self {
        let bits_per_entry = 1u8;
        let entries_per_word = 64 / bits_per_entry as usize;
        let word_count = (Self::VOLUME + entries_per_word - 1) / entries_per_word;
        Self {
            palette: vec![default_value],
            bits_per_entry,
            data: vec![0u64; word_count], // all index 0 = default_value
        }
    }

    /// Build from a flat dense slice of length `N^3`.
    pub fn from_dense(values: &[T]) -> Self {
        debug_assert_eq!(values.len(), Self::VOLUME);

        let mut palette: Vec<T> = Vec::new();
        for &v in values.iter() {
            if !palette.contains(&v) {
                palette.push(v);
                if palette.len() >= u16::MAX as usize {
                    break;
                }
            }
        }

        let bits_per_entry = bits_for_palette_size(palette.len());
        let entries_per_word = 64 / bits_per_entry as usize;
        let word_count = (Self::VOLUME + entries_per_word - 1) / entries_per_word;
        let mut data = vec![0u64; word_count];

        for (i, &v) in values.iter().enumerate() {
            let palette_idx = palette.iter().position(|p| *p == v).expect(
                "palette must contain every value seen during dense construction",
            );
            let word_index = i / entries_per_word;
            let bit_offset = (i % entries_per_word) * bits_per_entry as usize;
            data[word_index] |= (palette_idx as u64) << bit_offset;
        }

        Self { palette, bits_per_entry, data }
    }

    /// Get the value at flat index `i ∈ [0, N³)`.
    #[inline]
    pub fn get_index(&self, i: usize) -> T {
        debug_assert!(i < Self::VOLUME);
        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let word_index = i / entries_per_word;
        let bit_offset = (i % entries_per_word) * bits;
        let mask = (1u64 << bits) - 1;
        let palette_index = ((self.data[word_index] >> bit_offset) & mask) as usize;
        self.palette[palette_index]
    }

    /// Set the value at flat index `i ∈ [0, N³)`, growing the palette if needed.
    ///
    /// Returns `true` if the palette grew its bit width as a result.
    pub fn set_index(&mut self, i: usize, value: T) -> bool {
        debug_assert!(i < Self::VOLUME);

        let palette_idx = match self.palette.iter().position(|p| *p == value) {
            Some(idx) => idx,
            None => {
                self.palette.push(value);
                self.palette.len() - 1
            }
        };

        let grew = if self.palette.len() > (1usize << self.bits_per_entry) {
            self.grow();
            true
        } else {
            false
        };

        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let word_index = i / entries_per_word;
        let bit_offset = (i % entries_per_word) * bits;
        let mask = (1u64 << bits) - 1;
        self.data[word_index] =
            (self.data[word_index] & !(mask << bit_offset))
            | ((palette_idx as u64) << bit_offset);

        grew
    }

    /// Number of distinct entries currently in the palette.
    #[inline]
    pub fn palette_len(&self) -> usize { self.palette.len() }

    /// Bits per entry currently in use (1, 2, 4, 8, or 16).
    #[inline]
    pub fn bits_per_entry(&self) -> u8 { self.bits_per_entry }

    /// Iterate over (flat_index, value) pairs in storage order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, T)> + '_ {
        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let mask = (1u64 << bits) - 1;
        let palette = &self.palette;
        let data = &self.data;
        (0..Self::VOLUME).map(move |i| {
            let word_index = i / entries_per_word;
            let bit_offset = (i % entries_per_word) * bits;
            let palette_index = ((data[word_index] >> bit_offset) & mask) as usize;
            (i, palette[palette_index])
        })
    }

    /// Walk every entry. Faster than `iter()` for `T = Voxel`-sized types
    /// because the palette reference and bit-shift constants get hoisted.
    pub fn for_each<F: FnMut(usize, T)>(&self, mut f: F) {
        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let mask = (1u64 << bits) - 1;
        let palette = &self.palette;
        let data = &self.data;
        for i in 0..Self::VOLUME {
            let word_index = i / entries_per_word;
            let bit_offset = (i % entries_per_word) * bits;
            let palette_index = ((data[word_index] >> bit_offset) & mask) as usize;
            f(i, palette[palette_index]);
        }
    }

    /// Materialize into a dense `Vec<T>` of length `N^3`.
    pub fn to_dense(&self) -> Vec<T> {
        let mut out = Vec::with_capacity(Self::VOLUME);
        self.for_each(|_, v| out.push(v));
        out
    }

    /// If every entry is the same value, return it (for collapse back to Uniform).
    pub fn try_uniform(&self) -> Option<T> {
        if self.palette.len() == 1 {
            return Some(self.palette[0]);
        }
        // Multiple palette entries but possibly only one actually referenced.
        let first = self.get_index(0);
        for i in 1..Self::VOLUME {
            if self.get_index(i) != first {
                return None;
            }
        }
        Some(first)
    }

    /// Heap memory usage in bytes (excluding the struct header itself).
    pub fn memory_bytes(&self) -> usize {
        self.palette.len() * std::mem::size_of::<T>()
            + self.data.len() * std::mem::size_of::<u64>()
    }

    // --- internal ---

    fn grow(&mut self) {
        let old_bits = self.bits_per_entry as usize;
        let new_bits = next_power_bits(self.bits_per_entry);
        let old_entries_per_word = 64 / old_bits;
        let new_entries_per_word = 64 / new_bits as usize;
        let new_word_count =
            (Self::VOLUME + new_entries_per_word - 1) / new_entries_per_word;
        let mut new_data = vec![0u64; new_word_count];

        let old_mask = (1u64 << old_bits) - 1;

        for i in 0..Self::VOLUME {
            let old_word = i / old_entries_per_word;
            let old_offset = (i % old_entries_per_word) * old_bits;
            let palette_idx = (self.data[old_word] >> old_offset) & old_mask;

            let new_word = i / new_entries_per_word;
            let new_offset = (i % new_entries_per_word) * new_bits as usize;
            new_data[new_word] |= palette_idx << new_offset;
        }

        self.data = new_data;
        self.bits_per_entry = new_bits;
    }
}

/// Minimum `bits_per_entry` to hold `size` distinct palette entries.
fn bits_for_palette_size(size: usize) -> u8 {
    if size <= 2 {
        return 1;
    }
    // ceil(log2(size)) using integer math
    let bits = (u64::BITS - ((size - 1) as u64).leading_zeros()) as u8;
    // Round up to next supported width (1, 2, 4, 8, 16).
    match bits {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        5..=8 => 8,
        _ => 16,
    }
}

fn next_power_bits(current: u8) -> u8 {
    match current {
        1 => 2,
        2 => 4,
        4 => 8,
        8 => 16,
        _ => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_value_palette() {
        let p: PaletteStorage<u16, 4> = PaletteStorage::new(42);
        for i in 0..PaletteStorage::<u16, 4>::VOLUME {
            assert_eq!(p.get_index(i), 42);
        }
        assert_eq!(p.palette_len(), 1);
        assert_eq!(p.bits_per_entry(), 1);
    }
    
    #[test]
    fn grow_through_widths() {
        let mut p: PaletteStorage<u16, 4> = PaletteStorage::new(0);
        // Insert distinct values one by one; record bit-width growth.
        let widths: Vec<u8> = (1..=20)
            .map(|v| {
                p.set_index(v as usize, v);
                p.bits_per_entry()
            })
            .collect();
        // We should see widths grow through 2, 4, 8.
        assert!(widths.contains(&2));
        assert!(widths.contains(&4));
        assert!(widths.contains(&8));
    }
    
    #[test]
    fn try_uniform_after_overwrite() {
        let mut p: PaletteStorage<u16, 4> = PaletteStorage::new(7);
        // Introduce a divergent value then overwrite back to uniform.
        p.set_index(0, 9);
        assert_eq!(p.try_uniform(), None);
        p.set_index(0, 7);
        assert_eq!(p.try_uniform(), Some(7));
    }
}
