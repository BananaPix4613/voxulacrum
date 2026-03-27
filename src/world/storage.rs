//! Chunk storage: SoA layout with Uniform/Populated variants and palette compression.

use serde_json::json;
use super::chunk::{CHUNK_SIZE, CHUNK_VOLUME};
use super::voxel::MAT_AIR;

// ============================================================================
// PalettedBitArray
// ============================================================================

/// Minecraft-style palette + bit-packed index array for categorical voxel fields.
///
/// Indices do NOT span u64 word boundaries: each u64 holds `floor(64 / bits_per_entry)`
/// entries, with unused high bits wasted. This matches Minecraft's post-1.16 format
/// and avoids cross-word bit manipulation.
#[derive(Clone)]
pub struct PalettedBitArray {
    palette: Vec<u16>,
    bits_per_entry: u8,
    data: Vec<u64>,
}

impl PalettedBitArray {
    /// Create a single-entry palette filled with `default_value`.
    pub fn new(default_value: u16) -> Self {
        let bits_per_entry = 1u8;
        let entries_per_word = 64 / bits_per_entry as usize;
        let word_count = (CHUNK_VOLUME + entries_per_word - 1) / entries_per_word;
        Self {
            palette: vec![default_value],
            bits_per_entry,
            data: vec![0u64; word_count], // all index 0 = default_value
        }
    }

    /// Bulk construction from a flat array. Scans for unique values, builds palette, packs indices.
    pub fn from_raw(values: &[u16; CHUNK_VOLUME]) -> Self {
        // Collect unique values preserving insertion order
        let mut palette: Vec<u16> = Vec::new();
        for &v in values.iter() {
            if !palette.contains(&v) {
                palette.push(v);
            }
        }

        let bits_per_entry = bits_for_palette_size(palette.len());
        let entries_per_word = 64 / bits_per_entry as usize;
        let word_count = (CHUNK_VOLUME + entries_per_word - 1) / entries_per_word;
        let mut data = vec![0u64; word_count];

        for (i, &v) in values.iter().enumerate() {
            let palette_idx = palette.iter().position(|&p| p == v).unwrap();
            let word_index = i / entries_per_word;
            let bit_offset = (i % entries_per_word) * bits_per_entry as usize;
            data[word_index] |= (palette_idx as u64) << bit_offset;
        }

        Self {
            palette,
            bits_per_entry,
            data,
        }
    }

    /// Read the actual material ID at voxel index.
    #[inline]
    pub fn get(&self, index: usize) -> u16 {
        debug_assert!(index < CHUNK_VOLUME);
        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let word_index = index / entries_per_word;
        let bit_offset = (index % entries_per_word) * bits;
        let mask = (1u64 << bits) - 1;
        let palette_index = ((self.data[word_index] >> bit_offset) & mask) as usize;
        self.palette[palette_index]
    }

    /// Set a value at voxel index, auto-growing the palette if needed.
    pub fn set(&mut self, index: usize, value: u16) {
        debug_assert!(index < CHUNK_VOLUME);

        // Find or insert into palette
        let palette_idx = match self.palette.iter().position(|&v| v == value) {
            Some(idx) => idx,
            None => {
                self.palette.push(value);
                let new_idx = self.palette.len() - 1;
                // Grow if palette exceeds current bit capacity
                if self.palette.len() > (1usize << self.bits_per_entry) {
                    self.grow();
                }
                new_idx
            }
        };

        let bits = self.bits_per_entry as usize;
        let entries_per_word = 64 / bits;
        let word_index = index / entries_per_word;
        let bit_offset = (index % entries_per_word) * bits;
        let mask = (1u64 << bits) - 1;
        self.data[word_index] =
            (self.data[word_index] & !(mask << bit_offset)) | ((palette_idx as u64) << bit_offset);
    }

    /// Number of entries (always CHUNK_VOLUME).
    pub fn len(&self) -> usize {
        CHUNK_VOLUME
    }

    /// Total heap memory usage in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.palette.len() * std::mem::size_of::<u16>()
            + self.data.len() * std::mem::size_of::<u64>()
            + std::mem::size_of::<Self>()
    }

    /// Number of distinct values in the palette.
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// Serialize internal state: [palette_len:u16][palette:u16...][bits_per_entry:u8][word_count:u32][data:u64...]
    pub fn serialize_to_bytes(&self, buf: &mut Vec<u8>) {
        let palette_len = self.palette.len() as u16;
        buf.extend_from_slice(&palette_len.to_le_bytes());
        for &val in &self.palette {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.push(self.bits_per_entry);
        let word_count = self.data.len() as u32;
        buf.extend_from_slice(&word_count.to_le_bytes());
        for &word in &self.data {
            buf.extend_from_slice(&word.to_le_bytes());
        }
    }

    /// Deserialize from bytes at `offset`. Returns `(Self, new_offset)`.
    pub fn deserialize_from_bytes(bytes: &[u8], mut pos: usize) -> Option<(Self, usize)> {
        if pos + 2 > bytes.len() { return None; }
        let palette_len = u16::from_le_bytes(bytes[pos..pos+2].try_into().ok()?) as usize;
        pos += 2;

        if pos + palette_len * 2 > bytes.len() { return None; }
        let mut palette = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            palette.push(u16::from_le_bytes(bytes[pos..pos+2].try_into().ok()?));
            pos += 2;
        }

        if pos + 1 > bytes.len() { return None; }
        let bits_per_entry = bytes[pos];
        pos += 1;

        if pos + 4 > bytes.len() { return None; }
        let word_count = u32::from_le_bytes(bytes[pos..pos+4].try_into().ok()?) as usize;
        pos += 4;

        if pos + word_count * 8 > bytes.len() { return None; }
        let mut data = Vec::with_capacity(word_count);
        for _ in 0..word_count {
            data.push(u64::from_le_bytes(bytes[pos..pos+8].try_into().ok()?));
            pos += 8;
        }

        Some((Self { palette, bits_per_entry, data }, pos))
    }

    /// Re-encode all entries with a wider bit width.
    fn grow(&mut self) {
        let old_bits = self.bits_per_entry as usize;
        let new_bits = next_power_bits(self.bits_per_entry);
        let old_entries_per_word = 64 / old_bits;
        let new_entries_per_word = 64 / new_bits as usize;
        let new_word_count = (CHUNK_VOLUME + new_entries_per_word - 1) / new_entries_per_word;
        let mut new_data = vec![0u64; new_word_count];

        let old_mask = (1u64 << old_bits) - 1;

        for i in 0..CHUNK_VOLUME {
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

/// Minimum bits_per_entry to hold `size` distinct palette entries.
fn bits_for_palette_size(size: usize) -> u8 {
    if size <= 2 {
        return 1;
    }
    // ceil(log2(size)) using integer math
    (u64::BITS - ((size - 1) as u64).leading_zeros()) as u8
}

/// Next power-of-two bit width for palette growth.
fn next_power_bits(current: u8) -> u8 {
    match current {
        1 => 2,
        2 => 4,
        4 => 8,
        8 => 16,
        _ => 16,
    }
}

// ============================================================================
// ChunkStorage
// ============================================================================

/// Two-variant chunk storage. Uniform chunks cost ~8 bytes.
/// Populated chunks use SoA with palette compression on categorical fields.
#[derive(Clone)]
pub enum ChunkStorage {
    /// Every voxel is identical. Near-zero memory.
    Uniform { density: i8, material_id: u16 },
    /// Heterogeneous chunk with struct-of-arrays layout.
    Populated(Box<PopulatedChunk>),
}

#[derive(Clone)]
pub struct LightingData {
    pub light_sun: Box<[u8; CHUNK_VOLUME]>,
    pub light_emit: Box<[u8; CHUNK_VOLUME]>,
}

#[derive(Clone)]
pub struct SimulationData {
    pub moisture: Box<[u8; CHUNK_VOLUME]>,
    pub temperature: Box<[u8; CHUNK_VOLUME]>,
}

#[derive(Clone)]
pub struct FloraData {
    pub flora_id: PalettedBitArray,
    pub flora_growth: Box<[u8; CHUNK_VOLUME]>,
    pub hidden_flags: Box<[u8; CHUNK_VOLUME]>,
}

/// SoA layout - only Tier 1 (hot) fields in this phase.
/// Tiers 2-4 (lighting, simulation, flora) are added in Phase 3.
#[derive(Clone)]
pub struct PopulatedChunk {
    // Tier 1: Hot (always present)
    pub density: Box<[i8; CHUNK_VOLUME]>,
    pub material_id: PalettedBitArray,
    // Tier 2: Warm (lighting)
    pub lighting: Option<Box<LightingData>>,
    // Tier 3: Cold (simulation)
    pub simulation: Option<Box<SimulationData>>,
    // Tier 4: Sparse (flora)
    pub flora: Option<Box<FloraData>>,
}

impl ChunkStorage {
    /// Create a uniform air chunk.
    pub fn new_air() -> Self {
        ChunkStorage::Uniform {
            density: 0,
            material_id: MAT_AIR,
        }
    }

    /// Read density at a flat index (x + y*32 + z*32*32).
    #[inline]
    pub fn density(&self, index: usize) -> i8 {
        match self {
            ChunkStorage::Uniform { density, .. } => *density,
            ChunkStorage::Populated(p) => p.density[index],
        }
    }

    /// Read material at a flat index.
    #[inline]
    pub fn material(&self, index: usize) -> u16 {
        match self {
            ChunkStorage::Uniform { material_id, .. } => *material_id,
            ChunkStorage::Populated(p) => p.material_id.get(index),
        }
    }

    pub fn simulation(&self) -> Option<&SimulationData> {
        match self {
            ChunkStorage::Populated(p) => p.simulation(),
            ChunkStorage::Uniform { .. } => None,
        }
    }

    /// Write density at a flat index. Promotes Uniform -> Populated if needed.
    pub fn set_density(&mut self, index: usize, value: i8) {
        self.ensure_populated();
        if let ChunkStorage::Populated(p) = self {
            p.density[index] = value;
        }
    }

    /// Write material at a flat index. Promotes Uniform -> Populated if needed.
    pub fn set_material(&mut self, index: usize, value: u16) {
        self.ensure_populated();
        if let ChunkStorage::Populated(p) = self {
            p.material_id.set(index, value);
        }
    }

    /// Write both density and material (common in generation).
    pub fn set_voxel(&mut self, index: usize, density: i8, material: u16) {
        self.ensure_populated();
        if let ChunkStorage::Populated(p) = self {
            p.density[index] = density;
            p.material_id.set(index, material);
        }
    }

    /// Check if this chunk is uniform.
    pub fn is_uniform(&self) -> bool {
        matches!(self, ChunkStorage::Uniform { .. })
    }

    /// Try to collapse a Populated chunk back to Uniform if all values match.
    pub fn try_collapse(&mut self) {
        if let ChunkStorage::Populated(p) = self {
            let first_d = p.density[0];
            let first_m = p.material_id.get(0);
            let all_same = p.density.iter().all(|&d| d == first_d)
                && (0..CHUNK_VOLUME).all(|i| p.material_id.get(i) == first_m);
            if all_same {
                *self = ChunkStorage::Uniform {
                    density: first_d,
                    material_id: first_m,
                };
            }
        }
    }

    /// Get the raw density slice for cache-optimal meshing reads.
    /// For Uniform, returns None (caller fills a local buffer with the constant).
    pub fn density_slice(&self) -> Option<&[i8; CHUNK_VOLUME]> {
        match self {
            ChunkStorage::Populated(p) => Some(&p.density),
            ChunkStorage::Uniform { .. } => None,
        }
    }

    /// Total memory usage in bytes (for diagnostics).
    pub fn memory_bytes(&self) -> usize {
        match self {
            ChunkStorage::Uniform { .. } => std::mem::size_of::<Self>(),
            ChunkStorage::Populated(p) => {
                std::mem::size_of::<Self>() + p.memory_bytes()
            }
        }
    }

    /// Promote from Uniform to Populated, filling arrays with the uniform values.
    pub(crate) fn ensure_populated(&mut self) {
        if let ChunkStorage::Uniform {
            density,
            material_id,
        } = *self
        {
            let density_arr = {
                let mut arr = vec![density; CHUNK_VOLUME].into_boxed_slice();
                // SAFETY: Vec guarantees length == CHUNK_VOLUME
                unsafe {
                    Box::from_raw(Box::into_raw(arr) as *mut [i8; CHUNK_VOLUME])
                }
            };
            *self = ChunkStorage::Populated(Box::new(PopulatedChunk {
                density: density_arr,
                material_id: PalettedBitArray::new(material_id),
                lighting: None,
                simulation: None,
                flora: None,
            }));
        }
    }
}

impl PopulatedChunk {
    pub fn lighting_mut(&mut self) -> &mut LightingData {
        self.lighting.get_or_insert_with(|| Box::new(LightingData {
            light_sun: zeroed_u8_box(),
            light_emit: zeroed_u8_box(),
        }))
    }

    pub fn simulation_mut(&mut self) -> &mut SimulationData {
        self.simulation.get_or_insert_with(|| Box::new(SimulationData {
            moisture: zeroed_u8_box(),
            temperature: zeroed_u8_box(),
        }))
    }

    pub fn flora_mut(&mut self) -> &mut FloraData {
        self.flora.get_or_insert_with(|| Box::new(FloraData {
            flora_id: PalettedBitArray::new(0),
            flora_growth: zeroed_u8_box(),
            hidden_flags: zeroed_u8_box(),
        }))
    }

    pub fn lighting(&self) -> Option<&LightingData> { self.lighting.as_deref() }
    pub fn simulation(&self) -> Option<&SimulationData> { self.simulation.as_deref() }
    pub fn flora(&self) -> Option<&FloraData> { self.flora.as_deref() }

    pub fn memory_bytes(&self) -> usize {
        let mut total = CHUNK_VOLUME + self.material_id.memory_bytes();
        if let Some(ref l) = self.lighting {
            total += std::mem::size_of_val(l.as_ref());
        }
        if let Some(ref s) = self.simulation {
            total += std::mem::size_of_val(s.as_ref());
        }
        if let Some(ref f) = self.flora {
            total += f.flora_id.memory_bytes() + CHUNK_VOLUME * 2;
        }
        total
    }
}

fn zeroed_u8_box() -> Box<[u8; CHUNK_VOLUME]> {
    unsafe {
        let v: Vec<u8> = vec![0u8; CHUNK_VOLUME];
        let boxed_slice = v.into_boxed_slice();
        Box::from_raw(Box::into_raw(boxed_slice) as *mut [u8; CHUNK_VOLUME])
    }
}

/// Build a ChunkStorage from separate density and material arrays.
/// Checks for uniformity and returns Uniform when possible.
pub fn storage_from_arrays(
    density: Box<[i8; CHUNK_VOLUME]>,
    material: &[u16; CHUNK_VOLUME],
) -> ChunkStorage {
    // Check uniformity
    let first_d = density[0];
    let first_m = material[0];
    let is_uniform = density.iter().all(|&d| d == first_d)
        && material.iter().all(|&m| m == first_m);

    if is_uniform {
        ChunkStorage::Uniform {
            density: first_d,
            material_id: first_m,
        }
    } else {
        ChunkStorage::Populated(Box::new(PopulatedChunk {
            density,
            material_id: PalettedBitArray::from_raw(material),
            lighting: None,
            simulation: None,
            flora: None,
        }))
    }
}

pub fn storage_from_arrays_with_moisture(
    density: Box<[i8; CHUNK_VOLUME]>,
    material: &[u16; CHUNK_VOLUME],
    moisture: &[u8; CHUNK_VOLUME],
) -> ChunkStorage {
    // Check uniformity for density+material (same as before)
    let first_d = density[0];
    let first_m = material[0];
    let is_uniform = density.iter().all(|&d| d == first_d)
        && material.iter().all(|&m| m == first_m);

    if is_uniform {
        // Uniform chunks don't store moisture (regenerated from noise)
        ChunkStorage::Uniform { density: first_d, material_id: first_m }
    } else {
        // Check if moisture has any non-zero values
        let has_moisture = moisture.iter().any(|&m| m != 0);
        let simulation = if has_moisture {
            let mut moisture_box = zeroed_u8_box();
            moisture_box.copy_from_slice(moisture);
            Some(Box::new(SimulationData {
                moisture: moisture_box,
                temperature: zeroed_u8_box(),
            }))
        } else {
            None
        };

        ChunkStorage::Populated(Box::new(PopulatedChunk {
            density,
            material_id: PalettedBitArray::from_raw(material),
            lighting: None,
            simulation,
            flora: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_single_value() {
        let p = PalettedBitArray::new(7);
        for i in 0..CHUNK_VOLUME {
            assert_eq!(p.get(i), 7);
        }
    }

    #[test]
    fn palette_from_raw_roundtrip() {
        let mut raw = [0u16; CHUNK_VOLUME];
        for i in 0..CHUNK_VOLUME {
            raw[i] = (i % 9) as u16; // 9 materials
        }
        let p = PalettedBitArray::from_raw(&raw);
        for i in 0..CHUNK_VOLUME {
            assert_eq!(p.get(i), (i % 9) as u16, "mismatch at index {}", i);
        }
        assert_eq!(p.palette_len(), 9);
    }

    #[test]
    fn palette_set_grows() {
        let mut p = PalettedBitArray::new(0);
        // Add values 0..3 (needs 2 bits)
        p.set(0, 1);
        p.set(1, 2);
        p.set(2, 3);
        assert_eq!(p.get(0), 1);
        assert_eq!(p.get(1), 2);
        assert_eq!(p.get(2), 3);
        assert_eq!(p.get(3), 0); // unchanged default
    }

    #[test]
    fn storage_uniform_access() {
        let s = ChunkStorage::Uniform {
            density: 42,
            material_id: 3,
        };
        assert_eq!(s.density(0), 42);
        assert_eq!(s.density(16384), 42);
        assert_eq!(s.material(0), 3);
        assert!(s.is_uniform());
    }

    #[test]
    fn storage_promote_and_collapse() {
        let mut s = ChunkStorage::Uniform {
            density: 10,
            material_id: 5,
        };
        s.set_density(100, 20);
        assert!(!s.is_uniform());
        assert_eq!(s.density(100), 20);
        assert_eq!(s.density(0), 10); // other voxels unchanged

        // Set it back
        s.set_density(100, 10);
        s.try_collapse();
        assert!(s.is_uniform());
    }

    #[test]
    fn bits_for_palette() {
        assert_eq!(bits_for_palette_size(1), 1);
        assert_eq!(bits_for_palette_size(2), 1);
        assert_eq!(bits_for_palette_size(3), 2);
        assert_eq!(bits_for_palette_size(4), 2);
        assert_eq!(bits_for_palette_size(5), 3);
        assert_eq!(bits_for_palette_size(9), 4);
        assert_eq!(bits_for_palette_size(16), 4);
        assert_eq!(bits_for_palette_size(17), 5);
    }
}