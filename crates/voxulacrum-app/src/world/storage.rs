//! Chunk storage: SoA layout with Uniform/Populated variants and palette compression.

use super::chunk::CHUNK_VOLUME;
use voxel_core::Voxel;

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
    palette: Vec<u32>,   // packed Voxel (Voxel::pack)
    bits_per_entry: u8,
    data: Vec<u64>,
}

impl PalettedBitArray {
    /// Create a single-entry palette filled with `default_value`.
    pub fn new(default_value: u32) -> Self {
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
    pub fn from_raw(values: &[u32; CHUNK_VOLUME]) -> Self {
        // Collect unique values preserving insertion order
        let mut palette: Vec<u32> = Vec::new();
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
    pub fn get(&self, index: usize) -> u32 {
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
    pub fn set(&mut self, index: usize, value: u32) {
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
        self.palette.len() * std::mem::size_of::<u32>()
            + self.data.len() * std::mem::size_of::<u64>()
            + std::mem::size_of::<Self>()
    }

    /// Number of distinct values in the palette.
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// Serialize internal state: [palette_len:u16][palette:u32...][bits_per_entry:u8][word_count:u32][data:u64...]
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
        // palette_len is written as a u16 by serialize_to_bytes; read the same width.
        let palette_len = u16::from_le_bytes(bytes[pos..pos+2].try_into().ok()?) as usize;
        pos += 2;

        if pos + palette_len * 4 > bytes.len() { return None; }
        let mut palette = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            palette.push(u32::from_le_bytes(bytes[pos..pos+4].try_into().ok()?));
            pos += 4;
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
    Uniform { voxel: Voxel },
    /// Heterogeneous chunk with struct-of-arrays layout.
    Populated(Box<PopulatedChunk>),
}

/// Material-only hot tier. All sidecar layers (lighting, fluids, detail,
/// scatter, decals) live on the data-model `Chunk`, not in storage.
#[derive(Clone)]
pub struct PopulatedChunk {
    pub material_id: PalettedBitArray,
}

impl ChunkStorage {
    /// Create a uniform air chunk.
    pub fn new_air() -> Self {
        ChunkStorage::Uniform { voxel: Voxel::EMPTY }
    }

    /// Read material at a flat index.
    #[inline]
    pub fn voxel(&self, index: usize) -> Voxel {
        match self {
            ChunkStorage::Uniform { voxel } => *voxel,
            ChunkStorage::Populated(p) => Voxel::unpack(p.material_id.get(index))
                .unwrap_or(Voxel::EMPTY),
        }
    }

    /// Write voxel at a flat index. Promotes Uniform -> Populated if needed.
    pub fn set_voxel(&mut self, index: usize, value: Voxel) {
        self.ensure_populated();
        if let ChunkStorage::Populated(p) = self {
            p.material_id.set(index, value.pack());
        }
    }

    /// Check if this chunk is uniform.
    pub fn is_uniform(&self) -> bool {
        matches!(self, ChunkStorage::Uniform { .. })
    }

    /// Check if a voxel is solid
    #[inline]
    pub fn is_solid(&self, index: usize) -> bool {
        self.voxel(index).is_solid()
    }
    
    /// Try to collapse a Populated chunk back to Uniform if all values match.
    pub fn try_collapse(&mut self) {
        if let ChunkStorage::Populated(p) = self {
            let first_m = p.material_id.get(0);
            if (0..CHUNK_VOLUME).all(|i| p.material_id.get(i) == first_m) {
                *self = ChunkStorage::Uniform {
                    voxel: Voxel::unpack(first_m).unwrap_or(Voxel::EMPTY),
                };
            }
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
        if let ChunkStorage::Uniform { voxel } = *self {
            *self = ChunkStorage::Populated(Box::new(PopulatedChunk {
                material_id: PalettedBitArray::new(voxel.pack()),
            }));
        }
    }
}

impl PopulatedChunk {
    pub fn memory_bytes(&self) -> usize {
        self.material_id.memory_bytes()
    }
}

/// Build a ChunkStorage from a flat voxel array.
/// Checks for uniformity and returns Uniform when possible.
pub fn storage_from_arrays(voxels: &[Voxel; CHUNK_VOLUME]) -> ChunkStorage {
    let first = voxels[0];
    if voxels.iter().all(|&v| v == first) {
        ChunkStorage::Uniform { voxel: first }
    } else {
        let packed: Box<[u32; CHUNK_VOLUME]> = {
            let mut arr = Box::new([0u32; CHUNK_VOLUME]);
            for (i, v) in voxels.iter().enumerate() {
                arr[i] = v.pack();
            }
            arr
        };
        ChunkStorage::Populated(Box::new(PopulatedChunk {
            material_id: PalettedBitArray::from_raw(&packed),
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
        let mut raw = [0u32; CHUNK_VOLUME];
        for i in 0..CHUNK_VOLUME {
            raw[i] = (i % 9) as u32; // 9 materials
        }
        let p = PalettedBitArray::from_raw(&raw);
        for i in 0..CHUNK_VOLUME {
            assert_eq!(p.get(i), (i % 9) as u32, "mismatch at index {}", i);
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
        use voxel_core::MaterialId;
        let s = ChunkStorage::Uniform {
            voxel: Voxel::cube(MaterialId(3)),
        };
        assert_eq!(s.voxel(0), Voxel::cube(MaterialId(3)));
        assert!(s.is_uniform());
    }

    #[test]
    fn storage_promote_and_collapse() {
        use voxel_core::MaterialId;
        let five = Voxel::cube(MaterialId(5));
        let seven = Voxel::cube(MaterialId(7));
        let mut s = ChunkStorage::Uniform { voxel: five };
        s.set_voxel(100, seven);
        assert!(!s.is_uniform());
        assert_eq!(s.voxel(100), seven);
        assert_eq!(s.voxel(0), five);

        s.set_voxel(100, five);
        s.try_collapse();
        assert!(s.is_uniform());
    }

    /// Boundary contract: the engine container (`PalettedBitArray`) and the
    /// evaluator container (`voxel_core::ChunkBuffer`) are deliberately distinct
    /// and must be *semantically* equivalent - NOT byte-identical. The same
    /// voxel sequence written into each and read back must yield equal
    /// `Vec<Voxel>`. This is the invariant `StorageBoundary::materialize` relies
    /// on; the two containers stay separate by design, so byte-identity is
    /// neither required nor expected.
    #[test]
    fn engine_and_eval_containers_are_semantically_equivalent() {
        use voxel_core::{ChunkBuffer, MaterialId, ShapeId, Voxel};
        const N: usize = voxel_core::CHUNK_DIM;

        // Mean fixture: every ShapeId, the highest MaterialId, every flag combo.
        let shapes = [ShapeId::Empty, ShapeId::Cube, ShapeId::SlabBottom, ShapeId::SlabTop];
        let flags = [
            0u8,
            Voxel::FLAG_LIGHT_SOURCE,
            Voxel::FLAG_WATER_LOGGED,
            Voxel::FLAG_LIGHT_SOURCE | Voxel::FLAG_WATER_LOGGED,
        ];
        let seq: Vec<Voxel> = (0..CHUNK_VOLUME)
            .map(|i| Voxel {
                shape: shapes[i % shapes.len()],
                material: if i == 0 { MaterialId(u16::MAX) } else { MaterialId((i % 9) as u16) },
                flags: flags[i % flags.len()],
            })
            .collect();

        // Engine container: packed-u32 palette.
        let mut engine = PalettedBitArray::new(Voxel::EMPTY.pack());
        for (i, v) in seq.iter().enumerate() {
            engine.set(i, v.pack());
        }
        let engine_read: Vec<Voxel> =
            (0..CHUNK_VOLUME).map(|i| Voxel::unpack(engine.get(i)).unwrap()).collect();

        // Eval container: dense ChunkBuffer<Voxel, 32>, same flat index order.
        let mut buf: ChunkBuffer<Voxel, 32> = ChunkBuffer::uniform(Voxel::EMPTY);
        for (i, v) in seq.iter().enumerate() {
            let (x, y, z) = (i % N, (i / N) % N, i / (N * N));
            buf.set(x, y, z, *v);
        }
        let eval_read: Vec<Voxel> = (0..CHUNK_VOLUME)
            .map(|i| {
                let (x, y, z) = (i % N, (i / N) % N, i / (N * N));
                buf.get(x, y, z)
            })
            .collect();

        assert_eq!(engine_read, eval_read);
        assert_eq!(engine_read, seq);
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