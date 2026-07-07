# Voxel data representation for continuous-density engines

**A hybrid SoA + palette + sentinel architecture can reduce Voxulacrum's memory from 5.12 GB to under 720 MB for 10,000 chunks, while SQLite with delta persistence can shrink world saves by 10–100×.** The critical insight for this engine — continuous i8 signed distance fields with marching cubes — is that palette compression applies only to categorical channels (material, flora) while density must stay as flat arrays for cache-coherent meshing. This report synthesizes research across Minecraft's chunk format, Roblox's row-packing system, Veloren's Rust persistence layer, Minetest's SQLite backend, and the building-blocks crate ecosystem to recommend a production-ready architecture with concrete data structures, byte-level formats, and memory budgets.

---

## 1. Why palette compression fails for density but saves 75% on materials

Minecraft's post-1.13 chunk format stores 16³ sections with a local palette mapping compact bit-packed indices to block state IDs. Each section maintains `bits_per_entry = max(4, ceil(log2(palette_size)))`, packing indices into 64-bit longs without spanning long boundaries (since 1.16). When only one block state exists, the data array is **omitted entirely** — the single-valued optimization that makes homogeneous sections nearly free.

This system was designed for **discrete categorical data** where a typical section contains 3–10 block types. For Voxulacrum's continuous i8 density field, a surface chunk will contain **50–150 unique density values** out of 256 possible. At `ceil(log2(150)) = 8 bits`, palette compression matches raw i8 storage exactly — zero benefit with added overhead. Density must remain a flat `[i8; 32768]` array at **32 KB per chunk**.

Material_id tells a different story. With 9 defined materials, a chunk rarely contains more than 6–9 unique materials. At `ceil(log2(9)) = 4 bits`, palette compression yields **32,768 × 4 / 8 = 16,384 bytes** plus an 18-byte palette — a **4× reduction** from the raw 64 KB u16 array. Flora_id follows the same pattern: few species per chunk means 4-bit indices suffice. Fields like moisture and temperature, being continuous u8 values with high cardinality, gain negligible benefit from paletting and should stay as raw arrays.

| Field | Raw size | Unique values (typical) | Palette bits | Compressed size | Savings |
|-------|----------|------------------------|-------------|-----------------|---------|
| `material_id: u16` | 64 KB | 1–9 | 4 | ~16 KB | **75%** |
| `flora_id: u16` | 64 KB | 0–8 | 4 | ~16 KB | **75%** |
| `hidden_flags: u8` | 32 KB | 2–6 | 3 | ~12 KB | **62%** |
| `density: i8` | 32 KB | 50–150 | 7–8 | ~28–32 KB | **0–12%** ✗ |
| `moisture: u8` | 32 KB | 20–100+ | 7 | ~28 KB | **~12%** ✗ |
| `temperature: u8` | 32 KB | 20–100+ | 7 | ~28 KB | **~12%** ✗ |

The decomposition principle is clear: **palette-compress categorical fields, raw-store continuous fields, and separate them into a struct-of-arrays layout.**

---

## 2. The SoA + tiered allocation architecture that hits 720 MB for 10K chunks

### Homogeneous chunks are still common in volumetric worlds

Unlike heightmap worlds where everything above terrain is guaranteed air, a fully volumetric 3D noise world with caves, overhangs, and floating formations has lower homogeneity — but it's still substantial. Chunks far from any surface clamp to density -128 (deep solid) or +127 (open air). Based on analysis of volumetric terrain engines including Roblox's voxel terrain system and Godot's Voxel Tools, an estimated **40–55% of loaded chunks remain homogeneous** even in feature-rich volumetric worlds. These can be represented with a 16–32 byte sentinel.

### Tiered field allocation eliminates cold data from hot chunks

Different subsystems access different fields. Meshing reads density + material in a sequential 34³ scan. Lighting propagation reads light_sun + light_emit in BFS flood fill. Simulation ticks moisture + temperature periodically. Flora logic touches flora_id + flora_growth only where plants exist. By making warm/cold/sparse tiers `Option`-wrapped, chunks only allocate memory for fields actively in use:

- **Tier 1 (Hot, always present):** density `[i8; 32768]` at 32 KB + material palette at ~16 KB = **~48 KB**
- **Tier 2 (Warm, on lighting):** light_sun + light_emit at 32 KB each = **+64 KB**
- **Tier 3 (Cold, on simulation):** moisture + temperature at 32 KB each = **+64 KB**
- **Tier 4 (Sparse, where flora exists):** flora_id palette ~16 KB + flora_growth + hidden_flags at 32 KB each = **+80 KB**

A surface chunk with only Tier 1+2 allocated consumes **~115 KB** versus the current **512 KB** — a **4.5× reduction** before even counting sentinels.

### Memory budget for 10,000 loaded chunks

| Chunk type | Estimated count | Size each | Subtotal |
|-----------|----------------|-----------|----------|
| All-air sentinel | ~2,500 (25%) | 16 B | 40 KB |
| All-solid sentinel | ~2,000 (20%) | 32 B | 64 KB |
| Surface (Tier 1+2 hot) | ~4,000 (40%) | 115 KB | 460 MB |
| Cave-heavy (Tier 1+2) | ~1,500 (15%) | 130 KB | 195 MB |
| Simulation-active (all tiers) | ~500 (subset) | +130 KB | +65 MB |
| **Total** | **10,000** | | **~720 MB** |

This is well within the 1–2 GB target, down from **5.12 GB** with flat arrays. Adding Roblox-style row packing (discussed below) drops this further to ~500–600 MB.

### Concrete Rust data structures

```rust
/// Top-level chunk storage — near-zero cost for homogeneous chunks
#[derive(Clone)]
enum ChunkStorage {
    /// All voxels identical (density ±128, single material). ~16-32 bytes.
    Uniform { density: i8, material_id: u16 },
    /// Heterogeneous chunk with SoA tiered allocation.
    Populated(Box<PopulatedChunk>),
}

/// SoA layout with lazy tier allocation
struct PopulatedChunk {
    // ── Tier 1: Hot (always present) ──
    density:     Box<[i8; 32_768]>,       // 32 KB, raw flat array
    material_id: PalettedBitArray,         // ~16 KB for ≤16 materials

    // ── Tier 2: Warm (allocated when lighting computed) ──
    light_sun:   Option<Box<[u8; 32_768]>>,
    light_emit:  Option<Box<[u8; 32_768]>>,

    // ── Tier 3: Cold (allocated when simulation runs) ──
    moisture:    Option<Box<[u8; 32_768]>>,
    temperature: Option<Box<[u8; 32_768]>>,

    // ── Tier 4: Sparse (allocated where flora exists) ──
    flora_id:     Option<PalettedBitArray>,
    flora_growth: Option<Box<[u8; 32_768]>>,
    hidden_flags: Option<Box<[u8; 32_768]>>,
}

/// Minecraft-style palette with bit-packed indices
struct PalettedBitArray {
    palette: SmallVec<[u16; 16]>,   // Palette index → actual value
    bits_per_entry: u8,              // ceil(log2(palette.len())), min 1
    data: Box<[u64]>,               // Packed indices: ⌈32768 × bits / 64⌉ longs
}

impl PalettedBitArray {
    fn get(&self, index: usize) -> u16 {
        let bit_offset = index * self.bits_per_entry as usize;
        let long_index = bit_offset / 64;
        let bit_start = bit_offset % 64;
        let mask = (1u64 << self.bits_per_entry) - 1;
        let palette_idx = ((self.data[long_index] >> bit_start) & mask) as usize;
        self.palette[palette_idx]
    }

    fn set(&mut self, index: usize, palette_idx: usize) {
        let bpe = self.bits_per_entry as usize;
        let bit_offset = index * bpe;
        let long_index = bit_offset / 64;
        let bit_start = bit_offset % 64;
        let mask = (1u64 << bpe) - 1;
        self.data[long_index] &= !(mask << bit_start);
        self.data[long_index] |= (palette_idx as u64) << bit_start;
    }

    fn memory_bytes(&self) -> usize {
        self.palette.len() * 2 + self.data.len() * 8 + 1
    }
}
```

### Edit operations on palette-compressed material

When a voxel's material changes, four cases arise in order of frequency:

1. **Material already in palette:** Update the bit-packed index. O(1).
2. **Old material's refcount drops to zero:** Reuse that palette slot for the new material. O(1).
3. **Free slot exists (prior refcount reached zero):** Insert into free slot. O(1).
4. **Palette full, must grow:** Increase `bits_per_entry` and re-encode the entire 32,768-entry array. O(n) but rare — happens only when a new material type is introduced to the chunk.

Maintaining per-entry reference counts avoids unnecessary palette growth. With 9 defined materials and 4-bit palette capacity (16 slots), Case 4 almost never triggers in practice.

---

## 3. RLE and row-packing — compression with O(1) random access

### RLE is impractical for in-memory meshing

Arseny Kapoulkine at Roblox reported RLE achieving **0.07 bytes/voxel** (73 MB for ~1 billion voxels) — exceptional compression. But random access into RLE requires O(log n) lookups via interval tree per voxel. With marching cubes reading 34³ = 39,304 voxels, that's ~400K tree lookups versus 39K flat array accesses. The Subterranean Software blog confirmed this finding: random access in RLE is "clearly unacceptable" for meshing. **RLE belongs in on-disk serialization and network transport, not in-memory hot storage.**

### Row-packing preserves O(1) access with 2–6× compression

Roblox's solution: a **row-packed format** where each of the 1,024 rows (32 Y × 32 Z, each 32 voxels long in X) is either uniform (store a single value) or allocated (store 32 values). This achieved **0.49 bytes/voxel** — a 6× reduction from unpacked — while preserving O(1) random access for meshing. For a surface chunk where ~40–60% of rows are uniform (entirely air or entirely solid), this cuts the density array from 32 KB to ~13–19 KB:

```rust
struct RowPackedI8 {
    /// 1024 row headers. Bit 15 = allocated flag.
    /// If uniform: low 8 bits = the i8 value.
    /// If allocated: low 15 bits = offset into row_data (÷32).
    headers: Box<[u16; 1024]>,  // 2 KB
    row_data: Vec<i8>,           // Only non-uniform rows
}

impl RowPackedI8 {
    fn get(&self, x: usize, y: usize, z: usize) -> i8 {
        let header = self.headers[y * 32 + z];
        if header & 0x8000 != 0 {
            // Allocated: read from dense storage
            let offset = (header & 0x7FFF) as usize * 32;
            self.row_data[offset + x]
        } else {
            // Uniform: return the stored value
            (header & 0xFF) as i8
        }
    }
}
```

This is an optional enhancement over flat arrays. It adds indirection cost (~1 branch per access) but can reduce memory by another 30–50% on typical surface chunks.

---

## 4. SVOs are wrong for marching cubes on continuous density

Sparse Voxel Octrees compress by merging uniform subtrees — but with continuous SDF values, subtrees near the isosurface are **never uniform** because each voxel holds a unique distance gradient. Only far-interior/exterior regions benefit, and those are already handled by chunk-level sentinels. Three further problems make SVOs counterproductive here:

- **Cache incoherence.** Marching cubes iterates linearly through a 3D grid (x → z → y). SVO traversal requires pointer-chasing through scattered nodes — O(log n) per lookup versus O(1) for flat arrays. A 64-byte cache line holds 64 sequential i8 density values in a flat array; in an SVO, it holds a few tree nodes that may point to unrelated subtrees.

- **Per-node overhead.** Each SVO internal node needs at least 1 byte (child mask) plus pointers. For a 32³ chunk with high surface density, tree overhead can exceed flat array storage. Eisenwave's voxel compression documentation confirms: "Entirely filled models will have some overhead compared to an array."

- **Mutability cost.** Editing a single voxel in an SVO requires traversing and potentially restructuring the tree from root to leaf. In a flat array, it's a single indexed write.

The two-level sparse architecture — **HashMap of chunk coordinates → flat SoA arrays within chunks** — achieves comparable compression via sentinels and row-packing while being far simpler and cache-friendlier for marching cubes.

---

## 5. On-disk persistence — SQLite with delta storage

### Why SQLite beats region files and per-chunk files

Three storage backends dominate the voxel engine landscape: Minecraft Java's Anvil region files (32×32 chunk grids with 4 KB sector allocation), Minecraft Bedrock's LevelDB (LSM-tree key-value store), and Minetest/Luanti's SQLite (single-file relational database). After evaluating all three:

**Minetest migrated FROM individual files TO SQLite** because "filesystems were struggling under the number of files and directories" on large servers. This is directly relevant — Voxulacrum's current per-chunk mesh cache files will hit the same scaling wall. SQLite benchmarks show compressed BLOB reads are **35% faster than individual file reads** for BLOBs under 100 KB (due to eliminating per-file open()/close() syscalls). Pre-compressed voxel chunks at 10–50 KB each sit in this sweet spot.

LevelDB (Bedrock's choice) has **known reliability issues** documented by both the Minetest and broader database communities. Minetest explicitly deprecated their LevelDB backend. Anvil region files suffer from write amplification — updating a chunk that outgrows its sector allocation requires relocating it to the end of the file, leaving fragmentation holes.

SQLite in WAL mode provides **unlimited concurrent readers plus a single writer** with no blocking — ideal for a server-authoritative architecture where meshing threads read chunk data while the game loop writes edits. The `rusqlite` crate with its `bundled` feature provides zero-dependency SQLite bindings for Rust.

### Delta storage saves 10–100× on persistence

Base terrain is deterministically regenerable from the world seed. Veloren's production implementation stores **only player-modified blocks** as a `HashMap<Vec3<i32>, Block>` per chunk, applying them as overlays during chunk load. This pattern transforms persistence economics:

| Player activity | Chunks modified | Avg edits/chunk | Delta size/chunk | Full chunk size | Total save (delta) | Total save (full) |
|----------------|----------------|-----------------|------------------|----------------|-------------------|------------------|
| Fresh world | 0 | 0 | 0 B | 0 B | **0 B** | **0 B** |
| Light survival (1 hr) | ~500 | ~20 | ~400 B | ~40 KB | **~200 KB** | **~20 MB** |
| Heavy survival (10 hr) | ~5,000 | ~100 | ~2 KB | ~40 KB | **~10 MB** | **~200 MB** |
| Creative building | ~2,000 | ~5,000 | ~40 KB | ~40 KB | **~80 MB** | **~80 MB** |
| Server, 100 hr, 10 players | ~50,000 | ~200 | ~4 KB | ~40 KB | **~200 MB** | **~2 GB** |

The crossover point where a delta list exceeds full chunk storage: at **~6 bytes per edit** (3 bytes packed position in 32³ + 3 bytes essential voxel data), a full compressed chunk at ~40 KB is exceeded at **~6,700 edits**. For 32,768 total voxels, that's ~20% modification. **Auto-promote to full chunk storage when edit count exceeds 25% of voxels (8,192 edits).**

Veloren adds a self-healing optimization: if a stored delta block matches the regenerated terrain (player placed the same block that would have been generated), remove it from the delta list. This naturally shrinks saves over time as terrain generation evolves.

### Compression: zstd level 3 for storage, LZ4 for hot cache

| Metric | LZ4 | zstd-1 | zstd-3 | zstd-9 |
|--------|-----|--------|--------|--------|
| Compression speed | ~370 MB/s | ~338 MB/s | ~200 MB/s | ~50 MB/s |
| Decompression speed | ~1,590–4,000 MB/s | ~500 MB/s | ~500 MB/s | ~500 MB/s |
| Ratio (surface chunk) | ~4:1 | ~6:1 | ~8:1 | ~10:1 |
| Ratio (homogeneous chunk) | ~80:1 | ~150:1 | ~180:1 | ~200:1 |

Zstd's decompression speed is **constant regardless of compression level** — a level-9 compressed chunk decompresses just as fast as level-1. This enables asymmetric compression: spend CPU time compressing once on save, decompress cheaply on every load. For Voxulacrum, **zstd level 3** provides the best ratio/speed balance for on-disk storage. LZ4 remains optimal for the in-memory hot cache where decompression speed dominates (a 50 KB chunk decompresses in ~12 µs with LZ4).

Zstd's **dictionary compression** mode trains a shared dictionary on ~100+ sample chunks, improving ratios by 2–5× for small payloads. This is particularly valuable for delta chunks that may be only a few hundred bytes. Store the dictionary in the database's metadata table, versioned with the world save.

### Byte-level on-disk format specification

```
WORLD FILE: world_<name>.vxdb (SQLite database)

PRAGMAS:
  page_size     = 16384
  journal_mode  = WAL
  synchronous   = NORMAL
  cache_size    = -32768  (32 MB)
  mmap_size     = 268435456  (256 MB)

TABLE meta (key TEXT PRIMARY KEY, value BLOB) WITHOUT ROWID
  Keys: 'format_version' (u32), 'world_seed' (u64),
        'generator_version' (u32), 'zstd_dictionary' (BLOB),
        'created_epoch' (i64), 'engine_version' (string)

TABLE chunks (
  cx INTEGER, cy INTEGER, cz INTEGER,
  flags INTEGER,     -- bit 0: delta(1)/full(0)
                     -- bit 1-2: compression (0=none, 1=LZ4, 2=zstd, 3=zstd+dict)
                     -- bit 3: has_entities
  modified INTEGER,  -- epoch seconds
  data BLOB,
  PRIMARY KEY (cx, cy, cz)
) WITHOUT ROWID

CHUNK BLOB LAYOUT (after SQLite extraction, before decompression):
Offset  Size   Field
0       1      format_version (currently 1)
1       1      storage_type (0=delta_sparse, 1=full_soa, 2=full_raw)
2       4      uncompressed_size (LE u32)
6       2      entry_count (LE u16: edit count for delta, run count for RLE)
8       4      checksum (CRC32 of uncompressed payload)
12      N      compressed payload

DELTA_SPARSE PAYLOAD (storage_type=0, decompressed):
  Per edit (entry_count entries):
    Offset  Size  Field
    0       2     position: x[4:0] | y[9:5] | z[14:10] (15 bits, packed u16)
    2       1     density (i8)
    3       2     material_id (LE u16)
    5       1     moisture (u8)
    6       1     temperature (u8)
    --- 7 bytes per edit ---

FULL_SOA PAYLOAD (storage_type=1, decompressed):
    Offset      Size      Field
    0           32768     density[32768] as i8
    32768       2         material_palette_len (LE u16)
    32770       P×2       material_palette[P] as LE u16
    32770+P×2   1         material_bits_per_entry
    +1          ⌈32768×bpe/8⌉  material_packed_indices
    ...         32768     light_sun[32768] (or 0 bytes if absent, flagged in header)
    ...         32768     light_emit[32768]
    ...         32768     moisture[32768]
    ...         32768     temperature[32768]
    ...         (palette)  flora_id palette + packed indices
    ...         32768     flora_growth[32768]
    ...         32768     hidden_flags[32768]
    (field presence indicated by bitmask in format_version or a field-presence byte)
```

---

## 6. Integration architecture and concurrency with Bevy ECS

### Arc-based copy-on-write is the optimal concurrency model

Three concurrency patterns were evaluated for simultaneous meshing reads and gameplay writes:

**RwLock** blocks writers while meshers hold read locks (meshing takes milliseconds). Godot Voxel Tools initially used per-chunk RwLocks across thousands of chunks and found the approach problematic, eventually replacing it with spatial region locking. **Double-buffering** wastes 2× memory permanently even when no concurrent access occurs. **Arc-based copy-on-write** using Rust's `Arc::make_mut` provides the best tradeoff: snapshots cost only an atomic refcount increment, and data is cloned only when a writer detects an active reader reference:

```rust
struct ChunkSlot {
    data: Arc<ChunkStorage>,
    generation: u64,         // Monotonic version counter
    dirty_mesh: bool,
    dirty_persist: bool,
}

// Mesher: take zero-cost snapshot
fn snapshot(slot: &ChunkSlot) -> Arc<ChunkStorage> {
    Arc::clone(&slot.data)  // Atomic increment only, ~1ns
}

// Writer: copy-on-write only when meshers hold references
fn edit(slot: &mut ChunkSlot, idx: usize, new_density: i8, new_mat: u16) {
    let data = Arc::make_mut(&mut slot.data);  // Clones only if refcount > 1
    match data {
        ChunkStorage::Uniform { .. } => {
            // Promote to Populated on first heterogeneous edit
            *data = ChunkStorage::Populated(Box::new(PopulatedChunk::from_uniform(data)));
            // Then apply edit...
        }
        ChunkStorage::Populated(chunk) => {
            chunk.density[idx] = new_density;
            chunk.material_id.set_value(idx, new_mat);
        }
    }
    slot.generation += 1;
    slot.dirty_mesh = true;
    slot.dirty_persist = true;
}
```

When no mesher is active, `Arc::make_mut` mutates in place with zero allocation. When a mesher holds a snapshot, the write path clones the inner data (one-time ~48–115 KB allocation) while the mesher continues on its old, consistent snapshot. This is effectively **MVCC for voxel data.**

### Bevy ECS integration as chunk entities

Every surveyed Bevy voxel engine (vx_bevy, bevy_voxel_world, projekto) uses **chunks as ECS entities** rather than a monolithic Resource. This enables Bevy's change detection, marker component queries, and natural entity lifecycle:

```rust
#[derive(Component)] struct ChunkPos(IVec3);
#[derive(Component)] struct ChunkData(Arc<ChunkStorage>);
#[derive(Component)] struct ChunkState(LifecycleState);
#[derive(Component)] struct DirtyMesh;    // Marker: needs re-mesh
#[derive(Component)] struct DirtyPersist; // Marker: needs save
#[derive(Component)] struct MeshTask(Task<MeshResult>);
#[derive(Component)] struct GenTask(Task<ChunkStorage>);

#[derive(Resource)]
struct ChunkMap(HashMap<IVec3, Entity>);  // Spatial lookup

// System pipeline — ordered to respect data dependencies
app.add_systems(Update, (
    chunk_loading_system,         // Decide load/unload based on player position
    chunk_gen_dispatch,           // AsyncComputeTaskPool for noise generation
    chunk_gen_receive,            // Poll tasks → insert ChunkData + DirtyMesh
    chunk_edit_system,            // Apply edits → mark DirtyMesh + DirtyPersist
    chunk_mesh_dispatch,          // AsyncComputeTaskPool for marching cubes
    chunk_mesh_receive,           // Poll tasks → upload Mesh handles to GPU
    chunk_persist_dispatch,       // IoTaskPool for SQLite writes (background)
    chunk_unload_system,          // Force-save dirty → despawn entity
).chain());
```

Meshing dispatches by gathering `Arc::clone` snapshots of the target chunk plus its 6 face-neighbors (for the 34³ padded volume), then spawning onto `AsyncComputeTaskPool`. A generation counter on the returned mesh detects staleness — if the chunk was edited during meshing, the stale mesh is discarded and re-meshing is triggered.

### Border-aware dirty propagation and edit debouncing

When an edit occurs within 1 voxel of a chunk face, the neighboring chunk's mesh also becomes stale (marching cubes reads a 1-voxel border). Separate `dirty_mesh` and `dirty_persist` flags prevent unnecessary disk writes when only re-meshing is needed (e.g., neighbor border invalidation). For creative mode's rapid large-area edits, a **50ms debounce** timer prevents thrashing — edits accumulate into a batch, and re-meshing is dispatched only after the edit burst settles.

---

## 7. Meshing access patterns favor SoA with lazy material lookup

Marching cubes evaluates every cell corner in a 33³ grid (35,937 cells, reading 8 density values each). With SoA layout, the density array is contiguous — a 64-byte L1 cache line holds **64 sequential i8 density values**, giving near-perfect cache utilization for the sequential x→z→y iteration pattern. Under AoS layout (the current 16-byte struct), each cache line holds only 4 voxels' worth of density, wasting **81% of cache bandwidth** on fields the mesher doesn't need.

Material lookup is only needed at **surface voxels** — those where the SDF sign changes between adjacent cells. In a typical surface chunk, only **10–20% of voxels are near the isosurface** (3,000–6,000 out of 32,768). The palette indirection for material_id at these points is a single array lookup — negligible cost. There is no need to decompress the entire material array for meshing; the palette stays compressed and is accessed only at surface points.

This means the SoA density array IS the "hot format" — **no decompression step is needed for meshing**. Unlike RLE or octree representations that require full materialization before grid-sequential access, SoA with flat density arrays serves both storage efficiency and meshing performance simultaneously.

---

## 8. Comparison of three complete in-memory approaches

| Criterion | **A: Flat AoS (current)** | **B: SoA + Palette + Sentinels** | **C: SVO-based** |
|-----------|--------------------------|----------------------------------|-------------------|
| Memory per surface chunk | 512 KB | ~48–115 KB (hot tiers only) | ~60–200 KB (high overhead near surface) |
| Memory per air chunk | 512 KB | 16 bytes (sentinel) | 8 bytes (empty root node) |
| 10K chunks total | **5.12 GB** | **~720 MB** | ~1.5–3 GB (poor SDF compression) |
| Marching cubes read speed | Good (sequential, but 81% cache waste) | **Excellent** (density array fills cache lines) | Poor (O(log n) pointer chasing per voxel) |
| Edit cost | O(1) | O(1) amortized, O(n) rare palette grow | O(log n) tree restructure |
| Implementation complexity | Trivial | Moderate (palette logic, tier management) | High (tree ops, memory management) |
| Meshing preparation | None needed | None (density already flat) | Must flatten to array first |
| Suited for continuous SDF? | Yes (wastes space) | **Yes (optimal)** | No (defeats subtree merging) |

**Approach B is recommended** — it achieves a **7× memory reduction** over flat AoS while maintaining optimal cache performance for marching cubes. SVOs are categorically wrong for this engine's combination of continuous density and grid-sequential meshing.

---

## 9. Implementations and crates to build on

**building-blocks** (bonsairobo, Rust, MIT) is the most directly relevant existing crate. It provides `Array3x1` for flat SDF arrays, surface nets meshing, LZ4/Snappy chunk compression, and `ChunkTree` octree-of-chunks with clipmapping. Though in maintenance mode, its extracted crates remain active: **ndshape** for compile-time 3D array indexing (`ConstShape3u32<34, 34, 34>` for padded meshing volumes), **fast-surface-nets** for SDF→mesh conversion, and **block-mesh** for discrete voxel meshing. The `lz4_flex` crate provides pure-Rust LZ4 compression, and `zstd` provides bindings to Facebook's libzstd.

**Veloren** (Rust, GPL-3.0) provides a production-proven delta persistence system: only player-modified blocks are stored, serialized with bincode, written atomically via rename for crash safety. Its chunk versioning uses ordered loader arrays that try newest format first and fall back gracefully. **Minetest/Luanti** validates SQLite as a voxel world backend at scale, having migrated away from individual files and explicitly deprecated LevelDB due to reliability issues.

The **Eisenwave voxel compression documentation** is the most comprehensive reference for compression algorithm analysis (RLE, SVO, FLVC, space-filling curves). For SDF-specific research, **AMD's FidelityFX Brixelizer** demonstrates sparse distance field cascades with a brick-atlas architecture (sparse at world level, dense within 8³ bricks) — conceptually parallel to Voxulacrum's hashmap-of-dense-chunks approach.

---

## Conclusion

The recommended architecture combines five proven techniques into a coherent system:

**In memory**, decompose VoxelData into struct-of-arrays with palette compression on categorical channels (material_id at 4 bits saves 75%) and raw flat arrays for continuous channels (density at 32 KB enables cache-optimal marching cubes). Tiered lazy allocation ensures chunks only carry the fields their active subsystems need. Homogeneous chunk sentinels eliminate ~45% of chunks for ~16 bytes each. The total memory budget drops from 5.12 GB to approximately 720 MB for 10,000 chunks.

**On disk**, a single SQLite database per world with WAL mode provides concurrent read/write, ACID crash safety, and 35% faster reads than per-file storage for compressed BLOBs. Delta persistence — storing only player edits against the regenerable seed — cuts survival-mode world saves from hundreds of megabytes to single-digit megabytes. Zstd level 3 compression achieves 8:1 ratios on surface chunks (versus LZ4's 4:1) with constant-speed decompression. Auto-promotion from delta to full-chunk storage at 25% modification prevents pathological cases.

**For concurrency**, `Arc::make_mut` provides zero-cost snapshots for meshing workers and copy-on-write semantics for editors with no locks, no blocking, and no permanent memory doubling. Chunks live as Bevy ECS entities with marker components for dirty tracking, using `AsyncComputeTaskPool` for generation/meshing and `IoTaskPool` for persistence.

The key insight throughout: **continuous density fields fundamentally change the optimization landscape.** Palette compression, SVOs, and RLE all assume categorical or sparse data — they fail or break even on continuous SDF values. The winning strategy treats density as sacred flat arrays and applies compression only where data is genuinely categorical.