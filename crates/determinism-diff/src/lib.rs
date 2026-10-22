//! Compares two determinism manifests (roadmap §4.5, T7) and names where they
//! first differ.
//!
//! A manifest is the JSON `voxulacrum-app --determinism-manifest` writes: a list
//! of sections (the generated base and each scenario), each a list of records,
//! each a list of layer hashes. CI writes one per runner and compares them with
//! this, so a cross-platform difference fails the build and the log says which
//! chunk and layer to look at.
//!
//! Every section is compared, and each reports how many of its records differ
//! and the first that does. A generation difference usually shows in the
//! scenarios too, since they run on generated chunks, so the first differing
//! generation record is the one to read first.

use serde_json::Value;

/// One section compared across the two manifests.
#[derive(Debug, PartialEq, Eq)]
pub struct SectionReport {
    pub name: String,
    /// Records in the first manifest's section.
    pub records: usize,
    /// Records that differ, counting any present in only one manifest.
    pub differing: usize,
    /// The first difference, described for the log. `None` when identical.
    pub first: Option<String>,
}

/// What the two manifests describe, for the log's heading.
#[derive(Debug, PartialEq, Eq)]
pub struct Header {
    pub namespace: String,
    pub seed: u64,
    pub platforms: [String; 2],
}

/// Compares manifest `a` with manifest `b`.
///
/// Fails, without comparing, when the two have different formats or describe
/// different worlds: their hashes would differ for a reason that is not a
/// determinism defect.
pub fn compare(a: &Value, b: &Value) -> Result<(Header, Vec<SectionReport>), String> {
    let format = |m: &Value| m["format"].as_u64();
    if format(a).is_none() || format(a) != format(b) {
        return Err(format!(
            "the manifests' formats differ ({} and {}); write both with the same build",
            a["format"], b["format"],
        ));
    }
    if a["namespace"] != b["namespace"] || a["seed"] != b["seed"] {
        return Err(format!(
            "the manifests describe different worlds ({} seed {} and {} seed {})",
            a["namespace"], a["seed"], b["namespace"], b["seed"],
        ));
    }
    let header = Header {
        namespace: a["namespace"].as_str().unwrap_or("?").to_string(),
        seed: a["seed"].as_u64().unwrap_or(0),
        platforms: [platform(a), platform(b)],
    };

    let (sa, sb) = (sections(a)?, sections(b)?);
    let mut reports: Vec<SectionReport> = sa
        .iter()
        .map(|&(name, records)| match sb.iter().find(|(n, _)| *n == name) {
            Some(&(_, other)) => compare_section(name, records, other),
            None => SectionReport {
                name: name.to_string(),
                records: records.len(),
                differing: records.len(),
                first: Some("missing from the second manifest".to_string()),
            },
        })
        .collect();
    for &(name, records) in &sb {
        if !sa.iter().any(|(n, _)| *n == name) {
            reports.push(SectionReport {
                name: name.to_string(),
                records: 0,
                differing: records.len(),
                first: Some("missing from the first manifest".to_string()),
            });
        }
    }
    Ok((header, reports))
}

fn platform(manifest: &Value) -> String {
    manifest["platform"].as_str().unwrap_or("unknown").to_string()
}

/// Each section's name and records, in manifest order.
fn sections(manifest: &Value) -> Result<Vec<(&str, &[Value])>, String> {
    let sections = manifest["sections"]
        .as_array()
        .ok_or_else(|| "a manifest has no `sections` list".to_string())?;
    sections
        .iter()
        .map(|s| match (s["name"].as_str(), s["records"].as_array()) {
            (Some(name), Some(records)) => Ok((name, records.as_slice())),
            _ => Err("a manifest section has no `name` or `records`".to_string()),
        })
        .collect()
}

fn compare_section(name: &str, a: &[Value], b: &[Value]) -> SectionReport {
    let mut differing = a.len().abs_diff(b.len());
    let mut first = None;
    for (index, (ra, rb)) in a.iter().zip(b).enumerate() {
        if ra == rb {
            continue;
        }
        differing += 1;
        if first.is_none() {
            first = Some(describe(index, ra, rb));
        }
    }
    if first.is_none() && a.len() != b.len() {
        first = Some(format!("{} records against {}", a.len(), b.len()));
    }
    SectionReport { name: name.to_string(), records: a.len(), differing, first }
}

/// What differs between two records at the same position: the record and its
/// first differing layer, or the two labels when the records name different
/// things.
fn describe(index: usize, a: &Value, b: &Value) -> String {
    let at = a["at"].as_str().unwrap_or("?");
    if a["at"] != b["at"] {
        return format!(
            "record {index} is `{at}` in one and `{}` in the other",
            b["at"].as_str().unwrap_or("?"),
        );
    }
    let kinds = match a["kinds"].as_array() {
        Some(kinds) if !kinds.is_empty() => {
            let names: Vec<&str> = kinds.iter().filter_map(Value::as_str).collect();
            format!(" [{}]", names.join(", "))
        }
        _ => String::new(),
    };
    let empty = Vec::new();
    let la = a["layers"].as_array().unwrap_or(&empty);
    let lb = b["layers"].as_array().unwrap_or(&empty);
    for (layer_a, layer_b) in la.iter().zip(lb) {
        if layer_a != layer_b {
            return format!(
                "{at}{kinds}, layer {}: {} against {}",
                layer_a["layer"].as_str().unwrap_or("?"),
                layer_a["hash"].as_str().unwrap_or("?"),
                layer_b["hash"].as_str().unwrap_or("?"),
            );
        }
    }
    format!("{at}{kinds}: {} layers against {}", la.len(), lb.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(at: &str, voxels: &str, fluid: &str) -> Value {
        json!({
            "at": at,
            "layers": [
                { "layer": "voxels", "hash": voxels },
                { "layer": "fluid", "hash": fluid },
            ],
        })
    }

    fn manifest(platform: &str, generation: Vec<Value>, fluid: Vec<Value>) -> Value {
        json!({
            "format": 1,
            "namespace": "sandbox",
            "seed": 54321,
            "platform": platform,
            "kinds": {},
            "sections": [
                { "name": "generation", "records": generation },
                { "name": "fluid", "records": fluid },
            ],
        })
    }

    fn base() -> Vec<Value> {
        vec![record("chunk (0, 0, 0)", "0x1", "0x2"), record("chunk (1, 0, 0)", "0x3", "0x4")]
    }

    #[test]
    fn identical_manifests_report_no_difference() {
        let a = manifest("windows-x86_64", base(), vec![record("step 0, chunk (0, 0, 0)", "0x5", "0x6")]);
        let b = manifest("linux-x86_64", base(), vec![record("step 0, chunk (0, 0, 0)", "0x5", "0x6")]);
        let (header, reports) = compare(&a, &b).unwrap();
        assert_eq!(header.platforms, ["windows-x86_64".to_string(), "linux-x86_64".to_string()]);
        assert!(reports.iter().all(|r| r.differing == 0 && r.first.is_none()), "{reports:?}");
    }

    #[test]
    fn the_first_differing_record_and_layer_are_named() {
        let mut changed = base();
        changed[1] = record("chunk (1, 0, 0)", "0x3", "0xF");
        changed[1]["kinds"] = json!(["tree-bearing"]);
        let mut original = base();
        original[1]["kinds"] = json!(["tree-bearing"]);
        let (_, reports) = compare(&manifest("a", original, vec![]), &manifest("b", changed, vec![])).unwrap();
        assert_eq!(reports[0].differing, 1);
        assert_eq!(
            reports[0].first.as_deref(),
            Some("chunk (1, 0, 0) [tree-bearing], layer fluid: 0x4 against 0xF"),
        );
        assert_eq!(reports[1].differing, 0, "the fluid section is unaffected");
    }

    #[test]
    fn a_missing_record_or_section_is_a_difference() {
        let mut short = base();
        short.pop();
        let (_, reports) = compare(&manifest("a", base(), vec![]), &manifest("b", short, vec![])).unwrap();
        assert_eq!(reports[0].differing, 1);
        assert_eq!(reports[0].first.as_deref(), Some("2 records against 1"));

        let mut no_fluid = manifest("b", base(), vec![]);
        no_fluid["sections"].as_array_mut().unwrap().pop();
        let (_, reports) = compare(&manifest("a", base(), vec![]), &no_fluid).unwrap();
        assert_eq!(reports[1].first.as_deref(), Some("missing from the second manifest"));
    }

    #[test]
    fn different_worlds_or_formats_are_refused() {
        let a = manifest("a", base(), vec![]);
        let mut other_seed = a.clone();
        other_seed["seed"] = json!(1);
        assert!(compare(&a, &other_seed).unwrap_err().contains("different worlds"));

        let mut other_format = a.clone();
        other_format["format"] = json!(2);
        assert!(compare(&a, &other_format).unwrap_err().contains("formats differ"));
    }
}
