# Voxel System Benchmark Report — Data Handling for Meshing, Caching, and Streaming

**Engine:** Voxulacrum
**Date:** 2026-03-27 (revised from 2025-03-25 baseline)
**Scope:** Voxel data pipeline from generation through meshing to GPU upload, with annotations against stated design goals.

---

## 1. Design Goals Summary

| Axis | Target |
|------|--------|
| **World scale** | Infinite XZ streaming; vertical extent expanded from 4 chunks (64 voxels) to ~20 chunks (320 voxels) — 6 below, 10 above current range |
| **Multiplayer** | Local hosting (8–10 players), dedicated servers (100+), server-authoritative voxel state |
| **Persistence** | Multiple saved worlds per player; full terrain persistence across sessions |
| **Terrain mutability** | Single-voxel to small multi-voxel precision edits in survival; large multi-voxel sweeps in creative mode |
| **Visual style** | Stylized low-poly orthographic with pixel upscaling and palette quantization; SDF reserved for particles/effects |
| **Target performance** | 1080p @ 60 fps on moderate hardware; support for ultrawide and 4:3 aspect ratios |
| **Simulation** | Simplified cellular-automata fluids; Rain World–inspired living-world cycles (weather, plant growth, fauna behavior) at a pace tuned for novelty over realism |
| **LOD** | Standard play is 1–8 chunks across; bird's-eye drone mechanic zooms out far enough to require multiple LOD tiers |
| **Vegetation** | Dense, diverse biome coverage — grasses, moss, trees, shrubs, wildflowers, aquatic plants — core to visual identity and simulation |
| **Extensibility** | Data-driven material/voxel system for content scaling; modding as a secondary beneficiary |
| **World generation** | Heavy 3D noise-driven generation (not heightmap-based); rich volumetric features — caves, overhangs, arches, tunnels, floating formations — are first-class terrain, not carved exceptions. Variety and surprise in world topology is a core design pillar |
| **Pain points** | Startup and chunk generation speed; mesh cache disk size |

---

## 2. Current Architecture — Subsystem Audit

### 2.1 Voxel Data Representation

**Structure: SoA with tiered lazy allocation (post-sparse-storage implementation)**

```
ChunkStorage enum:
  Uniform { density: i8, material_id: u16 }           → ~16 bytes
  Populated(Box<PopulatedChunk>)                       → ~48–210 KB depending on tiers

PopulatedChunk (SoA layout):
  Tier 1 (Hot, always present):
    density:      Box<[i8; 32_768]>                    → 32 KB flat array
    material_id:  PalettedBitArray                     → ~16 KB (4-bit for 9 materials)
  Tier 2 (Warm, lazy — allocated when lighting computed):
    lighting:     Option<Box<LightingData>>            → +64 KB (light_sun + light_emit)
  Tier 3 (Cold, lazy — allocated when simulation runs):
    simulation:   Option<Box<SimulationData>>          → +64 KB (moisture + temperature)
  Tier 4 (Sparse, lazy — allocated where flora exists):
    flora:        Option<Box<FloraData>>               → ~80 KB (palette flora_id + growth + flags)
```

**PalettedBitArray:** Minecraft-style palette with bit-packed indices. Indices do not span u64 word boundaries. Auto-grows bits_per_entry when new values are introduced (rare with 9 materials). Supports O(1) get/set, bulk `from_raw()` construction.

**Chunk dimensions:** 32×32×32 = 32,768 voxels
**Memory per populated chunk (Tier 1 only):** ~48 KB
**Memory per uniform chunk:** ~16 bytes
**Indexing order:** X-fastest (x + z×32 + y×1024)

**Concurrency:** `Arc<ChunkStorage>` provides zero-cost snapshot cloning for meshing workers via atomic refcount increment. Edits use `Arc::make_mut` semantics (clone inner data only when meshing workers hold references).

**Assessment against goals:**

- [RESOLVED] **Memory per chunk reduced ~7–10×.** Surface chunks at ~48 KB (Tier 1 only) vs. former 512 KB. Uniform chunks (air/solid) at ~16 bytes. Estimated 10,000-chunk memory budget: ~720 MB, well within the 1–2 GB target. This enables the 20-vertical-chunk expansion without memory crisis.
- [RESOLVED] **Sparse representation via Uniform sentinel.** Homogeneous chunks (estimated 40–55% of loaded set in volumetric worlds) consume near-zero memory. The ChunkStorage enum eliminates allocation for these entirely.
- [RESOLVED] **Tiered allocation prevents cold data waste.** Chunks only carry fields their active subsystems need. A surface chunk that hasn't been simulated avoids 128 KB of lighting + simulation arrays.
- [RESOLVED] **Serialization implemented.** Full persistence via SQLite with delta storage (see §2.5).
- [OK] **PalettedBitArray for material_id** achieves 75% compression (4-bit indices vs. raw u16) with O(1) access.
- [OK] **Flat density array** is cache-optimal for marching cubes — 64 sequential i8 values per L1 cache line.
- [OK] **Density as i8 remains adequate** for the low-poly aesthetic.
- [REMAINING GAP] **No row-packing on density.** The research report identified Roblox-style row-packing (uniform rows stored as single value) as an optional 30–50% further reduction on density arrays. Not yet implemented — could push surface chunks from ~48 KB to ~30 KB.

### 2.2 Chunk Lifecycle and World Container

**Container:** `HashMap<IVec3, Chunk>` — flat hash map keyed by chunk coordinates.

**Chunk struct (post-implementation):**
```
Chunk {
    position: IVec3,
    storage: Arc<ChunkStorage>,         // Zero-cost cloning for snapshots
    mesh_dirty: bool,
    mesh_seq: u64,                      // Monotonic counter — detects stale mesh results
    mesh: Option<ChunkMesh>,
    generation: u64,                    // Version counter — incremented on each edit
    persist_dirty: bool,                // Needs saving to disk
    edit_list: Option<Vec<VoxelEdit>>,  // Delta edits for persistence; None = auto-promoted to full
    mesh_debounce: Option<Instant>,     // Edit-triggered debounce (50ms); None = generation-triggered
}
```

**Chunk state tracking:** Uses separate bool flags (`mesh_dirty`, `persist_dirty`) and the `mesh_seq` counter rather than a formal state machine enum. The `generation` counter enables stale mesh detection — if a chunk is edited during meshing, the returned mesh is discarded.

**Snapshot system:** 36×36×36 padded density snapshots with Arc-based material references.
```
ChunkSnapshot {
    position: IVec3,
    density: Box<[i8; SNAP_VOLUME]>,         // 46,656 bytes — flat, cache-optimal
    materials: SnapshotMaterials,             // Arc refs to 27 chunk storages
    border_min: [bool; 3],
}
```
Snapshot assembly materializes density into a contiguous flat buffer (memcpy for Populated, memset for Uniform). Material is accessed lazily through Arc references only at surface voxels (~10–20% of volume).

**Assessment against goals:**

- [IMPROVED] **Dirty tracking is functional.** Separate `mesh_dirty` and `persist_dirty` flags correctly distinguish "needs remeshing" from "needs disk save." Border edits propagate `mesh_dirty` to neighbors without falsely marking them `persist_dirty`.
- [IMPROVED] **Snapshot allocation reduced ~15×.** From ~730 KB (46,656 × 16 bytes AoS) to ~47 KB (46,656 × 1 byte density only). Material access is zero-copy through Arc refs. This largely resolves the former allocation pressure pain point.
- [IMPROVED] **Edit debouncing (50ms)** prevents creative-mode bulk edits from thrashing the meshing pipeline. Generation-triggered dirty skips debounce for immediate meshing.
- [REMAINING GAP] **No formal chunk state machine.** States are still implicit via bool flags. As networking and LOD are added, the combinatorial state space (generated/meshed/dirty/saving/replicating/LOD-level) will benefit from an explicit enum.
- [REMAINING GAP] **No region/column grouping.** Still flat HashMap with no spatial locality.
- [OK] **Arc-based snapshot isolation** is an excellent concurrency model — zero-cost for readers, copy-on-write for writers.
- [OK] **mesh_seq counter** cleanly solves the stale-mesh-during-edit race condition.

### 2.3 Terrain Generation

**Generator:** 10 FastNoiseLite instances (height FBm, ridged, detail; 3 cave noise layers + warp + entrance; material noise; flora noise).

**Execution:** Per-chunk, parallelized with rayon on background worker threads. Each of 4 worker threads owns its own `TerrainGenerator` clone.

**Per-voxel work:** Height computation (3-layer noise), cave carving (3 cave types + domain warp), material assignment, moisture, flora placement.

**Design note — 3D noise is intentional and non-negotiable:** The engine's generation philosophy is fundamentally volumetric. Caves, overhangs, arches, tunnels, floating rock formations, and other 3D features are first-class terrain — not post-hoc carving operations on a heightmap base. This rules out the common optimization of reducing generation to a 2D heightmap with exceptions. Every voxel must be evaluated through the full 3D noise stack because any voxel at any position could be surface, void, or solid depending on the volumetric field. Optimization strategies must respect this constraint.

**Assessment against goals:**

- [PAIN POINT] **Generation is the primary startup bottleneck.** At 4 vertical chunks and a ~20×20 XZ footprint, initial load generates ~1,600 chunks. At 20 vertical chunks, this becomes ~8,000 chunks. Each chunk evaluates 32,768 voxels through 10 noise functions — roughly 2.6 billion noise samples for initial load. With the intent to add even more 3D noise layers for richer variety (more biome types, more volumetric feature generators), this cost will increase further. Even with rayon parallelism across 4 workers, generation dominates startup time.
- [PAIN POINT] **3D noise fundamentally prevents trivial-chunk early-out.** Unlike heightmap-based engines where chunks far above the surface are guaranteed air, a fully volumetric generator can place solid material anywhere — a floating island at Y=15 or a cave entrance at the world ceiling. This means no chunk can be cheaply skipped based on Y-coordinate alone. The traditional optimization of "classify columns from a 2D heightmap, skip air/solid chunks" is incompatible with the generation goals. Alternative acceleration strategies are needed: SIMD/GPU noise evaluation, coarse-then-refine sampling (evaluate a sparse grid first to detect if the chunk is likely homogeneous before full-resolution evaluation), or hierarchical noise where expensive detail layers are only evaluated near surfaces detected by cheaper base layers.
- [GAP] **No coarse pre-sampling pass.** Even without heightmaps, a chunk could be sampled at a coarse resolution (e.g., 4³ or 8³ grid = 64–512 samples instead of 32,768) using only the dominant low-frequency noise layers. If all coarse samples agree on sign (all positive = solid, all negative = air), the chunk can be represented as a constant without full evaluation. Chunks where coarse samples disagree contain a surface and require full-resolution generation. This is the volumetric equivalent of heightmap-based early-out — it respects 3D features while still skipping the ~60–70% of chunks that are deep interior solid or high-altitude air with no floating features.
- [GAP] **No hierarchical noise evaluation.** The current generator evaluates all 10 noise functions for every voxel uniformly. A layered approach — evaluate cheap base noise first, then only evaluate expensive detail/cave/material noise for voxels near the isosurface — would reduce per-voxel cost for the majority of voxels that are clearly interior or exterior. This is especially important as more noise layers are added for variety.
- [GAP] **No SIMD or GPU-accelerated noise.** FastNoiseLite evaluates one sample at a time. SIMD-batch noise libraries (e.g., fastnoise2, or manual SIMD via std::simd / packed_simd) can evaluate 4–8 samples simultaneously. GPU compute generation could evaluate entire chunks in parallel on the GPU, returning voxel data via readback — particularly effective for the initial burst where thousands of chunks need generation.
- [GAP] **No incremental generation.** The system generates all visible chunks at startup. For infinite streaming, generation must be amortized across frames as the player moves, which the priority queue partially enables — but the initial burst remains problematic.
- [GAP] **No deterministic chunk generation from seed alone.** The current system regenerates the entire world from scratch. For persistence, chunks need to be generatable on-demand from world seed + chunk position, then overlaid with player modifications from disk.
- [GAP] **Current noise layer count (10) is modest relative to goals.** Achieving "a ton more interesting variety" in 3D-noise-driven generation will likely require 20–40+ noise evaluations per voxel (biome blending, multiple feature generators, material variation, moisture/temperature fields, ore distribution, etc.). The generation pipeline needs to be architected to scale to this density without linear cost growth — through hierarchical evaluation, spatial caching of intermediate noise results, or precomputed noise textures.
- [OK] **Worker-per-thread TerrainGenerator cloning** avoids contention.
- [OK] **Priority queue ordering** (nearest-first) is correct for perceived load time.
- [OK] **Fully volumetric 3D noise approach** is the correct architectural choice for the target world variety. The performance cost is real but solvable through the acceleration strategies above.

### 2.4 Meshing Pipeline

**Architecture:** Two-phase asynchronous worker thread pipeline.

**Phase 1 — Marching Cubes + Flat Shading (CPU, worker threads):**
1. Sample 8 corner densities per cell
2. Compute case index (256 cases) from sign bits
3. Edge vertex interpolation with quarter-voxel snapping (5 discrete positions per edge)
4. Triangle generation from TRI_TABLE
5. Flatten mesh (duplicate vertices per-triangle for flat shading)
6. Snap normals to 42 predefined directions
7. Per-vertex ambient occlusion (5×5×5 weighted kernel, radius 2)
8. Optional greedy face merging (coplanar triangle merge → boundary extraction → ear clipping)

**Phase 2 — Index extraction + cache save (CPU, worker threads):**
- Extracts pre-computed `greedy_indices`
- Writes mesh to disk cache (LZ4-compressed bincode)

**Worker count:** `num_cpus - 2` (minimum 2)
**Submission throttle:** 8 snapshots per frame maximum
**Neighbor wait:** 60 frames before meshing without full neighbors

**Assessment against goals:**

- [PAIN POINT] **3D noise increases the surface chunk ratio.** In a heightmap world, only ~30–40% of chunks contain a surface and need meshing. With volumetric 3D features (caves, tunnels, overhangs, floating formations), interior chunks that would be trivially solid in a heightmap world now contain surfaces — cave walls, tunnel ceilings, arch interiors. This raises the surface ratio to ~50–70%, proportionally increasing the total mesh workload.
- [PAIN POINT] **Meshing is the second major bottleneck.** The per-cell AO computation samples a 5×5×5 neighborhood for every vertex of every triangle. For a surface chunk producing ~5,000 triangles with 3 vertices each, that's 15,000 × 125 = 1.87 million density lookups per chunk just for AO.
- [GAP] **Greedy merging has diminishing returns at high cost.** The ear-clipping triangulator is O(n²) worst case. For the low-poly aesthetic, the triangle reduction from greedy merging may not justify the CPU cost, especially if the GPU is not draw-call or vertex bound at the target chunk counts. Profiling data is needed to confirm.
- [GAP] **No LOD mesh generation.** Every chunk meshes at full 32³ resolution. The bird's-eye drone view will render hundreds of distant chunks that could use 16³ or 8³ meshes (or even billboard impostors) at a fraction of the cost.
- [GAP] **Flat shading triples vertex count.** Duplicating vertices per-triangle for per-face normals is the simplest approach but means a chunk with 5,000 triangles has 15,000 vertices instead of ~3,000 shared vertices. With the 42-normal constraint, flat normals could be encoded as a per-face attribute or derived in the vertex shader from triangle ID, eliminating duplication.
- [GAP] **No mesh simplification or decimation pass.** For LOD transitions, a mesh simplification pipeline (e.g., quadric error metrics adapted for voxel geometry) would be needed.
- [GAP] **Two-phase split adds latency.** Phase 2 only extracts indices and writes cache — work that could be folded into Phase 1's worker without a second dispatch. The extra phase adds one frame of latency per mesh job.
- [OK] **Quarter-voxel snapping** is an excellent fit for the low-poly aesthetic, constraining geometry to a clean grid.
- [OK] **42 predefined normals** enforce consistent lighting across adjacent chunks and enable effective greedy merging.
- [OK] **Snapshot-based isolation** ensures worker threads never touch shared mutable voxel data.

### 2.5 Mesh Cache

**Format:** LZ4-compressed bincode files on disk.
**Path:** `cache/meshes/{x}_{y}_{z}_{hash:016x}.bin`
**Compact vertex:** 6 bytes (position quantized to u8, normal index, AO, material ID)
**Cache key:** SeaHash of density + material for interior + face-adjacent border voxels (excludes diagonal border voxels for stability) + sharpness values + material colors + meshing parameters
**Version:** CACHE_VERSION = 8 (incremented from 7)
**LRU eviction:** In-memory `MeshCacheLru` tracks per-chunk entries by `Instant` last-access time. Configurable `max_size_bytes` (default 256 MB) with batch eviction (default 64 entries per pass).

**Assessment against goals:**

- [RESOLVED] **Cache size is bounded.** LRU eviction with configurable max (256 MB default) prevents unbounded growth. Eviction deletes oldest-accessed entries in batches. `AtomicCacheStats` tracks hits, misses, errors, evictions, total bytes — exposed in the debug UI.
- [RESOLVED] **Voxel data is now persisted independently.** The SQLite persistence layer (§2.5b) stores voxel edits. Revisiting a chunk loads from DB + regeneration overlay rather than regenerating from scratch. The mesh cache remains useful for avoiding remeshing but is no longer the sole persistence mechanism.
- [RESOLVED] **Cache key is stable and deterministic.** Excludes diagonal neighbor data so cache hits are reliable regardless of which diagonal chunks are loaded. Includes material properties and meshing parameters for correctness.
- [REMAINING GAP] **Still one file per chunk.** Thousands of small files still cause filesystem overhead. Region-file consolidation remains a future improvement.
- [REMAINING GAP] **No network-aware caching.** In multiplayer, clients would need to cache server-provided chunk data.
- [OK] **6-byte compact vertex format** — 10.7× compression vs the 64-byte GPU vertex.
- [OK] **SeaHash** fast non-cryptographic hashing.
- [OK] **LZ4 compression** for fast decompression on the hot path.

### 2.5b Persistence Layer (NEW)

**Backend:** SQLite via `rusqlite` with `bundled` feature. WAL journal mode.
**File:** `saves/<world_name>/world.vxdb`
**Pragmas:** `page_size=16384`, `journal_mode=WAL`, `synchronous=NORMAL`, `cache_size=-32768` (32 MB)

**Schema:**
```
meta (key TEXT PK, value BLOB) WITHOUT ROWID
  Keys: format_version, world_seed, generator_version, created_epoch, zstd_dictionary

chunks (cx INT, cy INT, cz INT, flags INT, modified INT, data BLOB) PK(cx,cy,cz) WITHOUT ROWID
  Flags bit 0: delta(0) / full(1)
  Flags bits 1-2: compression (0=none, 2=zstd, 3=zstd+dict)
```

**Delta storage:** Only player-modified voxels are saved (5 bytes per edit: u16 index + i8 density + u16 material_id, plus optional bitflag-encoded moisture/flora fields). Auto-promotes to full chunk storage when edit count exceeds 8,192 (25% of volume). Self-healing: no-op edits (matching base terrain) are pruned on load.

**Compression:** Zstd level 3 for all persistence. Dictionary compression trained from 200+ chunk samples (32 KB dictionary stored in meta table). Dictionary-compressed chunks use flags=3.

**Concurrency:** `Mutex<Connection>` wrapper. WAL mode allows concurrent reads (streaming workers) with single writer (autosave/unload). Autosave every 30 seconds. Save-on-unload callback in streaming system.

**Assessment against goals:**

- [RESOLVED] **Full persistence layer implemented.** Multiple saved worlds, dirty tracking, delta + full storage modes, crash-safe WAL, autosave.
- [RESOLVED] **Delta storage is space-efficient.** Light survival (500 chunks, 20 edits each) → ~200 KB save. Fresh world → 0 bytes of chunk data.
- [RESOLVED] **Zstd dictionary compression** improves ratios 2–5× on small payloads vs plain zstd.
- [RESOLVED] **Self-healing edit pruning** naturally shrinks saves over time.
- [REMAINING GAP] **Single Mutex<Connection>.** Under heavy concurrent read load (many streaming workers), the Mutex could become a contention point. Separate read-only connections per worker would allow true concurrent reads.
- [REMAINING GAP] **No network-aware persistence.** Server-authoritative multiplayer will need the server to own the database and replicate chunk state to clients.

### 2.6 Streaming System

**Strategy:** Priority-queue frustum streaming with camera-relative distance ordering.

**Work queue:** `BinaryHeap<Reverse<(i32, [i32; 3])>>` — min-heap, priority = squared distance from camera chunk.
**Worker threads:** 4 background threads, each with its own `TerrainGenerator`.
**Frustum test:** Isometric view rectangle projected onto XZ plane with rotation, proportional margin + 2-chunk minimum buffer.
**Unload:** Chunks outside `unload_margin` (30% beyond visible area) removed each frame.
**Hysteresis:** `load_margin` (15%) < `unload_margin` (30%) prevents load/unload thrashing at the boundary.

**Assessment against goals:**

- [RESOLVED] **Chunk persistence on unload.** Save-on-unload callback checks `persist_dirty` and writes to SQLite before dropping. On reload, base terrain is regenerated from seed and overlaid with persisted delta edits. Self-healing prunes no-op edits during load.
- [GAP] **Y-axis streaming is static.** All chunks in the Y range [min_chunk_y, max_chunk_y) are loaded for every XZ column. At 20 vertical chunks, this means loading 20 chunks per column regardless of whether the player is on the surface or deep underground. Vertical streaming (only load Y-slices near the player's altitude, plus surface) would dramatically reduce the loaded set.
- [GAP] **No priority differentiation beyond distance.** All chunks at the same distance have equal priority. Chunks the player is looking toward, chunks containing the player's current column, and chunks needed for simulation (e.g., fluid propagation across a chunk boundary) should have elevated priority.
- [GAP] **Unload is synchronous and unbounded.** All chunks outside the unload margin are removed in a single frame. If the camera teleports (e.g., fast travel, drone view snap), this could unload hundreds of chunks in one frame — freeing GPU buffers, dropping voxel data — causing a frame spike. Now compounded by save-on-unload I/O for dirty chunks during teleport bursts.
- [GAP] **No prefetch or predictive loading.** The system only loads what's currently in the frustum plus margin. Moving the camera at speed can outpace generation, causing visible pop-in. Predictive loading based on camera velocity would help.
- [OK] **Squared-distance priority** is correct and avoids a sqrt per comparison.
- [OK] **Condvar wake** avoids busy-waiting on workers.
- [OK] **Hysteresis margins** are a sound anti-thrashing measure.

### 2.7 GPU Buffer Management

**Per-chunk buffers (3 types):**

| Buffer | Owner | Size Estimate | Allocation |
|--------|-------|---------------|------------|
| Terrain vertex + index | `Chunk.mesh` | ~156 KB (15K verts × 64B + 5K tris × 12B) | `create_buffer_init` on mesh completion |
| Vegetation instances | `VegetationPass.chunk_vegetation` | Variable (per-grass-blade instance data) | On mesh completion |
| Water vertex + index | `WaterPass.chunk_meshes` | Variable | On chunk load |

**Rendering:** Individual `draw_indexed()` call per visible chunk, per pass (terrain, vegetation, water). No batching, no indirect draw.

**Assessment against goals:**

- [GAP] **One draw call per chunk per pass.** At 500 visible chunks with 3 passes (terrain, vegetation, water), that's ~1,500 draw calls per frame. This is within budget for modern desktop GPUs at 60 fps, but the drone bird's-eye view could push visible chunk counts to 2,000+, and dense vegetation may require multiple draw calls per chunk (different plant types). Indirect drawing (one `draw_indexed_indirect` per pass with a GPU-side buffer of draw commands) would collapse this to 3 draw calls total.
- [GAP] **No buffer pooling or suballocation.** Each chunk allocates its own vertex and index buffers. Allocating and freeing hundreds of small GPU buffers per session fragments the GPU heap. A ring buffer or pool allocator (allocate chunks of a large shared buffer) would reduce allocation overhead and enable batched rendering.
- [GAP] **No frustum culling on GPU.** Frustum culling is CPU-side, building a list of visible chunks each frame. For thousands of chunks at drone-view scale, a GPU-driven culling pass (compute shader testing chunk AABBs against the frustum) would be more efficient.
- [GAP] **TerrainVertex is 64 bytes.** This is large for GPU vertex fetch. The compact 6-byte cache format proves all the information fits in 6 bytes — the GPU vertex format could be compressed similarly (quantized position relative to chunk origin, packed normal index + AO + material) and expanded in the vertex shader. This would reduce vertex buffer memory by ~12× and improve cache line utilization during vertex fetch.
- [OK] **Per-chunk buffer ownership** makes cleanup simple — drop the chunk, drop the buffer.
- [OK] **CPU frustum culling** is correct at the current scale.

---

## 3. Data Flow Summary

```
[Terrain Generation — Fully Volumetric 3D Noise]
  10× FastNoiseLite per voxel (target: 20–40×)
  No heightmap shortcut: every voxel evaluated through full 3D noise stack
       ↓
[ChunkStorage Construction]
  Uniformity check → Uniform{density,material} (~16 B) or Populated (~48 KB)
  SoA: flat i8 density array + PalettedBitArray material
  Moisture written to SimulationData tier (lazy allocation)
       ↓                                          ↓
[Persistence Overlay]                        [SQLite Load]
  If chunk has saved edits:                   load_chunk_edits(pos)
  Delta → apply VoxelEdits to storage         Self-heal: prune no-op edits
  Full → replace storage entirely
       ↓
[Arc Wrap + Insert]
  Arc::new(storage) → Chunk { storage, persist_dirty: false }
       ↓
[Snapshot Assembly]
  Arc::clone 27 storages → materialize density into flat [i8; 46_656] (~47 KB)
  Material via lazy Arc<ChunkStorage> lookup (surface voxels only)
       ↓
[Phase 1: Marching Cubes]                    [Disk Cache Check]
  Corner sampling (flat i8 array) →           SeaHash key (density+material+params)
  Case index → Edge interpolation →           LZ4 decompress → Bincode deserialize
  Flatten (3× vertex duplication) →                ↓
  Normal snap (42 dirs) →                    Cache hit: skip Phase 1+2
  AO (5³ kernel on i8 density) →
  Greedy merge (optional)
       ↓
[Phase 2: Finalize + Cache Save]
  Index extraction → LZ4 compress → Bincode serialize → Disk (LRU-bounded)
       ↓
[GPU Upload]
  Vec<TerrainVertex> → wgpu::Buffer (VERTEX)
  Vec<u32> → wgpu::Buffer (INDEX)
       ↓
[Render]
  Per-chunk draw_indexed() × {terrain, vegetation, water}
  CPU frustum cull → visible set → command encoder → submit
       ↓
[Unload Path]
  persist_dirty? → build_chunk_edits (Delta or Full) → zstd compress → SQLite
  Autosave: 30s interval saves all dirty chunks in batch transaction
```

---

## 4. Critical Gap Analysis — Prioritized by Design Goal Impact

### Resolved Gaps (from original report)

| # | Former Gap | Resolution |
|---|-----------|-----------|
| **1** | No persistence layer | ✅ SQLite with WAL mode, delta + full storage, zstd dictionary compression, autosave, save-on-unload |
| **2** | No voxel compression / sparse representation | ✅ ChunkStorage::Uniform/Populated enum, SoA density + PalettedBitArray material, tiered lazy allocation. ~7–10× memory reduction. |
| **6** | Cache architecture (no size limit, no voxel cache) | ✅ LRU eviction (256 MB cap), voxel persistence via SQLite eliminates the need for voxel caching separately |
| **14** | Snapshot allocation pressure | ✅ Reduced from ~730 KB to ~47 KB per snapshot via SoA density-only materialization |

### Tier 1: Blocking — Cannot reach design goals without addressing

| # | Gap | Affected Goals | Current State |
|---|-----|---------------|---------------|
| **3** | **Generation speed at scale** | New world startup, unexplored area streaming | 10 noise functions × 32,768 voxels × thousands of chunks. Persistence mitigates revisited-chunk cost but first-time generation remains the bottleneck. No coarse pre-sampling, no hierarchical evaluation, no SIMD/GPU acceleration. Scaling to 20–40 noise layers will compound this. |
| **4** | **No LOD system** | Drone bird's-eye view, world scale rendering | Every chunk meshes and renders at full 32³; no reduced-resolution meshes, no impostors. The drone view mechanic is completely blocked. |
| **5** | **No networking layer** | Multiplayer (8–100+ players) | Entirely single-process; no chunk replication, delta compression, or authority model. Persistence layer provides the serialization foundation but no network transport exists. |

### Tier 2: Scaling — Will hit walls as content grows

| # | Gap | Affected Goals | Current State |
|---|-----|---------------|---------------|
| **7** | **Static Y-axis loading** | Vertical scale (20 chunks) | Entire Y column loaded per XZ position; no vertical streaming. At 20Y, this means 20 chunks per column regardless of player altitude. |
| **8** | **Per-chunk draw calls** | Drone view performance, vegetation density | Individual draw_indexed per chunk per pass; no indirect draw or batching. At 500 visible chunks × 3 passes = 1,500 draw calls. Drone view pushes to 2,000+ chunks. |
| **9** | **64-byte GPU vertex** | Memory bandwidth, vertex cache | Full f32 positions/normals/colors when 6–12 bytes suffice. The 6-byte cache format proves all data fits compressed. 10.7× potential reduction. |
| **10** | **Flat-shaded vertex tripling** | Vertex count, mesh size, cache size | 3× vertex inflation for per-face normals; could be computed in shader from triangle ID + 42-normal quantization. |

### Tier 3: Quality-of-Life — Important but not blocking

| # | Gap | Affected Goals | Current State |
|---|-----|---------------|---------------|
| **11** | **No predictive/velocity-based prefetch** | Pop-in during movement | Loads only current frustum + margin |
| **12** | **No simulation infrastructure** | Living world (weather, flora, fauna) | Data structures in place (SimulationData, FloraData tiers) but no tick system, no propagation, no cross-chunk updates |
| **13** | **Vegetation system is single-type** | Visual density and diversity | Only grass blade instancing; no tree/shrub/flower pipeline |
| **14** | **No chunk state machine** | Code robustness at scale | Implicit states via bool flags; debounce + persist_dirty + mesh_seq are functional but will grow unwieldy with networking + LOD |
| **15** | **Mesh cache still one-file-per-chunk** | Filesystem overhead at scale | LRU eviction bounds size but thousands of small files still cause I/O overhead vs. region-file consolidation |
| **16** | **Single Mutex<Connection> for persistence** | Streaming worker contention | WAL allows concurrent reads but all access goes through one Mutex; separate read-only connections per worker would improve throughput |

---

## 5. Subsystem Metrics (Estimated)

These estimates are derived from code analysis, not runtime profiling. Actual measurements should be taken to validate.

| Metric | Before (flat AoS) | After (SoA + persistence) | Target (20Y, infinite XZ) |
|--------|-------------------|--------------------------|--------------------------|
| Loaded chunks (typical) | ~1,600 | ~1,600 (unchanged) | ~4,000–10,000 |
| Voxel memory | ~800 MB | **~115 MB** (est. 45% uniform + 55% populated @ ~48 KB) | ~720 MB for 10K chunks |
| Snapshot memory (in-flight) | ~5.8 MB (8 × 730 KB) | **~376 KB** (8 × 47 KB) | ~376 KB (unchanged) |
| Mesh cache on disk | Unbounded (~100–160 MB/world) | **Bounded at 256 MB** (LRU evicted) | 256 MB (configurable) |
| World save size (fresh) | N/A (no persistence) | **~0 bytes** (delta, no edits) | ~0 bytes |
| World save size (10hr survival) | N/A | **~10 MB** (est. delta edits) | ~10–50 MB |
| Draw calls/frame | ~300–600 | ~300–600 (unchanged) | ~1,500–6,000+ |
| GPU vertex memory | ~90 MB | ~90 MB (unchanged — TerrainVertex still 64B) | ~400 MB–1 GB |
| Noise evaluations (new world, 10 layers) | ~2.6 billion | ~2.6 billion (unchanged for new world) | ~13+ billion |
| Noise evaluations (existing world) | ~2.6 billion (regenerated every session) | **~0** for visited chunks (loaded from DB + regen overlay) | Amortized to once-ever |
| Noise evaluations (target 20–40 layers) | ~5–10 billion | ~5–10 billion (new world unchanged) | ~26–50+ billion |
| Mesh jobs on initial load | ~640–1,100 | ~640–1,100 (unchanged, but cached meshes hit on revisit) | ~2,800–7,000 |

---

## 6. Strengths to Preserve

These architectural decisions are sound and should be carried forward:

1. **Quarter-voxel snapping + 42-normal quantization** — Defines the visual identity and enables downstream optimizations (greedy merge, compact vertices, consistent lighting). This is a rare case where an aesthetic constraint also improves technical performance.

2. **Arc-based snapshot isolation (NEW)** — Worker threads clone `Arc<ChunkStorage>` at near-zero cost. Writers use copy-on-write semantics via `Arc::make_mut`. This is effectively MVCC for voxel data — no locks, no blocking, no permanent memory doubling. Superior to the previous heap-copy approach.

3. **SoA density array as the "hot format" (NEW)** — Flat `[i8; 32768]` density serves both storage efficiency and meshing performance simultaneously. No decompression step needed for marching cubes — the storage IS the meshing-ready format. Cache lines fill with 64 sequential density values.

4. **Delta persistence with self-healing (NEW)** — Only player edits are stored, base terrain regenerates from seed. Self-healing prunes no-op edits on load. This keeps save files tiny while enabling full world persistence. The architecture naturally supports the future networking model (server sends deltas to clients).

5. **PalettedBitArray for categorical fields (NEW)** — 75% compression on material_id with O(1) access. Auto-growing palette handles material diversity gracefully. Reusable for flora_id and future categorical fields.

6. **SeaHash + LZ4 cache format** — The compact 6-byte vertex format with fast hash and fast compression is well-engineered. Cache key excludes diagonal neighbors for stability.

7. **Priority queue streaming with hysteresis** — The min-heap distance ordering and load/unload margin separation are correct fundamentals. Now extended with save-on-unload persistence.

8. **Isometric texel snapping** — Sub-pixel camera alignment for pixel-perfect rendering is a craft-quality detail that directly serves the visual style.

9. **bevy_ecs stage scheduling** — The Input → Simulation → Meshing → UniformWrite → Render → PostFrame pipeline is clean and allows each stage to reason about data flow. Persistence integrates via the streaming unload callback and autosave timer.

---

## 7. Research Vectors — Updated Priority

### Resolved vectors (no further research needed)

| Vector | Resolution |
|--------|-----------|
| ~~Persistence format~~ | ✅ SQLite + WAL + delta storage + zstd dictionary compression. Proven architecture. |
| ~~Sparse voxel storage~~ | ✅ SoA + palette + Uniform sentinel + tiered allocation. ~7–10× memory reduction achieved. |
| ~~Cache rearchitecture~~ | ✅ LRU eviction with configurable size cap. Region-file consolidation remains a minor future improvement. |

### Active research vectors (prioritized)

| Priority | Vector | Starting Questions |
|----------|--------|--------------------|
| **1** | **Generation acceleration** | Coarse pre-sampling (4³–8³ grid) for homogeneous chunk detection? Hierarchical noise: cheap base layers first, expensive detail only near isosurface? SIMD-batch noise (fastnoise2, std::simd) for 4–8× throughput? GPU compute generation via wgpu compute shaders with voxel readback? Spatial caching of intermediate noise results across adjacent chunks? Precomputed 3D noise textures for static layers? Async generation with placeholder low-res meshes? What is the theoretical minimum noise cost per chunk given the target variety? **Note:** Persistence amortizes cost for revisited chunks, but new world creation and first-time exploration remain fully bottlenecked on generation speed. |
| **2** | **LOD pipeline** | Octree-based LOD selection? Mesh simplification vs. lower-resolution remesh at 16³ or 8³? Transition stitching between LOD levels (T-junction artifacts)? Impostor billboards for extreme distance? How does the isometric orthographic projection simplify or complicate LOD transitions vs. perspective? Directly blocks the drone bird's-eye view mechanic. |
| **3** | **GPU rendering optimization** | Compressed GPU vertex (6–12 bytes, unpack in vertex shader)? Indirect draw buffer (collapse 1,500 draw calls to 3)? Shared vertex pool with suballocation? Compute-based frustum culling for drone-view chunk counts? These three are tightly coupled — indirect draw benefits from shared pools, and compressed vertices reduce memory enabling more chunks visible. |
| **4** | **Vertical streaming** | Player-altitude-relative Y loading? Underground vs. surface detection? Column metadata (min/max solid Y) for fast skip? Interaction with LOD (surface chunks at full LOD, deep/sky at reduced)? Directly needed for the 20-vertical-chunk expansion. |
| **5** | **Networking** | Chunk delta replication (leverages existing ChunkEdits format)? Server-side meshing vs. client-side? Interest management (only replicate nearby chunks)? Compression for voxel deltas over wire (zstd dictionary already trained)? Server-owned WorldDatabase with client read-only access? |
| **6** | **Simulation tick system** | Separate tick rate from render? Chunk-local vs. cross-chunk propagation? Priority-based simulation (only tick near-player chunks)? Data structures are in place (SimulationData, FloraData tiers) — need the tick/propagation framework. |
| **7** | **Vegetation pipeline** | Hierarchical instancing (grass vs. tree vs. shrub)? Billboard LOD for distant plants? Wind animation in vertex shader? Biome-driven placement from voxel data stored in FloraData tier? |

---

## 8. Glossary

| Term | Definition |
|------|------------|
| **Chunk** | 32×32×32 voxel volume; fundamental unit of streaming, meshing, and storage |
| **ChunkStorage** | Two-variant enum: Uniform (homogeneous, ~16 B) or Populated (SoA with tiered allocation, ~48–210 KB) |
| **PalettedBitArray** | Bit-packed index array with local palette; indices don't span u64 word boundaries |
| **Tier 1–4** | Lazy allocation tiers: Hot (density+material), Warm (lighting), Cold (simulation), Sparse (flora) |
| **Delta persistence** | Storing only player-modified voxels against the regenerable seed; auto-promotes to full at 25% |
| **Snapshot** | 36×36×36 flat i8 density array (~47 KB) + Arc material references; immutable input to meshing |
| **Phase 1** | Marching cubes + flat shading + AO + greedy merge on worker thread |
| **Phase 2** | Index extraction + disk cache write on worker thread |
| **Compact vertex** | 6-byte disk format: quantized position (3B), normal index (1B), AO (1B), material (1B) |
| **TerrainVertex** | 64-byte GPU format: f32 position, normal, color, AO, material, flags, padding |
| **Greedy merge** | Coplanar triangle merging pass that reduces triangle count by combining adjacent faces |
| **Quarter-voxel snap** | Constraining MC edge interpolation to {0, 0.25, 0.5, 0.75, 1.0} for clean geometry |
| **42-normal set** | Predefined normal directions (6 axis + 12 edge + 8 corner + 16 gentle slope) |
| **Hysteresis** | Load margin < unload margin gap that prevents chunks from thrashing at the view boundary |
| **SeaHash** | Fast non-cryptographic hash used for cache keys |
| **WAL** | SQLite Write-Ahead Logging; enables concurrent reads + single writer without blocking |
| **Self-healing** | Pruning persisted edits that match the base-generated terrain on load |
| **mesh_seq** | Monotonic counter on Chunk; detects stale mesh results when chunk is edited during meshing |
