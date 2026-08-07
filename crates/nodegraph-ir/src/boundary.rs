//! Typed boundary declarations: the named, typed inputs/outputs a graph or
//! library exposes to its consumers.
//!
//! This is the shared mechanism behind both library activation (a `LibraryRef`
//! mirrors a library's boundary as its own pins) and cross-graph dataflow (a
//! `GraphRef` mirrors another graph's declared outputs as pins). See the design
//! doc §4 ("LibraryGraph mechanics"). This module defines only the declaration
//! data; resolving a declaration into a node's pins is a later concern.

use serde::{Deserialize, Serialize};

use crate::pin::PinType;

/// One named, typed port on a graph's boundary.
///
/// `name` is how consumers reference the port at resolve time and must be
/// unique within its side (inputs or outputs). `description` is author-facing
/// documentation surfaced in the editor; it does not affect evaluation.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BoundaryPort {
    /// Stable, unique-within-side identifier consumers resolve against.
    pub name: String,
    /// The type this port carries.
    pub ty: PinType,
    /// Author-facing documentation. Empty when unset.
    #[serde(default)]
    pub description: String,
}

impl BoundaryPort {
    /// A port with the given name and type and no description.
    pub fn new(name: impl Into<String>, ty: PinType) -> Self {
        Self { name: name.into(), ty, description: String::new() }
    }

    /// Builder-style: attach author-facing documentation.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }
}

/// The declared boundary of a graph: its named typed inputs and outputs.
///
/// - Libraries declare both `inputs` (fed by a `LibraryRef`'s incoming edges)
///   and `outputs` (exposed as the `LibraryRef`'s output pins).
/// - World / Zone / Biome graphs declare `outputs` only (consumed cross-graph
///   by a `GraphRef`); their `inputs` stay empty.
///
/// Empty by default, so a graph that predates this field - or one that simply
/// exposes no boundary - round-trips unchanged.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct GraphBoundary {
    /// Named typed inputs (libraries only; empty for World/Zone/Biome).
    #[serde(default)]
    pub inputs: Vec<BoundaryPort>,
    /// Named typed outputs.
    #[serde(default)]
    pub outputs: Vec<BoundaryPort>,
}

impl GraphBoundary {
    /// True when neither side declares any port (the default / legacy state).
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() && self.outputs.is_empty()
    }

    /// The declared output port with this name, if any.
    pub fn output(&self, name: &str) -> Option<&BoundaryPort> {
        self.outputs.iter().find(|p| p.name == name)
    }

    /// The declared input port with this name, if any.
    pub fn input(&self, name: &str) -> Option<&BoundaryPort> {
        self.inputs.iter().find(|p| p.name == name)
    }

    /// Project this boundary onto a referencing node's pins: each input port
    /// becomes an input pin and each output port an output pin, carrying the
    /// port's name and type. Pins are marked non-required for now - the boundary
    /// declaration carries no per-port required/optional flag; one can be added
    /// to [`BoundaryPort`] when library evaluation needs to enforce binding.
    pub fn to_resolved(&self) -> ResolvedBoundary {
        let project = |ports: &[BoundaryPort]| {
            ports
                .iter()
                .map(|p| ResolvedPin { name: p.name.clone(), ty: p.ty, required: false })
                .collect()
        };
        ResolvedBoundary { inputs: project(&self.inputs), outputs: project(&self.outputs) }
    }
    
    /// Project only the outputs as a consumer's dynamic pins.
    /// 
    /// A `GraphRef` reads a graph's declared outputs and feeds it nothing, so
    /// its resolved boundary has no inputs. Shared by the resolve pass and the
    /// editor's insert-time binding, so a node placed from the menu and one
    /// resolved at load carry identical pins.
    pub fn to_resolved_outputs(&self) -> ResolvedBoundary {
        ResolvedBoundary {
            inputs: Vec::new(),
            outputs: self
                .outputs
                .iter()
                .map(|p| ResolvedPin { name: p.name.clone(), ty: p.ty, required: false })
                .collect(),
        }
    }
}

/// A resolved dynamic pin: a boundary port projected onto a referencing node
/// (`LibraryRef` / `GraphRef`) with an owned name - the owned-name counterpart
/// of the static [`PinSpec`](crate::PinSpec), produced by a resolve pass.
#[derive(Clone, Debug)]
pub struct ResolvedPin {
    /// Pin name (from the referenced boundary port).
    pub name: String,
    /// Pin type.
    pub ty: PinType,
    /// Whether a connection is required for validity (input pins only).
    pub required: bool,
}

/// A referencing node's resolved pins - its inputs and outputs computed from a
/// referenced [`GraphBoundary`]. Cached on the node (not serialized) by a
/// resolve pass and recomputed on load.
#[derive(Clone, Debug, Default)]
pub struct ResolvedBoundary {
    /// Resolved input pins.
    pub inputs: Vec<ResolvedPin>,
    /// Resolved output pins.
    pub outputs: Vec<ResolvedPin>,
}

/// A node's effective pin on one side, unifying static [`PinSpec`]s and resolved
/// dynamic pins ([`ResolvedPin`]). Borrows its name from whichever backs it, so
/// producing one never allocates. This is the pin identity `connect`,
/// `validate`, and the editor operate on.
#[derive(Copy, Clone, Debug)]
pub struct EffectivePin<'a> {
    /// Pin name.
    pub name: &'a str,
    /// Pin type.
    pub ty: PinType,
    /// Whether a connection is required for validity (input pins only).
    pub required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_boundary_is_empty() {
        let b = GraphBoundary::default();
        assert!(b.is_empty());
        assert!(b.output("x").is_none());
    }

    #[test]
    fn lookup_by_name_finds_ports() {
        let b = GraphBoundary {
            inputs: vec![BoundaryPort::new("distance", PinType::SurfaceField)],
            outputs: vec![
                BoundaryPort::new("weight", PinType::SurfaceField)
                    .with_description("own-biome fade weight"),
            ],
        };
        assert!(!b.is_empty());
        assert_eq!(b.input("distance").unwrap().ty, PinType::SurfaceField);
        assert_eq!(b.output("weight").unwrap().description, "own-biome fade weight");
        assert!(b.output("distance").is_none()); // wrong side
    }

    #[test]
    fn port_round_trips_through_json() {
        let p = BoundaryPort::new("radius", PinType::Scalar).with_description("fade radius");
        let json = serde_json::to_string(&p).unwrap();
        let back: BoundaryPort = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn port_json_without_description_defaults_empty() {
        let back: BoundaryPort =
            serde_json::from_str(r#"{"name":"sea_level","ty":"Scalar"}"#).unwrap();
        assert_eq!(back.description, "");
        assert_eq!(back.ty, PinType::Scalar);
    }

    #[test]
    fn to_resolved_projects_ports_to_pins() {
        let b = GraphBoundary {
            inputs: vec![BoundaryPort::new("distance", PinType::SurfaceField)],
            outputs: vec![BoundaryPort::new("weight", PinType::SurfaceField)],
        };
        let r = b.to_resolved();
        assert_eq!(r.inputs.len(), 1);
        assert_eq!(r.inputs[0].name, "distance");
        assert_eq!(r.inputs[0].ty, PinType::SurfaceField);
        assert_eq!(r.outputs[0].name, "weight");
    }
}
