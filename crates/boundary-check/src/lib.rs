//! Checks the engine's two dependency rules.
//!
//! **The facade rule:** a workspace member that is not an engine crate may
//! depend on the `voxulacrum` facade and on no other engine crate. The engine
//! crates are listed in the root `Cargo.toml` under
//! `[workspace.metadata.voxulacrum] engine-crates`. Every member not on that
//! list is a consumer (the testbed now, games later), so an engine crate left
//! off the list fails the check rather than slipping past it. Only direct
//! dependencies are checked: what a consumer reaches through the facade is
//! allowed by definition.
//!
//! [`check`] reads the engine workspace itself. [`check_consumers`] reads a
//! workspace outside it, such as a game's, where every member is a consumer and
//! the list comes from the engine workspace its facade dependency belongs to.
//!
//! **The server rule** ([`check_server`]): no crate listed under `server-crates`
//! may reach a package whose name starts with an entry of `server-forbidden`
//! (the renderer, windowing and UI crates) through normal or build
//! dependencies, directly or transitively, on any target (`engine-design.md`
//! §12, "Crates and sides"). Dev-dependencies don't count: they build only
//! tests, examples and benchmarks.
//!
//! The input is the JSON from `cargo metadata --format-version 1`. The facade
//! rule reads the package list, so `--no-deps` is enough for it; the server
//! rule reads the resolve graph, so it needs the full output.

use std::collections::{HashMap, VecDeque};

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

/// Check the engine workspace's `metadata`. On failure, returns one message per
/// problem found.
pub fn check(metadata: &Value) -> Result<Summary, Vec<String>> {
    let engine = engine_crates(metadata)?;
    let members = members(metadata);
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
        errors.extend(dependency_errors(name, package, &is_engine));
    }

    if errors.is_empty() {
        Ok(Summary { engine_crates: engine.len(), consumers })
    } else {
        Err(errors)
    }
}

/// Check a workspace outside the engine, such as a game's: every member of
/// `workspace` is a consumer, and the engine crates are the ones `engine`, the
/// engine workspace's own metadata, lists. On failure, returns one message per
/// problem found.
pub fn check_consumers(workspace: &Value, engine: &Value) -> Result<Summary, Vec<String>> {
    let engine_list = engine_crates(engine)?;
    let is_engine = |name: &str| engine_list.iter().any(|e| e == name);

    let mut errors = Vec::new();
    let mut consumers = Vec::new();
    for package in members(workspace) {
        let name = package["name"].as_str().unwrap_or("<unnamed>");
        consumers.push(name.to_string());
        errors.extend(dependency_errors(name, package, &is_engine));
    }

    if errors.is_empty() {
        Ok(Summary { engine_crates: engine_list.len(), consumers })
    } else {
        Err(errors)
    }
}

/// Check the server rule over `metadata`, which must include the resolve graph
/// (read without `--no-deps`). On success, returns the server crates checked,
/// in list order; on failure, one message per problem found.
pub fn check_server(metadata: &Value) -> Result<Vec<String>, Vec<String>> {
    let servers = string_list(metadata, "server-crates")?;
    let forbidden = string_list(metadata, "server-forbidden")?;
    let Some(graph) = Graph::new(metadata) else {
        return Err(vec![
            "the metadata has no resolve graph; read it without `--no-deps`".to_string(),
        ]);
    };

    let members = members(metadata);
    let mut errors = Vec::new();
    for server in &servers {
        let id = members
            .iter()
            .find(|p| p["name"] == server.as_str())
            .and_then(|p| p["id"].as_str());
        match id {
            Some(id) => errors.extend(forbidden_errors(server, id, &graph, &forbidden)),
            None => errors.push(format!(
                "server-crates lists `{server}`, which is not a workspace member"
            )),
        }
    }

    if errors.is_empty() {
        Ok(servers)
    } else {
        Err(errors)
    }
}

/// The resolve graph: each package's name and dependency edges, by package id.
struct Graph<'a> {
    names: HashMap<&'a str, &'a str>,
    deps: HashMap<&'a str, &'a [Value]>,
}

impl<'a> Graph<'a> {
    /// `None` when `metadata` has no resolve graph.
    fn new(metadata: &'a Value) -> Option<Self> {
        let nodes = metadata["resolve"]["nodes"].as_array()?;
        let names = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?)))
            .collect();
        let deps = nodes
            .iter()
            .filter_map(|n| Some((n["id"].as_str()?, n["deps"].as_array()?.as_slice())))
            .collect();
        Some(Self { names, deps })
    }

    fn name(&self, id: &str) -> &'a str {
        self.names.get(id).copied().unwrap_or("<unknown>")
    }
}

/// One message for each forbidden package that server crate `name` (package id
/// `root`) reaches, naming the shortest chain of dependencies that reaches it.
/// The walk stops at a forbidden package, so each message names one the server
/// crate's own graph pulls in, not what that package pulls in after it.
fn forbidden_errors<'a>(
    name: &str,
    root: &'a str,
    graph: &Graph<'a>,
    forbidden: &[String],
) -> Vec<String> {
    // Each package reached, with the package and edge it was first reached by.
    let mut reached: HashMap<&'a str, Option<(&'a str, String)>> = HashMap::from([(root, None)]);
    let mut queue = VecDeque::from([root]);
    let mut errors = Vec::new();
    while let Some(id) = queue.pop_front() {
        for dep in graph.deps.get(id).copied().unwrap_or_default() {
            let Some(pkg) = dep["pkg"].as_str() else { continue };
            let Some(edge) = edge(dep) else { continue };
            if reached.contains_key(pkg) {
                continue;
            }
            reached.insert(pkg, Some((id, edge)));
            let dep_name = graph.name(pkg);
            if forbidden.iter().any(|f| dep_name.starts_with(f.as_str())) {
                errors.push(format!(
                    "{name}: reaches {dep_name} ({}); a server crate may not depend on a renderer, windowing or UI crate",
                    chain(pkg, &reached, graph)
                ));
            } else {
                queue.push_back(pkg);
            }
        }
    }
    errors
}

/// How `dep` is depended on, for the chain in a message: empty for a normal
/// dependency on every target, else its kinds, such as `build` or
/// `normal, cfg(windows)`. `None` when it is a dev-dependency only, which the
/// server rule doesn't follow.
fn edge(dep: &Value) -> Option<String> {
    let kinds: Vec<(&str, Option<&str>)> = dep["dep_kinds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|k| (k["kind"].as_str().unwrap_or("normal"), k["target"].as_str()))
        .filter(|(kind, _)| *kind != "dev")
        .collect();
    if kinds.is_empty() {
        None
    } else if kinds.contains(&("normal", None)) {
        Some(String::new())
    } else {
        let kinds: Vec<String> = kinds
            .iter()
            .map(|(kind, target)| match target {
                Some(target) => format!("{kind}, {target}"),
                None => kind.to_string(),
            })
            .collect();
        Some(kinds.join("; "))
    }
}

/// The chain of packages from the server crate to `id`, such as
/// `server -> codegen (build) -> winit`.
fn chain<'a>(id: &'a str, reached: &HashMap<&'a str, Option<(&'a str, String)>>, graph: &Graph) -> String {
    let mut links = Vec::new();
    let mut at = id;
    while let Some(Some((parent, edge))) = reached.get(at) {
        let name = graph.name(at);
        links.push(if edge.is_empty() { name.to_string() } else { format!("{name} ({edge})") });
        at = *parent;
    }
    links.push(graph.name(at).to_string());
    links.reverse();
    links.join(" -> ")
}

/// The workspace members' package entries, in metadata order.
fn members(metadata: &Value) -> Vec<&Value> {
    let member_ids: Vec<&Value> = metadata["workspace_members"]
        .as_array()
        .map(|ids| ids.iter().collect())
        .unwrap_or_default();
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| member_ids.contains(&&p["id"]))
        .collect()
}

/// One message for each direct dependency of consumer `name` that is an engine
/// crate other than the facade.
fn dependency_errors(name: &str, package: &Value, is_engine: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut errors = Vec::new();
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
    errors
}

/// The `engine-crates` list from the workspace metadata.
fn engine_crates(metadata: &Value) -> Result<Vec<String>, Vec<String>> {
    string_list(metadata, "engine-crates")
}

/// The string list `key` under `[workspace.metadata.voxulacrum]`.
fn string_list(metadata: &Value, key: &str) -> Result<Vec<String>, Vec<String>> {
    let Some(list) = metadata["metadata"]["voxulacrum"][key].as_array() else {
        return Err(vec![format!(
            "no `{key}` list under `[workspace.metadata.voxulacrum]` in the root Cargo.toml"
        )]);
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

    /// Metadata for an outside workspace whose one member, `game`, has `deps`,
    /// with the facade among its other packages.
    fn outside(deps: Vec<Value>) -> Value {
        json!({
            "packages": [
                { "name": "game", "id": "path+file:///game#0.1.0", "dependencies": deps },
                { "name": "voxulacrum", "id": "git+https://example/voxulacrum#0.4.2", "dependencies": [] },
            ],
            "workspace_members": ["path+file:///game#0.1.0"],
            "metadata": null,
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

    #[test]
    fn an_outside_consumer_on_the_facade_passes() {
        let engine = workspace(json!(ENGINE), vec![]);
        let summary = check_consumers(&outside(vec![dep("voxulacrum", None)]), &engine).unwrap();
        assert_eq!(summary, Summary { engine_crates: 3, consumers: vec!["game".to_string()] });
    }

    #[test]
    fn an_outside_consumer_on_an_engine_crate_fails() {
        let engine = workspace(json!(ENGINE), vec![]);
        let game = outside(vec![dep("voxulacrum", None), dep("voxel-core", None)]);
        let errors = check_consumers(&game, &engine).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("game: depends on voxel-core"), "{errors:?}");
    }

    #[test]
    fn an_outside_check_needs_the_engine_list() {
        let mut engine = workspace(json!(ENGINE), vec![]);
        engine["metadata"] = Value::Null;
        let errors = check_consumers(&outside(vec![]), &engine).unwrap_err();
        assert!(errors[0].contains("no `engine-crates` list"), "{errors:?}");
    }

    const FORBIDDEN: [&str; 6] = ["wgpu", "naga", "winit", "raw-window-handle", "egui", "eframe"];

    /// An edge in [`resolved`]: the package depended on, its kind (`None` for
    /// normal) and its target (`None` for every target).
    type Edge<'a> = (&'a str, Option<&'a str>, Option<&'a str>);

    /// Full metadata for a workspace whose one member and one server crate is
    /// `server`, with the resolve graph `graph`: each package and its edges.
    fn resolved(graph: Vec<(&str, Vec<Edge>)>) -> Value {
        fn id(name: &str) -> String {
            format!("registry+https://example/{name}#1.0.0")
        }
        let packages: Vec<Value> =
            graph.iter().map(|(name, _)| json!({ "name": name, "id": id(name) })).collect();
        let nodes: Vec<Value> = graph
            .iter()
            .map(|(name, edges)| {
                let deps: Vec<Value> = edges
                    .iter()
                    .map(|(dep, kind, target)| {
                        json!({
                            "name": dep.replace('-', "_"),
                            "pkg": id(dep),
                            "dep_kinds": [{ "kind": kind, "target": target }],
                        })
                    })
                    .collect();
                json!({ "id": id(name), "deps": deps })
            })
            .collect();
        json!({
            "packages": packages,
            "workspace_members": [id("server")],
            "resolve": { "nodes": nodes, "root": null },
            "metadata": {
                "voxulacrum": { "server-crates": ["server"], "server-forbidden": FORBIDDEN },
            },
        })
    }

    fn one_server_error(metadata: &Value) -> String {
        let errors = check_server(metadata).expect_err("check should fail");
        assert_eq!(errors.len(), 1, "{errors:?}");
        errors.into_iter().next().unwrap()
    }

    #[test]
    fn a_server_crate_clear_of_the_forbidden_crates_passes() {
        let ws = resolved(vec![
            ("server", vec![("glam", None, None)]),
            ("glam", vec![("libm", None, None)]),
            ("libm", vec![]),
        ]);
        assert_eq!(check_server(&ws).unwrap(), vec!["server".to_string()]);
    }

    #[test]
    fn a_transitive_forbidden_dependency_fails_and_names_the_chain() {
        let ws = resolved(vec![
            ("server", vec![("render-lib", None, None)]),
            ("render-lib", vec![("wgpu-core", None, None)]),
            ("wgpu-core", vec![]),
        ]);
        let error = one_server_error(&ws);
        // `wgpu-core` matches the `wgpu` entry by prefix.
        assert!(
            error.starts_with("server: reaches wgpu-core (server -> render-lib -> wgpu-core)"),
            "{error}"
        );
    }

    #[test]
    fn a_build_dependency_reaching_a_forbidden_crate_fails() {
        let ws = resolved(vec![
            ("server", vec![("codegen", Some("build"), None)]),
            ("codegen", vec![("winit", None, None)]),
            ("winit", vec![]),
        ]);
        let error = one_server_error(&ws);
        assert!(error.contains("(server -> codegen (build) -> winit)"), "{error}");
    }

    #[test]
    fn a_target_specific_forbidden_dependency_fails() {
        let linux_only = r#"cfg(target_os = "linux")"#;
        let ws = resolved(vec![("server", vec![("winit", None, Some(linux_only))]), ("winit", vec![])]);
        let error = one_server_error(&ws);
        assert!(error.contains(r#"winit (normal, cfg(target_os = "linux"))"#), "{error}");
    }

    #[test]
    fn a_dev_dependency_on_a_forbidden_crate_passes() {
        let ws = resolved(vec![("server", vec![("winit", Some("dev"), None)]), ("winit", vec![])]);
        assert_eq!(check_server(&ws).unwrap(), vec!["server".to_string()]);
    }

    #[test]
    fn a_forbidden_crate_reached_two_ways_is_reported_once() {
        let ws = resolved(vec![
            ("server", vec![("a", None, None), ("b", None, None)]),
            ("a", vec![("winit", None, None)]),
            ("b", vec![("winit", None, None)]),
            ("winit", vec![]),
        ]);
        assert!(one_server_error(&ws).contains("(server -> a -> winit)"));
    }

    #[test]
    fn an_unlisted_server_crate_fails() {
        let mut ws = resolved(vec![("server", vec![])]);
        ws["metadata"]["voxulacrum"]["server-crates"] = json!(["server", "old-server"]);
        assert!(one_server_error(&ws).contains("`old-server`"));
    }

    #[test]
    fn metadata_without_the_resolve_graph_fails() {
        let mut ws = resolved(vec![("server", vec![])]);
        ws["resolve"] = Value::Null;
        assert!(one_server_error(&ws).contains("no resolve graph"));
    }

    #[test]
    fn missing_server_lists_fail() {
        for key in ["server-crates", "server-forbidden"] {
            let mut ws = resolved(vec![("server", vec![])]);
            ws["metadata"]["voxulacrum"].as_object_mut().unwrap().remove(key);
            assert!(one_server_error(&ws).contains(&format!("no `{key}` list")), "{key}");
        }
    }
}
