# Optimizing terrain generation in a Rust voxel engine

FastNoise2's fused SIMD node graph is the single highest-impact change available — delivering **5–8× throughput** over scalar FastNoiseLite while supporting every noise type Voxulacrum needs. Combined with algorithmic optimizations (range analysis, heightmap caching, two-pass generation), a **20–50× total speedup** is achievable on CPU alone, comfortably enabling 20–40 noise layers per voxel at real-time chunk rates. GPU compute via wgpu is viable but introduces cross-vendor determinism risks and currently lacks a complete WGSL noise library, making it a strong second-phase optimization rather than the first move.

This report synthesizes research across noise libraries, SIMD strategies, GPU compute, algorithmic optimizations, reference implementations, and practical Rust tooling — all focused on implementable solutions for a Bevy ECS + wgpu engine with 32³ marching-cubes chunks.

---

## FastNoise2 dominates the Rust noise library landscape

**FastNoise2 via `fastnoise2-rs`** (v0.4.0, MIT, actively maintained) is the clear winner for Voxulacrum's needs. It supports every required noise type — **OpenSimplex2, Cellular with Distance2Sub, FBm, Ridged, PingPong fractal, and 3D Domain Warp** (both gradient and OpenSimplex2 variants with fractal progressive/independent modes). The library compiles the same template code for SSE2, SSE4.1, AVX2, AVX512, and NEON via its FastSIMD layer, selecting the fastest at runtime.

The critical architectural advantage is the **fused node graph**. Traditional approaches evaluate each noise layer into a separate buffer, then combine — thrashing memory bandwidth with 20–40 intermediate arrays of 32,768 floats each. FastNoise2 instead keeps the entire multi-layer computation inside SIMD registers: fractal octaves, domain warps, blends, and operators all fuse into a single pass with no intermediate memory writes. A single call to `GenUniformGrid3D` on the root node evaluates the entire noise tree for all **32,768 voxels** in one FFI call, with X as the innermost SIMD-vectorized dimension.

Benchmark numbers from an Intel 7820X at 4.9 GHz (AVX2 vs. scalar FastNoiseLite) for 3D noise in millions of points per second:

| Noise type | FastNoiseLite | FastNoise2 (AVX2) | Speedup |
|---|---|---|---|
| Simplex | 36.8 | 268.4 | **7.3×** |
| Perlin | 47.9 | 261.1 | **5.4×** |
| Value | 64.1 | 494.5 | **7.7×** |
| Cellular | 12.5 | 52.4 | **4.2×** |

AVX512 roughly doubles AVX2 throughput again. For a 32³ chunk at AVX2 speeds, a single Simplex evaluation takes roughly **120 μs** — meaning even 40 fused noise layers could complete in under 5 ms per chunk.

The build requires CMake + a C++17 compiler (ClangCL recommended over MSVC, which has known SIMD bugs). The `fastnoise2-sys` crate includes FastNoise2 as a git submodule and builds from source. Setting `FASTNOISE2_LIB_DIR` to precompiled binaries simplifies CI. FFI overhead is negligible — one function call boundary amortized over 32K samples. `SafeNode` is thread-safe for concurrent generation (all `Gen*` methods are const), so wrapping in `Arc<SafeNode>` and sharing across Bevy's `AsyncComputeTaskPool` workers is the correct pattern.

**Alternatives are significantly weaker.** `noise-rs` (v0.9.0) is pure Rust with a clean trait-based API but uses f64 scalar evaluation with no SIMD or batch support — roughly 10–20× slower than FastNoise2. `simdnoise` (v3.1.6) has SSE2/AVX2 batch generation but is unmaintained since ~2019, lacks OpenSimplex2, Domain Warp, and PingPong fractal, and has no AVX512 or NEON support. Custom SIMD via `std::simd` (portable_simd) remains **nightly-only** with no stabilization timeline — open design questions around mask types, swizzle APIs, and trait hierarchies remain unresolved as of 2026. For stable-Rust SIMD needs beyond FastNoise2, the `wide` crate provides portable `f32x4`/`f32x8` types compiled to appropriate intrinsics.

### Determinism considerations across SIMD widths

Within the same SIMD width and seed, FastNoise2 produces **bit-identical results**. Across different SIMD widths (e.g., SSE4 vs. AVX2), sub-ULP floating-point differences arise from different FMA usage and operation ordering. After quantization to i8 density (-128 to 127), these differences are typically invisible — but at sign boundaries (density near zero), a single bit could flip a voxel. The practical mitigation is to target a minimum SIMD level (e.g., `x86-64-v3` for AVX2) or accept ±1-voxel surface differences across CPU generations.

---

## Algorithmic optimizations yield another 6× before SIMD

The highest-impact algorithmic improvements, ranked by implementation effort versus speedup:

**Heightmap caching** is the easiest win. For a 32×32×32 chunk, 2D height noise currently re-evaluates identically across all 32 Y layers. Computing a **32×32 heightmap once** (1,024 evaluations instead of 32,768) provides a **32× reduction** in 2D noise cost. Unity Burst benchmarks show height evaluation constitutes ~37% of total noise cost, so this alone yields roughly a **2–3× overall speedup**. Extending to a 34×34 heightmap with 1-pixel borders enables central-difference gradient computation for slope-based biome selection at only 13% additional cost.

**Range analysis for homogeneous chunk detection** eliminates 40–55% of chunks entirely. The technique, pioneered by Godot's Voxel Tools, uses interval arithmetic: each noise node computes a [min, max] output range for a given spatial box. If the entire range falls below or above zero, the chunk is provably uniform and skipped. This is grounded in **Lipschitz bounds** — for a function with maximum gradient L, a sample value v at a box center constrains all values within the box to [v − L·d, v + L·d] where d is the half-diagonal. For FBm with octaves at amplitudes aᵢ and frequencies fᵢ, the composite Lipschitz constant is Σ(aᵢ · fᵢ · L_base). Voxel Tools uses subdivision sizes of 16³ and 8³; below 8³, iteration overhead exceeds computation savings. For Voxulacrum's cave tunnels (2–4 voxels wide), an **8×8×8 coarse grid** (512 samples at 4-voxel spacing) reliably detects features ≥2 voxels wide, with an estimated 5–15% false-negative rate. Adding Lipschitz bounds eliminates false negatives entirely for well-characterized noise functions. Domain warping complicates bounds — the Lipschitz constant of f(x + w(x)) becomes L_f · (1 + L_w) — and production engines like Voxel Plugin add a 10%+ safety margin.

**Two-pass generation with early termination** provides another 1.5–3× on non-uniform chunks. Pass one evaluates structural layers (2D height + base density) and classifies each voxel as definitely solid, definitely air, or near-isosurface. Pass two evaluates detail layers (caves, overhangs, biome modifiers) only for near-isosurface voxels. The margin equals the sum of detail-layer amplitudes — typically 4–16 voxels from the base surface. For typical terrain where the surface band is ~12 voxels thick (38% of the chunk), and detail layers constitute 80% of noise cost, this eliminates roughly 50% of total evaluation. The Moinet et al. paper "Fast sphere tracing of procedural volumetric noise" formalizes this as **nested bounding volumes**: evaluate FBm octaves sequentially, and if |partial_sum| exceeds the sum of remaining amplitudes, early-terminate.

**Domain warp interpolation on coarse grids** reduces warp evaluations by 64×. Since domain warp typically uses low-frequency noise (1–2 octaves), evaluating on an 8×8×8 grid and trilinearly interpolating to full resolution introduces negligible visual error while cutting warp evaluations from 32,768 to 512.

Combined estimates for a typical chunk: heightmap caching reduces evaluations from ~1.8M to ~1.2M; chunk skipping (50% uniform) halves the average to ~600K; two-pass with early termination cuts to ~300K. That is roughly a **6× average algorithmic speedup** before any SIMD gains. Multiplied by FastNoise2's 5–8× SIMD throughput, the total improvement reaches **30–50×**.

---

## GPU compute is viable but introduces hard tradeoffs

A wgpu compute shader approach can generate density fields massively in parallel, but three constraints shape the architecture: incomplete WGSL noise libraries, single-queue limitations, and cross-vendor determinism failures.

**WGSL noise coverage is partial.** The `noisy_bevy` crate provides Simplex 2D/3D (seeded), FBm Simplex, and Worley 2D as importable WGSL functions. Munrocket's community gist adds Value and Voronoi noise. The LYGIA shader library is expanding WGSL support but remains incomplete. Critically, **no WGSL implementations exist for OpenSimplex2, 3D Cellular, Ridged Multifractal, or Domain Warp**. The most practical path is porting FastNoiseLite's HLSL version to WGSL — HLSL is syntactically closest to WGSL, and a Godot GLSL port reports 99.9% identical output to the C++ version.

**Architecture for a 32³ compute dispatch** is straightforward. A workgroup size of **(4, 4, 4) = 64 threads** is the safest cross-platform choice (WebGPU limits workgroups to 256 total invocations). Dispatching `(8, 8, 8)` workgroups covers the full 32³ grid. Each thread computes one voxel's density and writes to a storage buffer at index `x + y*32 + z*32*32`. Noise parameters (seed, frequency, octaves, chunk world position) pass as a uniform buffer. For i8 output, WGSL lacks native `i8` — use `array<i32>` and convert on CPU readback, or pack four i8 values per u32 with bit manipulation.

**GPU→CPU readback latency** is dominated by synchronization, not data transfer. A 32³ × i8 chunk is only 32 KB — microseconds over PCIe. But `map_async` + `device.poll(Maintain::Wait)` forces a full pipeline drain, typically **0.5–5 ms**. The solution is **double or triple buffering** with non-blocking polls: dispatch chunk N, copy to staging buffer N, poll staging buffer N-2 (which finished two frames ago). Bevy's `Readback` component and `ReadbackComplete` observer handle this pattern automatically. The strongest architecture avoids readback entirely for rendering: GPU generates density AND runs marching cubes, keeping mesh data on-GPU via indirect draw. Only read back the vertex count (4 bytes) plus the density field (32 KB) if gameplay needs voxel data.

**wgpu currently provides only a single queue** per device — no separate async compute queue exists. All compute and render commands serialize through the same queue. The GPU driver may overlap execution of independent passes, but there is no explicit control. The workaround is to schedule compute dispatch nodes early in Bevy's render graph, allowing the driver to overlap compute with subsequent render work.

**Cross-vendor float determinism is not achievable.** Basic arithmetic (+, −, ×) is IEEE 754 compliant across all modern GPUs, but **FMA fusion differs between vendors** (NVIDIA aggressively fuses, AMD and Intel differ), and **transcendentals (sin, cos, pow) are not required to be correctly rounded** by any GPU spec. The WebGPU specification explicitly warns that some functions produce "different results on different machines." For multiplayer, the standard approach is CPU-authoritative terrain generation — the server generates all terrain, or all clients use identical CPU noise. GPU compute is best reserved for single-player or visual-only terrain where ±1-voxel differences are acceptable.

### Memory budget for GPU bulk generation

At 128 KB per chunk (f32 density), a conservative 100 MB GPU compute budget holds ~800 chunks simultaneously. With double-buffered staging, practical batch sizes of 16–64 chunks per frame require only 4–16 MB of working set. Pre-allocate a pool of storage buffers and cycle through them.

---

## How Veloren and Minetest handle terrain generation

**Veloren** (open-source Rust voxel RPG) uses `noise-rs` for noise and `rayon` for threading — no SIMD, no GPU compute. Its architecture is two-phase: a global simulation runs tectonic uplift and fluvial erosion (based on Cordonnier et al.'s academic model) across the entire world map, taking 10–30 minutes. Individual chunk voxels generate on-demand via `tokio::spawn_blocking`. Chunks are 32×32 horizontal columns of variable height. Veloren's worldgen is notably slow and represents a cautionary tale for choosing noise-rs in a performance-critical path.

**Minetest/Luanti** uses a custom C++ Perlin noise implementation with no SIMD and no GPU acceleration. Its mapgen operates on 80×80×80 "mapchunks" (5×5×5 map blocks of 16³ each) in dedicated `EmergeThread`s. Key optimizations include buffer reuse across chunks, flat array generation (`get_3d_map_flat`), and the standard 2D+3D noise split — 2D noise for base height, 3D only for caves and overhangs. MapgenV7 takes ~200–260 ms per mapchunk.

**No major open-source Rust voxel engine currently uses GPU compute for terrain generation.** The closest reference is qhdwight's `voxel-game-rs`, which implements marching cubes in a Bevy+WGSL compute shader with `@workgroup_size(8,8,8)` and atomic vertex append. The `bevy_voxel_world` plugin uses `AsyncComputeTaskPool` for multithreaded CPU generation with LOD support. The broader ecosystem gap confirms that GPU terrain gen in Rust is frontier territory.

---

## SIMD batch evaluation patterns for the generation loop

The optimal strategy depends on whether you use FastNoise2's fused node graph or need custom per-voxel logic.

**For pure noise evaluation, `GenUniformGrid3D` is optimal.** Pass the chunk's world offset, size (32, 32, 32), and step size. The library internally walks X as the innermost dimension, processing SIMD_WIDTH consecutive X values per iteration — exactly matching the 32-element X rows of your triple-nested loop. Output is a flat `&mut [f32]` in `data[(z * 32 + y) * 32 + x]` order. This is a single FFI call per noise tree per chunk.

**For conditional logic (e.g., cave carving where density > 0),** two approaches work. First, FastNoise2's node graph can express conditional blending via Min/Max/Fade/Select operators — the entire conditional tree stays fused in SIMD. Second, if you need Rust-side branching between separate FN2 trees, generate terrain and cave noise as two separate `GenUniformGrid3D` calls, then combine with SIMD masks. The `wide` crate on stable Rust provides `f32x8` with comparison → mask → select operations. The key insight is **speculative execution**: evaluate caves for all lanes, then discard results for air lanes via `mask.select()`. The `mask.none()` / `mask.all()` checks provide batch-level early-exit — if an entire SIMD batch of 8 voxels is air, skip the cave pass entirely.

**Memory layout must be SoA** (Structure of Arrays) for SIMD. FastNoise2's `GenPositionArray3D` takes separate `x[]`, `y[]`, `z[]` float arrays — explicitly SoA. AoS (interleaved `[x,y,z,x,y,z,...]`) requires gather instructions at 3–5× penalty. For the uniform grid case, coordinates are generated internally and never materialized, which is ideal.

---

## Progressive generation and scheduling for 60 fps

Budget approximately **4 ms per frame** for terrain generation within the 16.6 ms frame budget. Bevy's `AsyncComputeTaskPool` uses ~25% of CPU cores for background work — appropriate for chunk generation. The pattern is: spawn generation tasks into the pool as `Task<ChunkData>`, poll for completion each frame via `block_on(future::poll_once(&mut task))`, and spawn mesh entities for completed chunks. Bevy does not provide built-in priority scheduling, so implement a priority queue sorted by Manhattan distance to the camera, with near chunks and LOD 0 taking precedence.

**Progressive LOD generation** starts with an 8×8×8 coarse evaluation (512 voxels, sub-millisecond) to display approximate terrain immediately, then refines to full 32×32×32. The **Transvoxel algorithm** (patent-free, used in Space Engineers and Astroneer) handles seamless mesh transitions between LOD levels using 73 equivalence classes of transition cells. Godot's Voxel Tools implements both octree-based (spherical LOD around viewer) and clipbox-based (concentric boxes, multiple-viewer-friendly) LOD systems.

For background pre-generation during loading screens, increase `AsyncComputeTaskPool` concurrency (spawn more tasks) and pre-generate chunks in a spiral pattern outward from the spawn point.

---

## Benchmarking and profiling the generation pipeline

**criterion.rs** (v0.8.2, 181M+ downloads, stable Rust) is the standard for microbenchmarking. Isolate noise cost from mesh cost by benchmarking them separately: `bench_function("noise_32x32x32", ...)` with pre-allocated output buffers, and `bench_function("mesh_only", ...)` with pre-computed density data. Use `black_box()` to prevent dead-code elimination. Always benchmark with `--release` and `RUSTFLAGS="-C target-cpu=native"` for SIMD codegen.

**Tracy** integrates via Bevy's built-in `trace_tracy` cargo feature — run with `--features bevy/trace_tracy --release`. Tracy provides nanosecond-resolution frame timelines with separate rows for CPU threads and GPU. Use `tracy-capture -o trace.tracy` to record, then analyze in the Tracy profiler GUI. GPU timings appear as a "RenderQueue" row. The `profiling` crate (used internally by wgpu) provides a thin abstraction over Tracy, Puffin, Optick, and Superluminal via `#[profiling::function]` and `profiling::scope!("name")`.

**Puffin** (by Embark Studios) offers lightweight in-game flamegraphs via `puffin_egui` at ~50–200 ns overhead per scope. The `bevy_puffin` crate bridges Bevy's tracing spans to Puffin. For Voxulacrum, the recommended setup is the `profiling` crate for your own instrumentation (switchable backends) plus Bevy's `trace_tracy` for engine internals.

---

## Practical implementation roadmap

The recommended optimization sequence, ordered by impact-per-effort:

1. **Integrate FastNoise2** via `fastnoise2-rs`. Design the noise tree in the visual Node Editor (available as a desktop app and WASM web version), export as encoded base64 strings, load at runtime with `SafeNode::from_encoded_node_tree()`. Wrap in `Arc<SafeNode>` for thread-safe sharing. This single change should deliver **5–8× throughput** on the noise path.

2. **Add heightmap caching** — compute the 34×34 2D heightmap once per chunk column, reuse across 32 Y layers. Implementation: ~half a day. Expected: **2–3× additional speedup** on height-dominated terrain.

3. **Implement range analysis** for homogeneous chunk detection. Evaluate the noise tree at coarse 8×8×8 resolution with Lipschitz-bounded interval arithmetic. Skip chunks where the value range doesn't cross zero. Implementation: ~1–2 days. Expected: eliminates **40–55% of chunks** entirely.

4. **Add two-pass generation** — structural layers first, detail layers only near the isosurface (within the sum of detail-layer amplitudes). Implementation: ~2–3 days. Expected: **1.5–3× on remaining chunks**.

5. **Port FastNoiseLite HLSL to WGSL** for GPU compute as a second-phase optimization. Start with density-only generation; keep material/biome assignment on CPU. Use double-buffered staging for async readback. Reserve for single-player or accept ±1-voxel cross-vendor differences in multiplayer.

6. **Profile continuously** with Tracy from day one. Add `profiling::scope!()` around noise generation, meshing, and chunk scheduling to track regressions and validate each optimization's impact.

## Conclusion

The path from 55 scalar noise evaluations per voxel to 40+ SIMD-fused layers at real-time rates is primarily an **algorithmic + library** problem, not a GPU problem. FastNoise2's fused node graph eliminates the memory-bandwidth bottleneck that makes naïve multi-layer evaluation expensive, while range analysis and two-pass generation eliminate the majority of redundant work at the chunk and voxel level. GPU compute is a powerful accelerator but is best deployed after CPU optimizations are in place, when the remaining bottleneck is raw arithmetic throughput rather than redundant evaluation. The Rust ecosystem's tooling — criterion for benchmarking, Tracy for profiling, Bevy's AsyncComputeTaskPool for scheduling — provides the infrastructure to measure and validate each step. The key insight from production engines like Voxel Tools is that **smart culling beats brute-force parallelism**: proving a chunk is uniform from 512 coarse samples is always faster than evaluating 32,768 voxels, regardless of SIMD width.