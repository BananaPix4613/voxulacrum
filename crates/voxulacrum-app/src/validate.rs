//! Graph validation CLI (roadmap §4.5, Category D).
//!
//! Runs from the normal binary behind `--validate-graphs`, parsed before the
//! event loop exists - the same shape as `--verify-generation`, and for the same
//! reason: a second `[[bin]]` would need a `lib` target to reach these modules,
//! and a flag needs neither.
//!
//! **What it is for.** Every check here corresponds to a failure this project
//! has actually hit, or to one the invalidation and resolution machinery can
//! produce silently:
//!
//! - a `kind` disagreeing with a graph's manifest position - the defect that
//!   skipped `world.graph.json` labeled `Biome` for two versions
//! - a band table assigning an id nothing implements - the defect that made half
//!   the world resolve to biome 0 the moment zone ids became load-bearing
//! - a registered biome no zone ever assigns - dead content that looks live
//! - a reference to a library or graph that does not resolve - renders pinless
//!   and invalidates its graph
//!
//! **Errors fail; warnings report.** `--strict` promotes warnings, for the gate
//! that asks for a clean lint rather than a passing one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nodegraph_eval::assignable_ids;
use nodegraph_ir::{Graph, GraphKind, GraphRefTarget, NodeKind, Severity};

use crate::world::world_generator::{world_manifest_path, WorldManifest};

/// One validation finding, attributed to the asset it came from.
struct Finding {
    severity: Severity,
    source: String,
    message: String,
}

impl Finding {
    fn error(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, source: source.into(), message: message.into() }
    }
    fn warn(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, source: source.into(), message: message.into() }
    }
}

/// Handle `--validate-graphs [--strict]`.
///
/// Returns `Some(exit_code)` when this was a validation run and the process
/// should exit, or `None` to fall through and start the engine normally.
pub fn run_from_args(args: &[String]) -> Option<i32> {
    args.iter().position(|a| a == "--validate-graphs")?;
    let strict = args.iter().any(|a| a == "--strict");
    Some(run(&world_manifest_path(), strict))
}

fn run(manifest_path: &Path, strict: bool) -> i32 {
    println!("validate-graphs: {}", manifest_path.display());
    let findings = validate(manifest_path);

    let mut errors = 0usize;
    let mut warnings = 0usize;
    for f in &findings {
        let tag = match f.severity {
            Severity::Error => {
                errors += 1;
                "ERROR"
            }
            Severity::Warning => {
                warnings += 1;
                "WARN "
            }
            Severity::Info => "INFO ",
        };
        println!("  {tag} {}: {}", f.source, f.message);
    }

    println!("  {errors} error(s), {warnings} warning(s)");
    if errors > 0 || (strict && warnings > 0) {
        println!("validate-graphs: FAILED");
        1
    } else {
        println!("validate-graphs: ok");
        0
    }
}

fn read_graph(path: &Path) -> Result<Graph, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Graph::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One loaded asset: its display name, expected kind, and parsed graph.
struct Asset {
    name: String,
    expected_kind: GraphKind,
    graph: Graph,
}

fn validate(manifest_path: &Path) -> Vec<Finding> {
    let mut out = Vec::new();

    let manifest = match WorldManifest::read(manifest_path) {
        Ok(m) => m,
        Err(e) => {
            out.push(Finding::error("manifest", e));
            return out;
        }
    };
    let dir: PathBuf = manifest_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    // --- duplicate registrations -------------------------------------------
    let mut seen = HashSet::new();
    for z in &manifest.zones {
        if !seen.insert(z.id) {
            out.push(Finding::error("manifest", format!("zone id {} registered twice", z.id)));
        }
    }
    let mut seen = HashSet::new();
    for b in &manifest.biomes {
        if !seen.insert(b.id) {
            out.push(Finding::error("manifest", format!("biome id {} registered twice", b.id)));
        }
    }

    // --- load everything the manifest names ---------------------------------
    let mut assets: Vec<Asset> = Vec::new();
    let mut load = |out: &mut Vec<Finding>, rel: &str, kind: GraphKind| {
        match read_graph(&dir.join(rel)) {
            Ok(graph) => assets.push(Asset {
                name: rel.to_string(),
                expected_kind: kind,
                graph,
            }),
            Err(e) => out.push(Finding::error(rel, e)),
        }
    };
    load(&mut out, &manifest.world.clone(), GraphKind::World);
    for z in &manifest.zones {
        load(&mut out, &z.graph, GraphKind::Zone);
    }
    for b in &manifest.biomes {
        load(&mut out, &b.graph, GraphKind::Biome);
        if let Some(d) = &b.detail {
            load(&mut out, d, GraphKind::Detail);
        }
    }

    let libraries = crate::libraries::load_libraries();
    let materials = crate::materials::load_registry();

    let world_boundary = match assets.iter_mut().find(|a| a.expected_kind == GraphKind::World) {
        Some(a) => {
            a.graph.derive_output_boundary();
            a.graph.boundary.clone()
        }
        None => Default::default(),
    };
    for asset in assets.iter_mut() {
        asset.graph.resolve_library_refs(libraries.registry());
        asset.graph.resolve_graph_refs(|t| match t {
            GraphRefTarget::World => Some(world_boundary.clone()),
            _ => None,
        });
    }

    for asset in &assets {
        // --- kind matches manifest position ---------------------------------
        if asset.graph.kind != asset.expected_kind {
            out.push(Finding::error(
                &asset.name,
                format!(
                    "registered as {:?} but its `kind` is {:?} — the editor used to \
                     drop this on save, and per-kind validation rules key on it",
                    asset.expected_kind, asset.graph.kind
                ),
            ));
        }

        // --- generic graph validation ---------------------------------------
        for d in asset.graph.validate() {
            let f = match d.severity {
                Severity::Error => Finding::error(&asset.name, d.message),
                _ => Finding::warn(&asset.name, d.message),
            };
            out.push(f);
        }

        // --- references resolve ---------------------------------------------
        // Collected per asset and reported once each: a `Layer` with eight bands
        // all naming the same missing material is one problem, not eight.
        let mut missing_materials: HashSet<u32> = HashSet::new();
        for (_, node) in asset.graph.nodes.iter() {
            for m in node.kind.material_refs() {
                if materials.get(m).is_none() {
                    missing_materials.insert(m.0 as u32);
                }
            }
            match &node.kind {
                NodeKind::LibraryRef(p) => {
                    if libraries.registry().get(p.library).is_none() {
                        out.push(Finding::error(
                            &asset.name,
                            format!("LibraryRef targets library {}, which does not exist", p.library.0),
                        ));
                    }
                }
                NodeKind::GraphRef(p) if !matches!(p.target, GraphRefTarget::World) => {
                    out.push(Finding::warn(
                        &asset.name,
                        format!(
                            "GraphRef targets {:?}, which no resolution path handles yet — \
                             it will render with no pins",
                            p.target
                        ),
                    ));
                }
                _ => {}
            }
        }

        let mut missing: Vec<u32> = missing_materials.into_iter().collect();
        missing.sort_unstable();
        for id in missing {
            out.push(Finding::error(
                &asset.name,
                format!(
                    "references material {id}, which materials.ron does not define - \
                     generation would emit an id nothing can render"
                ),
            ));
        }
    }

    // --- assignment reaches what is registered, and vice versa --------------
    let registered_zones: HashSet<u16> = manifest.zones.iter().map(|z| z.id).collect();
    let registered_biomes: HashSet<u16> = manifest.biomes.iter().map(|b| b.id).collect();

    let mut assigned_zones: HashSet<u16> = HashSet::new();
    let mut assigned_biomes: HashSet<u16> = HashSet::new();
    for asset in &assets {
        for (_, node) in asset.graph.nodes.iter() {
            match &node.kind {
                NodeKind::WorldOutput(p) => {
                    for id in assignable_ids(p.zone_bands.len(), &p.zone_ids) {
                        assigned_zones.insert(id);
                        if !registered_zones.contains(&id) {
                            out.push(Finding::error(
                                &asset.name,
                                format!(
                                    "WorldOutput can assign zone {id}, which no manifest zone \
                                     implements; every column in it falls back to biome 0"
                                ),
                            ));
                        }
                    }
                }
                NodeKind::ZoneOutput(p) => {
                    for id in assignable_ids(p.biome_bands.len(), &p.biome_ids) {
                        assigned_biomes.insert(id);
                        if !registered_biomes.contains(&id) {
                            out.push(Finding::error(
                                &asset.name,
                                format!(
                                    "ZoneOutput can assign biome {id}, which no manifest biome \
                                     implements; those columns generate air"
                                ),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    for id in registered_zones.difference(&assigned_zones) {
        out.push(Finding::warn(
            "manifest",
            format!("zone {id} is registered but no WorldOutput assigns it — dead content"),
        ));
    }
    for id in registered_biomes.difference(&assigned_biomes) {
        out.push(Finding::warn(
            "manifest",
            format!("biome {id} is registered but no ZoneOutput assigns it — dead content"),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_world_has_no_validation_errors() {
        // Warnings are allowed here — `biome_test` is registered and unassigned,
        // which is a true finding about shipped content rather than a defect in
        // the checker. Errors are not.
        let findings = validate(&world_manifest_path());
        let errors: Vec<&Finding> =
            findings.iter().filter(|f| f.severity == Severity::Error).collect();
        assert!(
            errors.is_empty(),
            "shipped assets must validate: {}",
            errors
                .iter()
                .map(|f| format!("{}: {}", f.source, f.message))
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
}
