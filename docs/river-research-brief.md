# River generation: research brief

Written at the pause after Phase 11 Substep 19c, for a research session on river
generation. The purpose is to state what this engine's structure actually
constrains, so options can be evaluated against real limits rather than general
best practice. Everything here is a finding from building the first attempt, not
a preference.

## What the engine imposes

These are not negotiable without a larger redesign, and any proposal should be
checked against them first.

**P1: same seed + graphs + coords produces bit-identical chunks.** Enforced in
CI (`--verify-generation`), including an order-independence sweep: a strided
subset is regenerated in reverse, sequentially, by a fresh generator, and
compared. Any river model that accumulates state across chunks fails this.

**No chunk may read another chunk's output.** Chunks generate in parallel, in
arbitrary order, on a worker pool. A river spanning many chunks must be derived
world-absolutely - each chunk independently computing the same answer for the
region it can see - or it must not span chunks at all.

**The frontier budget is 33 ms worst case at an active generation frontier**
(§ 7.3), 16.6 ms sustained. Generation is already the largest consumer. Any
per-column work multiplies by 1024 per chunk; any per-chunk work multiplies by
the streaming rate.

**A pointwise surface query is expensive.** Asking "what is the terrain height at
world (x, z)" runs a biome-layer evaluation. Structures afford it because a
density roll rejects most candidate cells before probing, so a chunk typically
probes zero columns. **A river network has no such gate**, which is the single
constraint that killed the obvious approach (flow accumulation over the real
surface) before anything was built.

**Rivers are excluded from the cross-chunk feature model** (§ 5). That model -
margin-band world-absolute derivation, used for structures - costs
`((extent + chunk_dim) / cell_dim)²` per chunk and does not survive a feature
spanning hundreds of chunks. A river is therefore a modification to the
**density field at stages 3-4**, before material (5), walkability (6) and slab
smoothing (7), so every downstream stage sees the carved surface.

**Stage ordering has one hard consequence**: scatter and foliage anchor to
pre-smoothing surfaces, so anything that lowers terrain after placement leaves
props hanging. Carving at stage 3-4 is what keeps rivers clear of that.

**Shapes are Cube, SlabBottom, SlabTop.** No sloped or partial geometry below
half-cell resolution until D1 (octant-occupancy mask) lands at 0.6.0. River banks
cannot be smooth; they will be terraced at half-voxel granularity at best.

**Fluid is a cellular automaton** over resident chunks, committed a tick at a
time through the mutation door. It is not coupled to generation. Generation can
author a static per-column water level (the existing `FluidOutput` mechanism);
making water actually *flow* along a river is 0.5.0 work and needs the sim.

## What was built, and what it produced

A coarse world grid, one hash-jittered node per cell. Each node samples an
**elevation-potential field** - a dedicated low-frequency 2D noise, deliberately
not the generated surface - and links to whichever of its eight neighbors has the
lowest potential. Links form a forest; **acyclicity is structural** because every
link strictly decreases potential. Channel floor interpolates between endpoint
node elevations; density above the floor is removed.

It works, is deterministic, and is continuous across chunk seams (verified across
72 configurations: three cell sizes x three widths x eight seeds, every one
reporting two adjacent chunks agreeing on every sampled column).

**It looks wrong.** The reported symptom: zero vertical variance, reading as a
series of ravines cutting straight through the ground.

**The cause is a design decision, not a parameter.** The bed elevation maps the
potential onto a fixed `[bed_low, bed_high]` band with no relationship to where
the terrain surface actually sits. Over any visible stretch the potential is
nearly constant, so the floor is flat, while the terrain rises and falls across
it. A flat floor cut through varying ground *is* a ravine. The design predicted
this consequence and stated it - "a river cannot follow terrain it did not
shape" - and the prediction turned out to be unacceptable rather than merely
notable.

## The open question, and the constraint it runs into

The obvious fix is to make the bed follow the terrain's large-scale elevation
rather than an independent field. The design already anticipated this as a
*content convention* ("author the world graph's elevation from the same
potential"), which is the wrong shape - two fields that must agree, kept in sync
by an author remembering to.

Making it a **graph edge** instead - a second input on the River node carrying
bed elevation per column, wired from the same node that drives the terrain -
gives one source of truth at no extra cost.

**The unresolved part is flow topology.** Choosing which way a river flows needs
the elevation field sampled *at node positions*, which are arbitrary world XZ,
not chunk-local. The pointwise sampler is world-position arithmetic and is known
to read above its own chunk window; whether it is valid outside the window in X
and Z has not been verified. If it is, one input can drive both bed and flow. If
it is not, bed and flow must come from different sources, and the problem is
open again.

## Directions worth evaluating

Not recommendations - starting points, each with the constraint it must answer.

1. **Elevation-driven bed, potential-driven topology.** Keep the current network
   for *where* rivers go; take the bed from a wired elevation input. Cheapest
   change. Open question: does a river routed by one field but incised into
   another still read correctly, or does it wander across contours?

2. **Make the potential field *be* the terrain's macro elevation**, with the
   biome's detail noise added on top rather than replacing it. Then routing and
   bed agree by construction. Open question: does this over-constrain terrain
   authoring?

3. **Precomputed river network as an asset.** Derive the network once, offline or
   at load, into a compact structure the generator samples. Sidesteps the
   per-chunk cost entirely and allows a global algorithm (real flow
   accumulation, erosion). Open questions: world size bounds, how it interacts
   with graph hot-reload, and whether it violates "generation is a pure function
   of seed and coords" or merely relocates it.

4. **Rivers as a region-graph concern (0.6.0).** The roadmap already plans a
   region graph with cross-border summary joins - the one place a genuine
   job-to-job dependency was predicted. Rivers may simply belong there rather
   than in per-chunk generation. Open question: does that leave 0.4.0 with no
   rivers at all, and is that acceptable?

Option 4 is the one to take seriously if 1 and 2 both fail, and it is a roadmap
amendment rather than a defeat - it would be the second time this phase that a
system turned out to belong to a later version than the plan assumed.

## Anything a proposal must be able to answer

- How does a chunk compute its portion without reading a neighbor's output?
- What is the per-chunk cost, and how does it scale with river length?
- Is the result identical regardless of generation order? (CI will check.)
- Does the bed follow the terrain, and where does that agreement come from?
- Does the channel have enough vertical fall for a cellular automaton to flow
  along later, or does it pool?
- What happens at a biome boundary, and at a chunk-Y boundary?
