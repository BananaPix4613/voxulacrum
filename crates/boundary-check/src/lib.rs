//! Checks the engine/game boundary: a workspace member that is not an engine
//! crate may depend on the `voxulacrum` facade and on no other engine crate.
//!
//! The engine crates are listed in the root `Cargo.toml` under
//! `[workspace.metadata.voxulacrum] engine-crates`. Every member not on that
//! list is a consumer (the testbed now, games later), so an engine crate left
//! off the list fails the check rather than slipping past it.
//!
//! The input is the JSON from `cargo metadata --format-version 1 --no-deps`.
//! Only direct dependencies are checked: what a consumer reaches through the
//! facade is allowed by definition.

use serde_json::Value;

/// The one engine crate a consumer may depend on.
pub const FACADE: &str = "voxulacrum";

/// The result of a passing check.
#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    /// How many crates the engine list names.
    pub engine_crates: usize,
    /// Workspace members that are not engine crates, in metadata order.
    pub consumers: Vec<String>,
}

/// Check `metadata`. On failure, returns one message per problem found.
pub fn check(metadata: &Value) -> Result<Summary, Vec<String>> {
    let engine = engine_crates(metadata)?;
    let member_ids: Vec<&Value> = metadata["workspace_members"]
        .as_array()
        .map(|ids| ids.iter().collect())
        .unwrap_or_default();
    let members: Vec<&Value> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| member_ids.contains(&&p["id"]))
        .collect();
    let is_engine = |name: &str| engine.iter().any(|e| e == name);

    let mut errors = Vec::new();
    if !is_engine(FACADE) {
        errors.push(format!("engine-crates does not list the facade, `{FACADE}`"));
    }
    for name in &engine {
        if !members.iter().any(|p| p["name"] == name.as_str()) {
            errors.push(format!("engine-crates lists `{name}`, which is not a workspace member"));
        }
    }

    let mut consumers = Vec::new();
    for package in &members {
        let name = package["name"].as_str().unwrap_or("<unnamed>");
        if is_engine(name) {
            continue;
        }
        consumers.push(name.to_string());
        for dep in package["dependencies"].as_array().into_iter().flatten() {
            // `name` is the package name even when the dependency is renamed.
            let dep_name = dep["name"].as_str().unwrap_or_default();
            if dep_name == FACADE || !is_engine(dep_name) {
                continue;
            }
            let kind = dep["kind"].as_str().unwrap_or("normal");
            let place = match dep["target"].as_str() {
                Some(target) => format!("{kind}, {target}"),
                None => kind.to_string(),
            };
            errors.push(format!(
                "{name}: depends on {dep_name} ({place}); depend on {FACADE} instead"
            ));
        }
    }

    if errors.is_empty() {
        Ok(Summary { engine_crates: engine.len(), consumers })
    } else {
        Err(errors)
    }
}

/// The `engine-crates` list from the workspace metadata.
fn engine_crates(metadata: &Value) -> Result<Vec<String>, Vec<String>> {
    let Some(list) = metadata["metadata"]["voxulacrum"]["engine-crates"].as_array() else {
        return Err(vec![
            "no `engine-crates` list under `[workspace.metadata.voxulacrum]` in the root Cargo.toml"
                .to_string(),
        ]);
    };
    list.iter()
        .map(|v| match v.as_str() {
            Some(name) => Ok(name.to_string()),
            None => Err(vec![format!("engine-crates entry {v} is not a string")]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ENGINE: [&str; 3] = ["voxel-core", "voxulacrum-app", "voxulacrum"];

    fn dep(name: &str, kind: Option<&str>) -> Value {
        json!({ "name": name, "kind": kind, "rename": null, "target": null })
    }

    /// Metadata for a workspace with the three engine crates plus `extra`.
    fn workspace(engine: Value, extra: Vec<(&str, Vec<Value>)>) -> Value {
        let mut packages = vec![
            ("voxel-core", vec![]),
            ("voxulacrum-app", vec![dep("voxel-core", None)]),
            ("voxulacrum", vec![dep("voxulacrum-app", None)]),
        ];
        packages.extend(extra);
        let packages: Vec<Value> = packages
            .into_iter()
            .map(|(name, deps)| {
                json!({ "name": name, "id": format!("path+file:///ws/{name}#0.4.2"), "dependencies": deps })
            })
            .collect();
        let ids: Vec<Value> = packages.iter().map(|p| p["id"].clone()).collect();
        json!({
            "packages": packages,
            "workspace_members": ids,
            "metadata": { "voxulacrum": { "engine-crates": engine } },
        })
    }

    fn one_error(metadata: &Value) -> String {
        let errors = check(metadata).expect_err("check should fail");
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors.into_iter().next().unwrap()
    }

    #[test]
    fn engine_crates_may_depend_on_each_other() {
        let summary = check(&workspace(json!(ENGINE), vec![])).unwrap();
        assert_eq!(summary, Summary { engine_crates: 3, consumers: vec![] });
    }

    #[test]
    fn consumer_on_the_facade_passes() {
        let ws = workspace(
            json!(ENGINE),
            vec![("testbed", vec![dep("voxulacrum", None), dep("serde", None)])],
        );
        let summary = check(&ws).unwrap();
        assert_eq!(summary.consumers, vec!["testbed".to_string()]);
    }

    #[test]
    fn consumer_on_an_engine_crate_fails_for_every_kind() {
        for kind in [None, Some("dev"), Some("build")] {
            let ws = workspace(json!(ENGINE), vec![("testbed", vec![dep("voxel-core", kind)])]);
            let error = one_error(&ws);
            assert!(error.starts_with("testbed: depends on voxel-core"), "{error}");
            assert!(error.contains(kind.unwrap_or("normal")), "{error}");
        }
    }

    #[test]
    fn consumer_on_the_app_crate_fails() {
        let ws = workspace(json!(ENGINE), vec![("testbed", vec![dep("voxulacrum-app", None)])]);
        assert!(one_error(&ws).contains("voxulacrum-app"));
    }

    #[test]
    fn renamed_dependency_is_still_caught() {
        let renamed = json!({ "name": "voxel-core", "kind": null, "rename": "core", "target": null });
        let ws = workspace(json!(ENGINE), vec![("testbed", vec![renamed])]);
        assert!(one_error(&ws).contains("voxel-core"));
    }

    #[test]
    fn target_specific_dependency_names_its_target() {
        let windows_only =
            json!({ "name": "voxel-core", "kind": null, "rename": null, "target": "cfg(windows)" });
        let ws = workspace(json!(ENGINE), vec![("testbed", vec![windows_only])]);
        assert!(one_error(&ws).contains("(normal, cfg(windows))"));
    }

    #[test]
    fn unlisted_engine_crate_is_treated_as_a_consumer() {
        let ws = workspace(json!(ENGINE), vec![("server-core", vec![dep("voxel-core", None)])]);
        assert!(one_error(&ws).starts_with("server-core: depends on voxel-core"));
    }

    #[test]
    fn stale_list_entry_fails() {
        let ws = workspace(json!(["voxel-core", "voxulacrum-app", "voxulacrum", "old-name"]), vec![]);
        assert!(one_error(&ws).contains("`old-name`"));
    }

    #[test]
    fn list_without_the_facade_fails() {
        // The facade becomes a consumer, which may not depend on the app crate,
        // so this reports both problems.
        let ws = workspace(json!(["voxel-core", "voxulacrum-app"]), vec![]);
        let errors = check(&ws).expect_err("check should fail");
        assert!(errors.iter().any(|e| e.contains("does not list the facade")), "{errors:?}");
    }

    #[test]
    fn missing_list_fails() {
        let mut ws = workspace(json!(ENGINE), vec![]);
        ws["metadata"] = Value::Null;
        assert!(one_error(&ws).contains("no `engine-crates` list"));
    }
}
