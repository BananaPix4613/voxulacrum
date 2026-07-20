# Networking Architecture — Proposal

**Status:** PROPOSAL — not a settled design-doc revision. Written ahead of the player phase so
player-facing systems are built against the shapes networking will need. Companion to
`engine-design.md` §12 (Networking, Determinism) and the architecture-drift review.

The design constraint driving everything here: voxulacrum is a streamed-chunk sandbox with
join-in-progress multiplayer, a live world editor on the host, and modest player counts.
It is not a twitch shooter and not a deterministic-lockstep RTS.

---

## 1. Authority model: replicate state, not simulation

There are two ways to use this engine's determinism. **Lockstep** (Factorio): every client runs
the full simulation and only inputs cross the wire — bit-perfect determinism required everywhere,
join-in-progress means transferring the whole sim state, and any divergence is a fatal desync.
**Authoritative state** (Minecraft, Terraria): the server owns the simulation; clients render
replicated state and predict only their own avatar.

**This proposal is authoritative-state.** Lockstep is wrong for this engine for three reasons:
join-in-progress and per-client streaming radii are core to the genre; the fluid simulation is
cheap enough to run server-only; and lockstep turns every f32/SIMD/platform wobble into a fatal
fork, whereas authoritative-state turns divergence into a bounded, self-correcting rendering
artifact.

Determinism still earns its keep — in **generation**, not simulation. Clients generate terrain
locally from `(seed, graphs)` and the server ships only deviations. That is the entire bandwidth
story, and the generated/authored split (`ChunkOverrides`) is already wire-shaped.

Division of authority:

| Domain | Server | Client |
|---|---|---|
| Terrain base | generates (canonical) | generates locally from seed+graphs, verified by hash |
| Overrides / edits | validates + applies + broadcasts | predicts own edits, corrected by server |
| Fluid simulation | simulates (sole simulator) | renders received deltas; cosmetic interpolation only |
| Own player movement | simulates authoritatively | predicts + reconciles |
| Other players / entities | simulates | snapshot interpolation (~100–150 ms behind) |
| Graphs / registries / manifest | source of truth, sent at join | consumes |

---

## 2. Wire model

### 2.1 Session and handshake

- **Version lock:** protocol version = build version; mismatched clients are refused. This is
  standard for the genre and is what makes "deterministic across machines" mean "deterministic
  across identical builds," which is achievable.
- On join the server sends: world seed, the manifest + all graphs (small JSON), `materials.ron`
  and `prefabs.ron` content (registries must match or nothing renders correctly), sea level /
  layer table, and a content hash over all of it.
- **SIMD caveat:** fastnoise2 output can vary by CPU instruction set. Do not trust cross-machine
  generation blindly — see chunk hashing (§2.2). Optionally pin the SIMD level as a server setting.

### 2.2 Chunk replication: seed + overrides + fluid

Client-side flow on subscribing to a chunk:

1. Client generates the chunk locally from seed + graphs (it already has this code — the
   server-core crate is shared, §5).
2. Server sends: a **content hash of the generated base**, the chunk's `ChunkOverrides`, and the
   fluid deviation set (cells differing from generated fluid, including tombstones).
3. Client checks its generated base against the hash. Match → apply overrides + fluid, done
   (bytes on the wire: just the deviations). Mismatch → request the authoritative full chunk
   (the `TAG_FULL_POPULATED` persistence encoding is reusable). This makes generation divergence
   self-healing instead of fatal.

The persistence blob serializers (`persistence.rs`) are deterministic, versioned, key-sorted,
and bounds-checked — they are the wire encoding for chunk payloads. Do not invent a second one.

**Prerequisite exposed:** the fluid save model must become a true diff against re-derived
generated fluid, with removal tombstones (drift-review finding 1.3). On the wire, "snapshot the
whole field" has the same bugs it has on disk, plus bandwidth.

### 2.3 Mutations: commands up, deltas down

All world mutation becomes a single command enum (the same "one mutation door" argued for in the
player-phase discussion — networking is why it must exist):

- Client → server: `Command { seq, tick, kind: PlaceVoxel | RemoveVoxel | PlaceScatter |
  RemoveScatter | PourFluid | ... }`.
- Server validates (reach, permissions, placement rules), applies through the door, and
  broadcasts the resulting **authoritative delta** (chunk pos + the same override-section
  encodings) to every subscribed client, stamped with a per-chunk sequence number.
- The sender applies its command **predictively** on click (zero-latency feel), remembers the
  prediction under its `seq`, and on the server's response either confirms (delta == prediction,
  drop it) or corrects (apply the authoritative delta over the predicted state). Voxel edits are
  cell-granular and low-rate; optimistic-apply-with-rollback is simple and sufficient.
- Concurrent edits to the same cell: the server serializes; last-write-wins in server arrival
  order. No merging, no CRDTs — a cell is a u32.

The host's editor and admin tools issue the same commands in-process. One door, all writers.

### 2.4 Player movement: predict own, interpolate others

- **Fixed-timestep player sim** (30–60 Hz), implemented as a pure function
  `step(state, input, &world) -> state` in the shared server-core crate. Both sides run the
  identical function — this is the one place client and server share simulation code.
- Client: samples input per tick, sends `InputFrame { tick, move_vec, actions }`, applies it
  locally immediately (prediction), and keeps a ring buffer of unacknowledged inputs.
- Server: simulates received inputs against its authoritative world, replies with
  `{ last_processed_tick, authoritative_state }`.
- Client reconciliation: on receipt, rewind to `last_processed_tick`, snap to the authoritative
  state, replay the pending inputs. Divergence is usually zero (same pure function, same world)
  and visible only when the worlds differed (e.g. another player edited terrain under you).
- Other players: buffer snapshots and render ~100–150 ms in the past with interpolation. No
  extrapolation — for a builder game, late is better than wrong.
- The already-agreed separation (fixed sim tick, interpolated camera) is the seam that makes
  prediction possible. Build the single-player player on it from day one.

### 2.5 Interest management: per-client observer sets

§12 already specifies this: "streaming radius is configurable per observer; the union of all
radii determines the resident set." The implementation becomes:

- Each client holds a **subscription set** of chunks (its streaming radius, camera- or
  player-centered — plus the Y-bubble once tall worlds land).
- Subscribe → server sends §2.2 payload. Subscribed → server sends deltas (edits, fluid changes,
  entity snapshots scoped to those chunks). Unsubscribe → nothing.
- Server residency = union of all client subscriptions + chunks with active fluid + (later) mob
  simulation bubbles. Unobserved chunks with settled state unload exactly as today; fluid in
  unobserved chunks freezes and wakes on observation (consistent with current unload behavior).
- `ChunkStreamingManager` is the shape to generalize: it currently answers "what does the camera
  need"; it becomes "what does each observer need," and the local player is just observer 0.

### 2.6 Tick, channels, transport

- **Server tick:** fixed (20–30 Hz) for entities/commands; fluid stays a 10 Hz sub-rate on the
  same clock (`FluidClock` pattern generalizes to a server clock). The `FrameStage` schedule runs
  headless minus UniformWrite/Render — same code, fewer stages.
- **Channels:** two delivery classes are enough. Reliable-ordered per-chunk/per-stream for chunk
  payloads, command results, and edit deltas; unreliable-sequenced (latest-wins) for movement
  snapshots. In Rust: `renet` or QUIC via `quinn` (streams for reliable, datagrams for movement).
- **Pragmatic note:** Terraria shipped on plain TCP. Reliable-everything is a legitimate first
  transport if the protocol layer is abstracted (messages + sequencing designed as above,
  transport swappable). Head-of-line blocking only matters for movement feel; measure before
  adding transport complexity.
- **Serialization:** serde + bincode with a versioned envelope, mirroring persistence discipline.

---

## 3. Desync taxonomy — each class and the mechanism that kills it

| Desync class | Mechanism |
|---|---|
| Terrain divergence (client regen ≠ server) | Per-chunk base-content hash at subscribe; authoritative-fetch fallback (§2.2). Self-healing, not fatal. |
| Edit races (two clients, same cell) | Single server mutation door; per-chunk delta sequence numbers; clients apply deltas in sequence order; prediction correction (§2.3). |
| Fluid divergence | Clients never simulate fluid. Server broadcasts changed-cell deltas per fluid tick (bounded by the active set — the settled/active design directly bounds bandwidth). Client-side smoothing is cosmetic only. |
| Own-player divergence | Reconciliation: rewind + replay against authoritative state (§2.4). |
| Other-entity jitter | Snapshot interpolation behind real time; never extrapolate. |
| Out-of-order application | Server tick stamped on every message; per-chunk sequences; a client never applies delta N+1 before N for the same chunk. |
| Join-in-progress inconsistency | Subscribe payloads snapshot at a tick boundary; the fluid sim's global two-phase commit already provides a consistent point per tick. |
| Live worldgen edits (host edits a graph mid-session) | The §4 invalidation table becomes a network event: server re-overlays overrides on regenerated chunks (requires drift-review 1.1 fixed), bumps the graph hash, and re-sends affected subscribed chunks. This is the admin/god-host vision working *because of* the architecture, not despite it. |

What is deliberately **not** built: lockstep, rollback beyond own-player reconciliation, P2P,
client authority over anything, cross-fluid client simulation, account auth/encryption (later,
at the transport layer, orthogonal to this design).

---

## 4. What this demands of the current codebase (prerequisites)

In dependency order — each is valuable single-player work on its own:

1. **Single mutation door** (command enum through which editor, player tools, and later the
   network all write). Also fixes the `persist_dirty`/override-bucket ad-hocery behind the
   fluid-save bug (drift review 1.2).
2. **Fluid diff-vs-generated model with tombstones** (drift review 1.3). Disk format and wire
   format are the same problem; fix once.
3. **Regen override re-overlay** (drift review 1.1). Prerequisite for live graph editing with
   connected clients — and for not destroying player work single-player.
4. **Fixed-timestep player sim as a pure function** in shared code (player phase, day one).
5. **Server-core crate extraction**: world state, generation, fluids, overrides, persistence,
   command API — no wgpu types inside (`LoadedChunk.mesh` moves to a client-side chunk-view
   struct). The compiler enforces the client/server boundary from then on.
6. **Observer-set streaming**: generalize `ChunkStreamingManager` from camera-driven to
   per-observer subscriptions.

Binaries after the split: dev/host (server core + editor + renderer — today's exe), dedicated
server (server core, headless), player client (renderer + prediction + protocol). Single-player
ships as client + embedded server core in-process over a loopback/in-memory channel.

---

## 5. Sequencing

1. Player phase, single-player, built through the command door on a fixed sim tick (no netcode).
2. Prerequisites 1–3 land as part of it (they are bug fixes + hygiene with immediate value).
3. Server-core crate extraction (pure refactor; compiler-driven).
4. **Loopback milestone:** the engine talking to a separate client process on one machine over
   the in-memory/loopback channel. This exposes ~90% of the architectural issues (subscription
   lifecycle, prediction, delta ordering) with zero netcode polish — no NAT, no loss, no lag.
5. LAN play; then artificial latency/loss testing (a delay-injecting channel wrapper) before any
   internet exposure.

*End of proposal.*
