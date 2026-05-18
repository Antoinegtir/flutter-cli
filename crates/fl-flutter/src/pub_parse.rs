//! Parsers for `flutter pub` plain-text output.

use fl_core::{OutdatedRow, PubDepKind, PubEvent, PubTreeNode};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Parse `flutter pub get` / `pub upgrade` stdout. Returns a `Got` event.
/// Lines we look for:
///   `+ package_name 1.0.0`      → added
///   `- package_name`             → removed
///   `> package_name 1.0.0 (was 0.9.0)` → modified
pub fn parse_pub_get(stdout: &str) -> PubEvent {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    for line in stdout.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("+ ") {
            let mut parts = rest.split_whitespace();
            if let Some(name) = parts.next() {
                added.push(name.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("- ") {
            let mut parts = rest.split_whitespace();
            if let Some(name) = parts.next() {
                removed.push(name.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("> ") {
            // > foo 1.0.0 (was 0.9.0)
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("").to_string();
            let new_v = parts.next().unwrap_or("").to_string();
            let was = rest.split_once("(was ").and_then(|(_, r)| r.strip_suffix(')')).unwrap_or("").to_string();
            modified.push((name, was, new_v));
        }
    }
    PubEvent::Got { added, removed, modified }
}

/// Parse the output of `flutter pub outdated`.
/// The table has 5 columns:
///   Package Name   Current   Upgradable   Resolvable   Latest
/// We split on whitespace runs and skip header / separator lines.
pub fn parse_outdated_table(stdout: &str) -> Vec<OutdatedRow> {
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in stdout.lines() {
        if line.trim_start().starts_with("Package Name") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // Skip section headers like "direct dependencies:" that Flutter prints.
        let trimmed = line.trim_start();
        if trimmed.ends_with(':') && !trimmed.contains("  ") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 5 {
            let n = fields.len();
            let package = fields[..n - 4].join(" ");
            rows.push(OutdatedRow {
                package,
                current: fields[n - 4].to_string(),
                upgradable: fields[n - 3].to_string(),
                resolvable: fields[n - 2].to_string(),
                latest: fields[n - 1].to_string(),
            });
        }
    }
    rows
}

/// Parse `flutter pub deps --json` and return a tree rooted at the project.
pub fn parse_deps_json(json: &str) -> anyhow::Result<PubTreeNode> {
    let v: Value = serde_json::from_str(json).map_err(|e| anyhow::anyhow!("invalid deps json: {e}"))?;
    let root_name = v.get("root").and_then(Value::as_str).unwrap_or("root").to_string();

    let packages = v.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    let pkg_map: HashMap<String, Value> = packages
        .iter()
        .filter_map(|p| p.get("name").and_then(Value::as_str).map(|n| (n.to_string(), p.clone())))
        .collect();

    let direct: HashSet<String> = v
        .get("directDependencies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let dev: HashSet<String> = v
        .get("devDependencies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let root_node = build_node(&root_name, "0.0.0", PubDepKind::Direct, &pkg_map, &direct, &dev, &mut HashSet::new());
    Ok(root_node)
}

fn build_node(
    name: &str,
    fallback_version: &str,
    kind: PubDepKind,
    pkg_map: &HashMap<String, Value>,
    direct: &HashSet<String>,
    dev: &HashSet<String>,
    visited: &mut HashSet<String>,
) -> PubTreeNode {
    let version = pkg_map.get(name).and_then(|p| p.get("version")).and_then(Value::as_str).unwrap_or(fallback_version).to_string();
    let mut children = Vec::new();
    if !visited.insert(name.to_string()) {
        return PubTreeNode { name: name.to_string(), version, kind, children };
    }
    let deps = pkg_map
        .get(name)
        .and_then(|p| p.get("dependencies"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for d in deps {
        let Some(dn) = d.as_str() else { continue };
        let dn = dn.to_string();
        let child_kind = if direct.contains(&dn) {
            PubDepKind::Direct
        } else if dev.contains(&dn) {
            PubDepKind::Dev
        } else {
            PubDepKind::Transitive
        };
        children.push(build_node(&dn, "0.0.0", child_kind, pkg_map, direct, dev, visited));
    }
    PubTreeNode { name: name.to_string(), version, kind, children }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fl_core::PubEvent;

    #[test]
    fn parses_added_removed_modified() {
        let input = "\
Resolving dependencies...
+ shiny_new_pkg 1.0.0
- legacy_pkg
> updated_pkg 2.0.0 (was 1.9.0)
Got dependencies!
";
        match parse_pub_get(input) {
            PubEvent::Got { added, removed, modified } => {
                assert_eq!(added, vec!["shiny_new_pkg".to_string()]);
                assert_eq!(removed, vec!["legacy_pkg".to_string()]);
                assert_eq!(modified.len(), 1);
                assert_eq!(modified[0], ("updated_pkg".into(), "1.9.0".into(), "2.0.0".into()));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn empty_output_returns_empty_vecs() {
        match parse_pub_get("") {
            PubEvent::Got { added, removed, modified } => {
                assert!(added.is_empty() && removed.is_empty() && modified.is_empty());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_outdated_table() {
        let input = "\
Showing outdated packages.

Package Name      Current   Upgradable  Resolvable  Latest

direct dependencies:
http              0.13.5    0.13.6      0.14.0      1.2.0
provider          6.0.5     6.0.5       6.1.1       6.1.1

dev dependencies:
flutter_test      sdk       sdk         sdk         sdk
";
        let rows = parse_outdated_table(input);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].package, "http");
        assert_eq!(rows[0].current, "0.13.5");
        assert_eq!(rows[0].latest, "1.2.0");
        assert_eq!(rows[2].package, "flutter_test");
    }

    #[test]
    fn parses_deps_json_tree() {
        let json = r#"{
            "root": "myapp",
            "directDependencies": ["http", "provider"],
            "devDependencies": ["flutter_test"],
            "packages": [
                {"name": "myapp", "version": "1.0.0", "dependencies": ["http", "provider", "flutter_test"]},
                {"name": "http", "version": "0.13.5", "dependencies": ["http_parser"]},
                {"name": "http_parser", "version": "4.0.0", "dependencies": []},
                {"name": "provider", "version": "6.0.5", "dependencies": []},
                {"name": "flutter_test", "version": "sdk", "dependencies": []}
            ]
        }"#;
        let tree = parse_deps_json(json).unwrap();
        assert_eq!(tree.name, "myapp");
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"http"));
        assert!(names.contains(&"provider"));
        assert!(names.contains(&"flutter_test"));
        let http = tree.children.iter().find(|c| c.name == "http").unwrap();
        assert_eq!(http.kind, PubDepKind::Direct);
        let dev = tree.children.iter().find(|c| c.name == "flutter_test").unwrap();
        assert_eq!(dev.kind, PubDepKind::Dev);
        let transitive = http.children.first().unwrap();
        assert_eq!(transitive.kind, PubDepKind::Transitive);
        assert_eq!(transitive.name, "http_parser");
    }
}
