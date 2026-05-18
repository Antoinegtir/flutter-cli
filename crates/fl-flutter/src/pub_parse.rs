//! Parsers for `flutter pub` plain-text output.

use fl_core::PubEvent;

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
}
