//! Per-`NodeKind` parameter editor widgets.

use egui::Ui;
use nodegraph_ir::{Axis, FractalType, GraphRefTarget, NodeKind};
use voxel_core::MaterialId;

/// One parameter row: a left-aligned label followed by its editor widget.
/// Returns `true` if the widget reported a change this frame.
fn row(ui: &mut Ui, label: &str, widget: impl FnOnce(&mut Ui) -> egui::Response) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        widget(ui)
    })
        .inner
        .changed()
}

/// Two tightly-related parameter fields on one row:
/// `label_a [a]   label_b [b]`. Returns `true` if either widget changed.
/// Use only for short-labeled pairs that fit the body width cap.
fn row2(
    ui: &mut Ui,
    label_a: &str,
    a: impl FnOnce(&mut Ui) -> egui::Response,
    label_b: &str,
    b: impl FnOnce(&mut Ui) -> egui::Response,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label_a);
        let ca = a(ui).changed();
        ui.label(label_b);
        let cb = b(ui).changed();
        ca || cb
    })
        .inner
}

/// The id catalogs a node's parameter UI can pick from: the world's zones and
/// biomes, each as `(id, display name)`.
///
/// Bundled rather than passed as two bare `&[(u16, String)]` arguments - those
/// are indistinguishable at a call site and would swap silently, assigning
/// biome ids to zone bands.
#[derive(Clone, Debug, Default)]
pub struct GraphCatalogs {
    /// Every zone in the world, in selector order.
    pub zones: Vec<(u16, String)>,
    /// Every biome in the world, in selector order.
    pub biomes: Vec<(u16, String)>,
    /// Every library that can be referenced, with the boundary a reference to it
    /// resolves to.
    pub libraries: Vec<LibraryChoice>,
    /// Every hierarchy graph that can be referenced cross-graph.
    pub graphs: Vec<GraphChoice>,
    /// Every blueprint on disk, by name, in selector order.
    pub blueprints: Vec<String>,
}

/// One library the author may reference, carrying the boundary a `LibraryRef` to
/// it resolves to.
///
/// The boundary travels with the choice so the editor can bind a node at the
/// moment it is placed. A reference that had to be pointed at an id and then
/// saved and reloaded to acquire pins is not authorable - it spends the interval
/// as an invalid graph.
#[derive(Clone, Debug)]
pub struct LibraryChoice {
    /// Stable library id, stored on the node.
    pub id: nodegraph_ir::LibraryGraphId,
    /// Display name (the library's stable string handle).
    pub name: String,
    /// The library's declared boundary.
    pub boundary: nodegraph_ir::GraphBoundary,
}

/// One hierarchy graph the author may reference cross-graph.
#[derive(Clone, Debug)]
pub struct GraphChoice {
    /// The target stored on the node.
    pub target: GraphRefTarget,
    /// Display name.
    pub name: String,
    /// The target's declared output boundary.
    pub boundary: nodegraph_ir::GraphBoundary,
}

/// Draw the body of a node - its parameters. Returns `true` if anything
/// was edited this frame.
pub fn params_ui(ui: &mut Ui, kind: &mut NodeKind, catalogs: &GraphCatalogs) -> bool {
    match kind {
        NodeKind::Constant(p) => row(ui, "value", |ui| {
            ui.add(egui::DragValue::new(&mut p.value).speed(0.05))
        }),
        NodeKind::Threshold(p) => row(ui, "threshold", |ui| {
            ui.add(egui::DragValue::new(&mut p.threshold).speed(0.05))
        }),
        NodeKind::Clamp(p) => row2(
            ui,
            "min", |ui| ui.add(egui::DragValue::new(&mut p.min).speed(0.05)),
            "max", |ui| ui.add(egui::DragValue::new(&mut p.max).speed(0.05)),
        ),
        NodeKind::Remap(p) => {
            let mut changed = false;
            changed |= row2(
                ui,
                "src lo", |ui| ui.add(egui::DragValue::new(&mut p.src_lo).speed(0.05)),
                "src hi", |ui| ui.add(egui::DragValue::new(&mut p.src_hi).speed(0.05)),
            );
            changed |= row2(
                ui,
                "dst lo", |ui| ui.add(egui::DragValue::new(&mut p.dst_lo).speed(0.05)),
                "dst hi", |ui| ui.add(egui::DragValue::new(&mut p.dst_hi).speed(0.05)),
            );
            changed
        }
        NodeKind::Output(p) => row(ui, "label", |ui| ui.text_edit_singleline(&mut p.label)),
        NodeKind::Perlin2D(p) | NodeKind::Perlin3D(p)
        | NodeKind::Simplex2D(p) | NodeKind::Simplex3D(p) => noise_params_ui(ui, p),
        NodeKind::WorldAxis(p) => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("axis");
                egui::ComboBox::from_id_salt("world_axis")
                    .selected_text(format!("{:?}", p.axis))
                    .show_ui(ui, |ui| {
                        for ax in [Axis::X, Axis::Y, Axis::Z] {
                            if ui.selectable_value(&mut p.axis, ax, format!("{:?}", ax)).clicked() {
                                changed = true;
                            }
                        }
                    });
            });
            changed
        }
        NodeKind::DomainWarp(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "frequency", |ui| ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001)));
            changed |= row(ui, "amplitude", |ui| ui.add(egui::DragValue::new(&mut p.amplitude).speed(0.1)));
            changed
        }
        NodeKind::CurveMapper(p) => curve_stops_ui(ui, &mut p.stops),
        NodeKind::ConstantMaterial(p) => material_id_ui(ui, &mut p.material, "material"),
        NodeKind::Layer(p) => layer_bands_ui(ui, p),
        NodeKind::JitteredGrid(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "cell size", |ui| ui.add(egui::DragValue::new(&mut p.cell_size).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "jitter", |ui| ui.add(egui::DragValue::new(&mut p.jitter).speed(0.02).range(0.0..=1.0)));
            changed |= row(ui, "density", |ui| ui.add(egui::DragValue::new(&mut p.density).speed(0.02).range(0.0..=1.0)));
            changed
        }
        NodeKind::PoissonDisk(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "radius", |ui| ui.add(egui::DragValue::new(&mut p.radius).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "k", |ui| ui.add(egui::DragValue::new(&mut p.k).range(1..=64)));
            changed
        }
        NodeKind::FindFlat(p) => {
            let mut changed = row(ui, "max step", |ui| ui.add(egui::DragValue::new(&mut p.max_step).range(0..=16)));
            changed |= material_list_ui(ui, &mut p.on_materials);
            changed
        }
        NodeKind::PlaceTree(p) => {
            let mut changed = false;
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "trunk min", |ui| ui.add(egui::DragValue::new(&mut p.trunk_min).range(1..=32)));
            changed |= row(ui, "trunk max", |ui| ui.add(egui::DragValue::new(&mut p.trunk_max).range(1..=32)));
            changed |= row(ui, "canopy radius", |ui| ui.add(egui::DragValue::new(&mut p.canopy_radius).range(1..=8)));
            changed |= material_id_ui(ui, &mut p.trunk_material, "trunk");
            changed |= material_id_ui(ui, &mut p.leaf_material, "leaf");
            changed
        }
        NodeKind::PlaceBlueprint(p) => {
            name_combo(ui, "place_blueprint", "blueprint", &mut p.blueprint, &catalogs.blueprints)
        }
        NodeKind::PlaceStructure(p) => {
            let mut changed = name_combo(
                ui,
                "structure_blueprint",
                "blueprint",
                &mut p.blueprint,
                &catalogs.blueprints,
            );
            changed |= row(ui, "cell size", |ui| ui.add(egui::DragValue::new(&mut p.cell_size).range(4..=512)));
            changed |= row(ui, "density", |ui| ui.add(egui::DragValue::new(&mut p.density).speed(0.01).range(0.0..=1.0)));
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "random yaw", |ui| ui.checkbox(&mut p.random_yaw, ""));
            changed |= row(ui, "surface offset", |ui| ui.add(egui::DragValue::new(&mut p.surface_offset).range(-8..=8)));
            changed
        }
        NodeKind::River(p) => {
            let mut changed = row(ui, "cell size", |ui| ui.add(egui::DragValue::new(&mut p.cell_size).range(16.0..=1024.0)));
            changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "potential freq", |ui| ui.add(egui::DragValue::new(&mut p.potential_frequency).speed(0.0005).range(0.0001..=0.05)));
            changed |= row2(ui, "bed low", |ui| ui.add(egui::DragValue::new(&mut p.bed_low)), "high", |ui| ui.add(egui::DragValue::new(&mut p.bed_high)));
            changed |= row2(ui, "width min", |ui| ui.add(egui::DragValue::new(&mut p.min_width).range(0.5..=64.0)), "max", |ui| ui.add(egui::DragValue::new(&mut p.max_width).range(0.5..=64.0)));
            changed |= row(ui, "depth", |ui| ui.add(egui::DragValue::new(&mut p.depth).range(0.0..=32.0)));
            changed
        }
        NodeKind::LibraryRef(p) => library_ref_ui(ui, p, &catalogs.libraries),
        NodeKind::GraphRef(p) => graph_ref_ui(ui, p, &catalogs.graphs),
        NodeKind::GraphOutput(p) => {
            row(ui, "output name", |ui| ui.text_edit_singleline(&mut p.name))
        }
        NodeKind::SurfaceNoise(p) => noise_params_ui(ui, p),
        NodeKind::WorldOutput(p) => world_output_ui(ui, p, &catalogs.zones),
        NodeKind::ZoneOutput(p) => zone_bands_ui(ui, p, &catalogs.biomes),
        NodeKind::YBand(p) => row2(
            ui,
            "min y", |ui| ui.add(egui::DragValue::new(&mut p.min).speed(0.5)),
            "max y", |ui| ui.add(egui::DragValue::new(&mut p.max).speed(0.5)),
        ),
        NodeKind::PoissonDistribution(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "radius", |ui| ui.add(egui::DragValue::new(&mut p.radius).speed(0.1).range(0.5..=64.0)));
            changed |= row(ui, "jitter", |ui| ui.add(egui::DragValue::new(&mut p.jitter).speed(0.02).range(0.0..=1.0)));
            changed
        }
        NodeKind::SurfaceFilter(p) => {
            let mut changed = row(ui, "max slope", |ui| ui.add(egui::DragValue::new(&mut p.max_slope).speed(0.05).range(0.0..=10.0)));
            changed |= row(ui, "min height", |ui| ui.add(egui::DragValue::new(&mut p.min_height).speed(0.5)));
            changed |= row(ui, "max height", |ui| ui.add(egui::DragValue::new(&mut p.max_height).speed(0.5)));
            changed |= material_list_ui(ui, &mut p.materials);
            changed
        }
        NodeKind::BiomeContextMask(p) => row(ui, "biome", |ui| ui.add(egui::DragValue::new(&mut p.biome))),
        NodeKind::SpeciesPicker(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= band_list_ui(ui, "species weights", &mut p.weights);
            changed
        }
        NodeKind::PaintDensity(p) => {
            let mut changed = row(ui, "layer id", |ui| ui.add(egui::DragValue::new(&mut p.layer_id)));
            changed |= row(ui, "species", |ui| ui.add(egui::DragValue::new(&mut p.species)));
            changed |= row(ui, "density", |ui| ui.add(egui::DragValue::new(&mut p.density)));
            changed |= row(ui, "tint", |ui| ui.add(egui::DragValue::new(&mut p.tint)));
            changed
        }
        NodeKind::ScatterPlace(p) => {
            let mut changed = row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
            changed |= row(ui, "type id", |ui| ui.add(egui::DragValue::new(&mut p.type_id)));
            changed |= row(ui, "prefab id", |ui| ui.add(egui::DragValue::new(&mut p.prefab_id)));
            changed
        }
        NodeKind::BiomeParam(p) => {
            let mut changed = row(ui, "param name", |ui| ui.text_edit_singleline(&mut p.name));
            changed |= row(ui, "default", |ui| ui.add(egui::DragValue::new(&mut p.default).speed(0.05)));
            changed
        }
        NodeKind::WorldParam(p) => {
            let mut changed = row(ui, "param name", |ui| ui.text_edit_singleline(&mut p.name));
            changed |= row(ui, "default", |ui| ui.add(egui::DragValue::new(&mut p.default).speed(0.05)));
            changed
        }
        // Parameterless variants:
        NodeKind::WorldPos(_) | NodeKind::Add(_) | NodeKind::Multiply(_)
        | NodeKind::Subtract(_) | NodeKind::Min(_) | NodeKind::Max(_)
        | NodeKind::Lerp(_) | NodeKind::Union(_) | NodeKind::Intersect(_)
        | NodeKind::DensitySubtract(_) | NodeKind::Mix(_) | NodeKind::Mask(_)
        | NodeKind::Queue(_) | NodeKind::TerrainOutput(_) | NodeKind::BuildTerrain(_)
        | NodeKind::DensityOutput(_) | NodeKind::FluidOutput(_)
        | NodeKind::Abs(_) | NodeKind::SurfaceToDensity(_) => {
            ui.weak("(no parameters)");
            false
        }
    }
}

/// Retarget a `LibraryRef`, re-resolving its pins immediately.
///
/// A picker over libraries that exist, not a number: an id field can name a
/// library that is not there, and the node then renders with no pins and makes
/// its whole graph invalid. Re-resolving on the spot is the other half - pins
/// must follow the choice, not wait for a save and a reload.
fn library_ref_ui(
    ui: &mut Ui,
    p: &mut nodegraph_ir::LibraryRefParams,
    libraries: &[LibraryChoice],
) -> bool {
    let mut changed = false;
    let selected = libraries
        .iter()
        .find(|l| l.id == p.library)
        .map(|l| l.name.clone())
        .unwrap_or_else(|| format!("library {} (missing)", p.library.0));
    ui.horizontal(|ui| {
        ui.label("library");
        egui::ComboBox::from_id_salt("library_ref")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for lib in libraries {
                    if ui.selectable_label(lib.id == p.library, &lib.name).clicked()
                        && lib.id != p.library
                    {
                        p.library = lib.id;
                        p.resolved = Some(lib.boundary.to_resolved());
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// Retarget a `GraphRef`, re-resolving its pins immediately. See
/// [`library_ref_ui`] - same reasoning, same shape.
fn graph_ref_ui(
    ui: &mut Ui,
    p: &mut nodegraph_ir::GraphRefParams,
    graphs: &[GraphChoice],
) -> bool {
    let mut changed = false;
    let selected = graphs
        .iter()
        .find(|g| g.target == p.target)
        .map(|g| g.name.clone())
        .unwrap_or_else(|| format!("{:?} (missing)", p.target));
    ui.horizontal(|ui| {
        ui.label("target");
        egui::ComboBox::from_id_salt("graph_ref_target")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for g in graphs {
                    if ui.selectable_label(g.target == p.target, &g.name).clicked()
                        && g.target != p.target
                    {
                        p.target = g.target;
                        p.resolved = Some(g.boundary.to_resolved_outputs());
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// Editor for an ascending list of band thresholds (zone or biome selection),
/// labeled by `label`. Returns `true` if edited this frame.
fn band_list_ui(ui: &mut Ui, label: &str, bands: &mut Vec<f32>) -> bool {
    let mut changed = false;
    ui.weak(label);
    let mut remove = None;
    for (i, b) in bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.add(egui::DragValue::new(b).speed(0.01)).changed() {
                changed = true;
            }
            if ui.small_button("×").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        bands.remove(i);
        changed = true;
    }
    if ui.small_button("+ band").clicked() {
        bands.push(0.0);
        changed = true;
    }
    changed
}

fn noise_params_ui(ui: &mut Ui, p: &mut nodegraph_ir::NoiseParams) -> bool {
    let mut changed = false;
    changed |= row(ui, "seed", |ui| ui.add(egui::DragValue::new(&mut p.seed)));
    changed |= row(ui, "frequency", |ui| ui.add(egui::DragValue::new(&mut p.frequency).speed(0.001)));
    changed |= row(ui, "octaves", |ui| ui.add(egui::DragValue::new(&mut p.octaves).range(1..=10)));
    changed |= row(ui, "lacunarity", |ui| ui.add(egui::DragValue::new(&mut p.lacunarity).speed(0.05)));
    changed |= row(ui, "gain", |ui| ui.add(egui::DragValue::new(&mut p.gain).speed(0.05)));
    ui.horizontal(|ui| {
        ui.label("fractal");
        egui::ComboBox::from_id_salt("fractal_type")
            .selected_text(format!("{:?}", p.fractal_type))
            .show_ui(ui, |ui| {
                for ft in [FractalType::FBm, FractalType::Ridged, FractalType::PingPong] {
                    if ui.selectable_value(&mut p.fractal_type, ft, format!("{:?}", ft)).clicked() {
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// Inline drag-value for a `MaterialId` (just its u16). Authors can look up
/// IDs in the preview's material color table.
fn material_id_ui(ui: &mut Ui, id: &mut MaterialId, label: &str) -> bool {
    row(ui, label, |ui| {
        ui.add(egui::DragValue::new(&mut id.0).range(0..=u16::MAX))
    })
}

/// Editor for a `Vec<MaterialId>` (the FindFlat allow-list). Empty = any.
fn material_list_ui(ui: &mut Ui, mats: &mut Vec<MaterialId>) -> bool {
    let mut changed = false;
    ui.label("on materials (empty = any):");
    let mut remove: Option<usize> = None;
    for (i, m) in mats.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("mat");
            changed |= ui.add(egui::DragValue::new(&mut m.0)).changed();
            if ui.small_button("×").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        mats.remove(i);
        changed = true;
    }
    if ui.button("+ material").clicked() {
        mats.push(MaterialId(6));
        changed = true;
    }
    changed
}

fn layer_bands_ui(ui: &mut Ui, p: &mut nodegraph_ir::LayerParams) -> bool {
    let mut changed = false;
    ui.label("Bands (top to down):");
    let mut remove: Option<usize> = None;
    for (i, (mat, thickness)) in p.bands.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("mat");
            changed |= ui.add(egui::DragValue::new(&mut mat.0)).changed();
            ui.label("×");
            changed |= ui
                .add(egui::DragValue::new(thickness).range(1u32..=64))
                .changed();
            if ui.small_button("×").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        p.bands.remove(i);
        changed = true;
    }
    if ui.button("+ band").clicked() {
        p.bands.push((MaterialId(1), 1));
        changed = true;
    }
    ui.separator();
    changed |= material_id_ui(ui, &mut p.fill, "fill");
    changed
}

fn curve_stops_ui(ui: &mut Ui, stops: &mut Vec<(f32, f32)>) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    for (i, (x, y)) in stops.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label("x");
            changed |= ui.add(egui::DragValue::new(x).speed(0.02)).changed();
            ui.label("y");
            changed |= ui.add(egui::DragValue::new(y).speed(0.02)).changed();
            if ui.small_button("×").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        stops.remove(i);
        changed = true;
    }
    if ui.button("+ stop").clicked() {
        let last = stops.last().copied().unwrap_or((0.0, 0.0));
        stops.push((last.0 + 0.1, last.1));
        changed = true;
    }
    changed
}

/// Editor for a ZoneOutput's climate→biome assignment: a biome picker per band
/// region, interleaved with the climate thresholds that separate them. Region k's
/// biome is `biome_ids[k]`, kept sized to `biome_bands.len() + 1`. Artists pick the
/// biome by name, so ids need not follow band order.
fn zone_bands_ui(
    ui: &mut Ui,
    p: &mut nodegraph_ir::ZoneOutputParams,
    biomes: &[(u16, String)],
) -> bool {
    let mut changed = false;
    // Keep one editable biome slot per region. Growing to fit isn't a user edit,
    // so it doesn't set `changed`; it stabilizes after the first frame.
    let regions = p.biome_bands.len() + 1;
    while p.biome_ids.len() < regions {
        p.biome_ids.push(p.biome_ids.len() as u16);
    }
    p.biome_ids.truncate(regions);

    ui.weak("Climate bands (low to high):");
    let mut remove: Option<usize> = None;
    for region in 0..regions {
        ui.horizontal(|ui| {
            ui.label(format!("band {region}:"));
            changed |= id_combo(ui, ("zone_biome", region), &mut p.biome_ids[region], biomes, "biome");
        });
        if region < p.biome_bands.len() {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(">=");
                changed |= ui
                    .add(egui::DragValue::new(&mut p.biome_bands[region]).speed(0.01))
                    .changed();
                if ui.small_button("×").on_hover_text("Remove threshold").clicked() {
                    remove = Some(region);
                }
            });
        }
    }
    if let Some(i) = remove {
        p.biome_bands.remove(i);
        if i + 1 < p.biome_ids.len() {
            p.biome_ids.remove(i + 1); // drop the region that merged away
        }
        changed = true;
    }
    if ui.button("+ threshold").clicked() {
        let last = p.biome_bands.last().copied().unwrap_or(0.0);
        p.biome_bands.push(last + 0.1);
        p.biome_ids.push(p.biome_ids.last().copied().unwrap_or(0));
        changed = true;
    }
    changed
}

/// Labeled picker over string-keyed assets, the string counterpart to
/// [`id_combo`].
///
/// A value absent from the catalog renders as `name (missing)` rather than being
/// cleared or snapped to something that exists. A renamed asset should be
/// visible as broken: silently reassigning it turns a fixable error into wrong
/// content, which is the failure mode named references exist to prevent.
fn name_combo(
    ui: &mut Ui,
    salt: &'static str,
    label: &str,
    name: &mut String,
    catalog: &[String],
) -> bool {
    let mut changed = false;
    let selected = if catalog.iter().any(|c| c == name) {
        name.clone()
    } else if name.is_empty() {
        "(none)".to_string()
    } else {
        format!("{name} (missing)")
    };
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if catalog.is_empty() {
                    ui.weak("no blueprints in assets/blueprints");
                }
                for candidate in catalog {
                    if ui.selectable_label(candidate == name, candidate).clicked()
                        && candidate != name
                    {
                        *name = candidate.clone();
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// A combo box selecting one id from a catalog of `(id, display name)`.
///
/// Shared by the zone and biome pickers. The catalog is the *only* source of
/// choices, so a band cannot be assigned an id nothing implements - the failure
/// this guards against was a `WorldOutput` assigning zone 1 in a world with one
/// zone, which produced silently different terrain and no message.
///
/// An id absent from the catalog still renders, marked `(missing)`: hiding it
/// would make an already-broken graph look fine.
fn id_combo(
    ui: &mut Ui,
    salt: (&'static str, usize),
    id: &mut u16,
    catalog: &[(u16, String)],
    noun: &str,
) -> bool {
    let mut changed = false;
    let selected = catalog
        .iter()
        .find(|(cid, _)| *cid == *id)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| format!("{noun} {id} (missing)"));
    egui::ComboBox::from_id_salt(salt)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for (cid, name) in catalog {
                if ui.selectable_label(*cid == *id, name).clicked() && *id != *cid {
                    *id = *cid;
                    changed = true;
                }
            }
        });
    changed
}

/// Editor for a WorldOutput's climate->zone assignment: a zone picker per band
/// region, interleaved with the climate thresholds that separate them.
///
/// Mirrors [`zone_bands_ui`], and exists for the same reason: `band_id` falls
/// back to the *band index* when the id list is short, so leaving ids unset does
/// not mean "zone 0" - it means "zone 0, 1, 2 …". Keeping one slot per band
/// filled makes that fallback unreachable from the editor.
fn world_output_ui(
    ui: &mut Ui,
    p: &mut nodegraph_ir::WorldOutputParams,
    zones: &[(u16, String)],
) -> bool {
    let mut changed = false;
    // Keep one editable zone slot per region. Growing to fit isn't a user edit,
    // so it doesn't set `changed`; it stabilizes after the first frame.
    let regions = p.zone_bands.len() + 1;
    let fallback = zones.first().map(|(id, _)| *id).unwrap_or(0);
    while p.zone_ids.len() < regions {
        p.zone_ids.push(fallback);
    }
    p.zone_ids.truncate(regions);

    ui.weak("Climate bands (low to high):");
    let mut remove: Option<usize> = None;
    for region in 0..regions {
        ui.horizontal(|ui| {
            ui.label(format!("band {region}:"));
            changed |= id_combo(ui, ("world_zone", region), &mut p.zone_ids[region], zones, "zone");
        });
        if region < p.zone_bands.len() {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(">=");
                changed |= ui
                    .add(egui::DragValue::new(&mut p.zone_bands[region]).speed(0.01))
                    .changed();
                if ui.small_button("×").on_hover_text("Remove threshold").clicked() {
                    remove = Some(region);
                }
            });
        }
    }
    if let Some(i) = remove {
        p.zone_bands.remove(i);
        if i + 1 < p.zone_ids.len() {
            p.zone_ids.remove(i + 1); // drop the region that merged away
        }
        changed = true;
    }
    if ui.button("+ threshold").clicked() {
        let last = p.zone_bands.last().copied().unwrap_or(0.0);
        p.zone_bands.push(last + 0.1);
        p.zone_ids.push(p.zone_ids.last().copied().unwrap_or(fallback));
        changed = true;
    }
    changed
}
