# Editor UI Overhaul — Audits & Visual Specification

**Audit date:** 2026-06-10
**Scope:** Phase 1, Step 9 — presentation-layer rebuild of the embedded node-graph editor (`nodegraph-editor`). Three audits captured here: (9.1) the engine's existing UI style, used as the reference visual language; (9.2) `egui-snarl`'s styling/customization surface; (9.3) the resulting visual specification and the settled design decisions.
**Constraints honored:** all styling lives in the `nodegraph-editor` crate (never `voxulacrum-app`); no data-path changes; `egui-snarl` is not forked. Versions: **egui 0.32.3**, **egui-snarl 0.8.0** (per `Cargo.lock`).

---

## 1. Purpose

The editor styling was rebuilt to match the engine's existing visual quality without touching the data path or the node-graph paradigm. These audits were the read-before-plan groundwork: 9.1 established *what to match*, 9.2 established *what snarl lets us change*, and 9.3 reconciled the two into a signed-off spec that the 9.4–9.11 substeps implemented. This document is the durable record of those findings, since the running editor now embodies the decisions but not the reasoning.

---

## 2. Audit 9.1 — Engine UI Style (reference visual language)

The engine's egui UI (`crates/voxulacrum-app/src/ui/panels.rs`) is the quality bar. The editor matches its conventions rather than inventing a new visual language.

### 2.1 Layout conventions

- **Docked side panels.** Engine parameters live in `SidePanel::right("engine_params_panel")`, `default_width(320.0)`, `resizable(true)`, wrapped in a vertical `ScrollArea`. The graph editor is the mirror panel: `SidePanel::left("graph_editor_panel")` (toggled with **F2**, hidden by default).
- **Sectioned parameter stacks.** Content is a vertical stack of labeled sections separated by `ui.separator()`, each opened with `ui.heading(...)`. Parameters read top-to-bottom — a *vertical-primary* layout. This directly motivated the editor's vertical node-body stacking.

### 2.2 The change-cost dot convention

`panels.rs` annotates each parameter with a colored "cost" dot (`fn change_dot`): an `8×8` allocated rect with `painter().circle_filled(center, 4.0, color)` plus a hover tooltip. The three semantic colors:

| Color        | RGB             | Meaning                        |
|--------------|-----------------|--------------------------------|
| Green        | `(80, 200, 80)` | Instant (uniform-only)         |
| Yellow       | `(220, 200, 60)`| Requires remeshing             |
| Red          | `(220, 60, 60)` | Requires world regeneration    |

This dot idiom — small filled circle + tooltip, painted directly — was adopted wholesale for the editor's per-node **diagnostic badge** (a `10×10` rect, `circle_filled(center, 5.0, ...)`), and the diagnostic severity palette reuses the engine's red/yellow exactly:

- `Severity::Error` → `(220, 60, 60)` (same red as "world regeneration")
- `Severity::Warning` → `(220, 200, 60)` (same yellow as "remeshing")
- `Severity::Info` → `(100, 180, 230)` (new blue, no engine analogue needed)

### 2.3 DPI / sizing

The engine UI expresses all sizes in **egui points**, letting the framework scale per display. The editor follows suit: every size constant (`BODY_MAX_WIDTH = 220.0`, `OUTPUT_PIN_GAP = 6.0`, pin/badge dimensions, margins) is in points, and the editor inherits the engine's egui context and DPI when embedded — so no DPI-specific code is required.

### 2.4 Takeaways that fed the spec

1. Vertical-primary parameter stacks → node bodies must stack params vertically.
2. The filled-dot + tooltip status idiom → node diagnostic badges.
3. Shared semantic colors (red/yellow) → diagnostic severity palette.
4. Points-only sizing → DPI-safe by construction.

---

## 3. Audit 9.2 — egui-snarl Capability Audit

What `egui-snarl 0.8.0` lets us customize, confirmed by reading its source (`egui-snarl-0.8.0/src/ui.rs`). The editor's source of truth is `Snarl<NodeKind>`; snarl owns node/wire rendering, so styling happens through (a) a canvas-wide `SnarlStyle` and (b) `SnarlViewer` trait overrides.

### 3.1 `SnarlStyle` (canvas-wide)

Every field is `Option<T>`; `None` means "derive from the egui `Style`." Build via `SnarlStyle::new()` (const, all-`None`) plus a `..` struct spread. Fields relevant to us:

| Field                  | Default when `None`                         | Editor value |
|------------------------|---------------------------------------------|--------------|
| `pin_size`             | `interact_size.y * 0.6`                      | inherited |
| `pin_placement`        | `PinPlacement::Inside`                        | inherited (Inside) |
| `pin_stroke`           | `widgets.active.bg_stroke`                    | `Stroke::new(1.0, gray(30))` |
| `pin_fill` / `pin_shape` | active bg fill / `Circle`                  | set per-pin in viewer |
| `wire_width`           | `pin_size * 0.1` (~1px)                       | `2.5` |
| `wire_style`           | `WireStyle::Bezier5`                          | inherited |
| `wire_layer`           | `WireLayer::BehindNodes`                      | inherited |
| `bg_pattern`           | `Grid` (spacing `(50,50)`, **angle 1.0 rad**) | `Grid::new((50,50), 0.0)` (axis-aligned) |
| `bg_pattern_stroke`    | derived                                       | `Stroke::new(1.0, gray(40))` |
| `bg_frame`             | derived                                       | `Frame::new().fill(gray(18))` |
| `node_layout`          | `NodeLayoutKind::Coil`                         | inherited (Coil) |
| `node_frame` / `header_frame` | derived window frames                  | overridden per-node in viewer |

Notable defaults worth knowing: the background grid is **rotated ~1 radian** by default (we set angle `0.0` for an axis-aligned grid), and the default wire is barely ~1px (we widened to 2.5). These live in `crates/nodegraph-editor/src/style.rs::editor_snarl_style()`, rebuilt cheaply each frame.

### 3.2 `NodeLayoutKind`

Three layouts: **`Coil`** (default), `Sandwich`, `FlippedSandwich`. We use `Coil`, which already matches the desired arrangement — **header on top, inputs in a left column, body in the middle, outputs in a right column** — so no fight with snarl. This was the key 9.2 finding: the wanted layout is the default.

### 3.3 `SnarlViewer` hooks used

| Hook            | Gives us                                  | Editor use |
|-----------------|-------------------------------------------|------------|
| `title`         | node title string                          | display name |
| `inputs`/`outputs` | pin counts                              | from descriptor |
| `show_input`/`show_output` | a `ui` + returns `impl SnarlPin` (`PinInfo`) | label + per-pin shape/fill; output adds `OUTPUT_PIN_GAP` clearance |
| `has_body`/`show_body` | a plain `ui: &mut Ui`               | params, wrapped in `ui.vertical(...)` (see §3.5) |
| `node_frame`    | the node's `egui::Frame`                   | `inner_margin(8)` |
| `header_frame`  | the header's `egui::Frame`                 | category fill + margins + rounding |
| `show_header`   | a `ui`                                      | luminance-aware title text |
| `connect`       | connection request                          | pin-type compatibility + single-input rule |
| graph/node menus | context-menu `ui`s                        | add/delete nodes |

### 3.4 Pins: shapes, colors, wires

- **`PinShape`** has only four variants: `Circle`, `Triangle`, `Square`, `Star`. The IR has **nine `PinType`s**, so types fold into four *shape families* (shape = at-a-glance grouping; color disambiguates within a family). Mapping in `colors.rs::pin_shape`.
- **`PinInfo`** builder: `.with_shape()`, `.with_fill()`, `.with_stroke()`, `.with_wire_color()`, `.with_wire_style()`. `PinInfo::default()` leaves all `None`.
- **Wires inherit pin fill color automatically** — snarl merges the two endpoints' colors. Setting `.with_fill(pin_color(ty))` per pin therefore colors both the marker *and* its wires; no separate wire-color wiring needed.
- **`WireStyle`**: `Line`, `AxisAligned`, `Bezier3`, `Bezier5` (default). Left at default.

### 3.5 Coil body/pin layout — the two gotchas

Reading `draw_node` / `draw_body` / `draw_inputs` / `draw_outputs` in `ui.rs` surfaced two layout facts that caused real bugs until handled:

1. **`draw_body` builds the body UI with `Layout::left_to_right(Align::Min)`.** Anything `show_body` adds is placed *side-by-side*, so stacked param rows overlapped. **Fix:** wrap all body content in `ui.vertical(|ui| { ... })` so rows stack top-to-bottom. (`viewer.rs::show_body`.)
2. **Output pin rows use `Layout::right_to_left(Align::Min)`** with only `pin_size` reserved before the marker. Because outputs are right-aligned and read *toward* the pin, the marker crowded the last glyph (`out` → `ou●`). Inputs (`left_to_right`, reading *away* from the marker) don't show this. **Fix:** insert `OUTPUT_PIN_GAP` (`ui.add_space(6.0)`) before the output label so the text clears the marker. (`viewer.rs::show_output`.)

Both fixes are pure editor-crate styling — snarl is not modified.

---

## 4. Visual Specification (9.3) & Settled Decisions

The spec reconciles 9.1 (what to match) with 9.2 (what's changeable). All forks below were presented and signed off.

### 4.1 Settled decisions

- **Vertical-primary node layout.** Inputs stack in the left column, outputs in the right, parameters stack vertically in the body. Horizontal grouping is opt-in per row (the `row2` helper pairs two short-labeled fields).
- **Pin identity by shape AND color**, shape primary. Four shape families (§3.4) for at-a-glance grouping; the nine type colors disambiguate within a family.
- **Match the engine, don't reinvent.** Reuse the engine's status-dot idiom, semantic colors, vertical stacking, and points-only sizing (§2).
- **Snarl stays; editor crate owns all styling.** No fork of snarl; nothing in `voxulacrum-app`.

### 4.2 Color system (`colors.rs`)

- `pin_color(PinType)` — nine distinct colors: Scalar `(180,220,255)`, Density `(120,200,120)`, Material `(220,90,90)`, Positions `(230,200,70)`, Assignments `(230,160,70)`, Curve `(255,200,120)`, Vec3 `(255,140,200)`, BiomeId `(180,130,255)`, Terrain `(230,230,230)`.
- `pin_shape(PinType)` — families: **Circle** = Scalar/Density/Curve (continuous fields & transfer functions); **Square** = Material/BiomeId/Assignments (discrete category data); **Triangle** = Positions/Vec3 (spatial); **Star** = Terrain (terminal payload).
- `category_fill(NodeCategory)` — per-category header band color (Source blue, Math violet, Curves amber, Domain pink, Density green, Material/Output red, Positions yellow, Scanners slate, Props tan, Biome purple).
- `header_text_color(NodeCategory)` — perceptual-luminance threshold (`0.299r+0.587g+0.114b`, cutoff `140`) picks dark `gray(20)` vs light `gray(235)` title text for contrast against the band.

### 4.3 Out of scope (explicitly excluded)

Changing the node-graph paradigm; `egui_dock`; new node types; modifying snarl; accessibility beyond color+shape; theme switching; animation; mini-map; save/load UI.

---

## 5. Implementation Map

Where each audit finding landed, for cross-reference:

| Finding | File | Item |
|---------|------|------|
| Canvas-wide style (grid, wires, bg, pin stroke) | `nodegraph-editor/src/style.rs` | `editor_snarl_style()` |
| Pin colors / shapes, category fills, title contrast | `nodegraph-editor/src/colors.rs` | `pin_color`, `pin_shape`, `category_fill`, `header_text_color` |
| Header band + title, node frame, body, pins, connect rules | `nodegraph-editor/src/viewer.rs` | `GraphViewer` impl; `severity_color`, `BODY_MAX_WIDTH`, `OUTPUT_PIN_GAP` |
| Vertical param rows (`row`/`row2`) | `nodegraph-editor/src/params.rs` | `params_ui` and helpers |
| Reference UI conventions (dots, sections, sizing) | `voxulacrum-app/src/ui/panels.rs` | `change_dot`, `draw_engine_panel` (read-only reference) |

### 5.1 Known cosmetic notes (non-blocking)

- `OUTPUT_PIN_GAP = 6.0` is hand-tuned; revisit if pin sizing changes.
- `category_fill` returns the same red for `Material` and `Output` (intentional).
- `header_text_color` currently always resolves to dark text, since every category band is bright enough to exceed the luminance cutoff — the light-text branch is latent until a darker band is introduced.
