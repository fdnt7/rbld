//! what changed between two versions of a flake's lock file
//!
//! nix prints what `nix flake update` changed, but for a terminal: wrapped
//! to its width, behind a progress bar, and in a format of its own choosing.
//! The lock files on either side say the same thing in a format that is
//! versioned and meant to be read, so the summary is taken from them instead,
//! which also covers a lock file that changed for any other reason, such as
//! an input added to the flake.

use std::{collections::BTreeMap, fmt::Write as _};

use {anyhow::Context as _, chrono::DateTime, serde::Deserialize};

/// how wide a commit body may run
const BODY_WIDTH: usize = 72;

/// how many characters of a revision name it, as git abbreviates one
const SHORT_REV: usize = 7;

#[derive(Deserialize)]
struct Lock {
    nodes: BTreeMap<String, Node>,
    root: String,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    inputs: BTreeMap<String, Target>,
    locked: Option<Locked>,
}

/// where an input points: a node of its own, or another input it follows,
/// named by its path from the root
#[derive(Deserialize)]
#[serde(untagged)]
enum Target {
    Node(String),
    Follows(Vec<String>),
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Locked {
    #[serde(rename = "type")]
    kind: String,
    owner: Option<String>,
    repo: Option<String>,
    url: Option<String>,
    path: Option<String>,
    rev: Option<String>,
    nar_hash: Option<String>,
    last_modified: Option<i64>,
}

/// what an input resolves to, as far as the summary is concerned
#[derive(Clone, PartialEq, Eq)]
enum Input {
    Locked(Locked),
    /// the path, from the root, of the input this one follows; empty for the
    /// flake itself
    Follows(String),
}

impl Locked {
    /// where the input comes from, without the revision
    fn source(&self) -> String {
        match (&self.owner, &self.repo, &self.url, &self.path) {
            (Some(owner), Some(repo), ..) => format!("{}:{owner}/{repo}", self.kind),
            (_, _, Some(url), _) => url.clone(),
            (_, _, _, Some(path)) => path.clone(),
            _ => self.kind.clone(),
        }
    }

    /// the revision it is locked to, abbreviated, and the date it was last
    /// changed, as far as the lock file records either
    fn version(&self) -> String {
        let rev = self
            .rev
            .as_deref()
            .map(|rev| rev.chars().take(SHORT_REV).collect::<String>());
        let date = self
            .last_modified
            .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
            .map(|time| time.format("%Y-%m-%d").to_string());

        match (rev, date) {
            (Some(rev), Some(date)) => format!("{rev} ({date})"),
            (Some(rev), None) => rev,
            (None, Some(date)) => format!("({date})"),
            (None, None) => "unversioned".to_owned(),
        }
    }
}

impl Input {
    /// the input on its own, for one that was added or removed
    fn describe(&self) -> String {
        match self {
            Self::Locked(locked) => format!("{} {}", locked.source(), locked.version()),
            Self::Follows(path) => follows(path),
        }
    }

    /// the input as it was and as it is, naming the source only where the two
    /// differ, since an update mostly moves the revision alone
    fn change_to(&self, new: &Self) -> String {
        match (self, new) {
            (Self::Locked(old), Self::Locked(new)) if old.source() == new.source() => {
                format!("{} -> {}", old.version(), new.version())
            }
            _ => format!("{} -> {}", self.describe(), new.describe()),
        }
    }
}

fn follows(path: &str) -> String {
    if path.is_empty() {
        "follows the flake itself".to_owned()
    } else {
        format!("follows {path}")
    }
}

impl Lock {
    fn parse(text: &str) -> anyhow::Result<Self> {
        serde_json::from_str(text).context("reading flake.lock")
    }

    /// every input, direct or not, by its path from the root
    fn inputs(&self) -> BTreeMap<String, Input> {
        let mut inputs = BTreeMap::new();
        self.walk(&self.root, "", &mut inputs, 0);
        inputs
    }

    fn walk(&self, node: &str, prefix: &str, inputs: &mut BTreeMap<String, Input>, depth: usize) {
        // a lock file is a tree with shared nodes, never a cycle, but this
        // stays finite whatever it is handed
        if depth > 64 {
            return;
        }

        let Some(node) = self.nodes.get(node) else {
            return;
        };

        for (name, target) in &node.inputs {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };

            match target {
                Target::Follows(to) => {
                    inputs.insert(path, Input::Follows(to.join("/")));
                }
                Target::Node(key) => {
                    if let Some(locked) = self.nodes.get(key).and_then(|n| n.locked.clone()) {
                        inputs.insert(path.clone(), Input::Locked(locked));
                    }
                    self.walk(key, &path, inputs, depth + 1);
                }
            }
        }
    }
}

/// the body of a commit that takes the lock file from `old` to `new`: the
/// inputs updated, added and removed, one list each, wrapped to fit a commit
/// body; empty where no input changed
pub fn summarise(old: &str, new: &str) -> anyhow::Result<String> {
    let (old, new) = (Lock::parse(old)?.inputs(), Lock::parse(new)?.inputs());

    let updated: Vec<String> = new
        .iter()
        .filter_map(|(path, input)| {
            let was = old.get(path)?;
            (was != input).then(|| format!("{path}: {}", was.change_to(input)))
        })
        .collect();
    let added: Vec<String> = new
        .iter()
        .filter(|(path, _)| !old.contains_key(*path))
        .map(|(path, input)| format!("{path}: {}", input.describe()))
        .collect();
    let removed: Vec<String> = old
        .keys()
        .filter(|path| !new.contains_key(*path))
        .cloned()
        .collect();

    let mut body = String::new();

    for (heading, items) in [("Updated", updated), ("Added", added), ("Removed", removed)] {
        if items.is_empty() {
            continue;
        }

        if !body.is_empty() {
            body.push('\n');
        }

        let _ = writeln!(body, "{heading}:");
        for item in items {
            body.push_str(&wrap(&format!("- {item}")));
        }
    }

    Ok(body.trim_end().to_owned())
}

/// a list item, wrapped to the body's width, its later lines indented to sit
/// under its text rather than its marker
fn wrap(item: &str) -> String {
    let mut lines = vec![];
    let mut line = String::new();

    for word in item.split(' ') {
        if !line.is_empty() && line.len() + 1 + word.len() > BODY_WIDTH {
            lines.push(std::mem::take(&mut line));
            line.push_str("  ");
        }

        if !line.is_empty() && !line.ends_with("  ") {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);

    lines.into_iter().map(|line| line + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(nodes: &str) -> String {
        format!(r#"{{ "nodes": {{ {nodes} }}, "root": "root", "version": 7 }}"#)
    }

    fn github(repo: &str, rev: &str, last_modified: i64) -> String {
        format!(
            r#"{{ "locked": {{ "type": "github", "owner": "o", "repo": "{repo}", "rev": "{rev}", "narHash": "sha256-x", "lastModified": {last_modified} }} }}"#
        )
    }

    #[test]
    fn nothing_changed_says_nothing() {
        let a = lock(&format!(
            r#""root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }}, "nixpkgs": {}"#,
            github(
                "nixpkgs",
                "0954f7ee2f6bb3dc7d4e3d0d8bcb8fd4bde4cfc5",
                1_785_318_670
            )
        ));

        assert_eq!(summarise(&a, &a).unwrap_or_default(), "");
    }

    #[test]
    fn an_update_names_the_revisions_and_dates() {
        let old = lock(&format!(
            r#""root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }}, "nixpkgs": {}"#,
            github(
                "nixpkgs",
                "0954f7ee2f6bb3dc7d4e3d0d8bcb8fd4bde4cfc5",
                1_785_318_670
            )
        ));
        let new = lock(&format!(
            r#""root": {{ "inputs": {{ "nixpkgs": "nixpkgs" }} }}, "nixpkgs": {}"#,
            github(
                "nixpkgs",
                "02c087f3fe673c9e317f206a627cb6b910336dbe",
                1_792_592_221
            )
        ));

        assert_eq!(
            summarise(&old, &new).unwrap_or_default(),
            "Updated:\n- nixpkgs: 0954f7e (2026-07-29) -> 02c087f (2026-10-21)"
        );
    }

    #[test]
    fn nested_inputs_and_follows_are_named_by_path() {
        let old = lock(r#""root": { "inputs": {} }"#);
        let new = lock(&format!(
            r#""root": {{ "inputs": {{ "devshell": "devshell", "nixpkgs": "nixpkgs" }} }},
               "devshell": {{ "inputs": {{ "nixpkgs": ["nixpkgs"], "treefmt-nix": "treefmt-nix" }}, "locked": {{ "type": "github", "owner": "fdnt7", "repo": "devshell", "rev": "a7e2c2129d30d3114118f45144e3ef34590e0526", "lastModified": 1792592221 }} }},
               "treefmt-nix": {}, "nixpkgs": {}"#,
            github(
                "treefmt-nix",
                "0d6d739a833ba9a9c93034862e4a7b061a062a1c",
                1_791_885_023
            ),
            github(
                "nixpkgs",
                "02c087f3fe673c9e317f206a627cb6b910336dbe",
                1_792_592_221
            )
        ));

        assert_eq!(
            summarise(&old, &new).unwrap_or_default(),
            "Added:\n\
             - devshell: github:fdnt7/devshell a7e2c21 (2026-10-21)\n\
             - devshell/nixpkgs: follows nixpkgs\n\
             - devshell/treefmt-nix: github:o/treefmt-nix 0d6d739 (2026-10-13)\n\
             - nixpkgs: github:o/nixpkgs 02c087f (2026-10-21)"
        );
    }

    #[test]
    fn removals_are_listed_by_name() {
        let old = lock(&format!(
            r#""root": {{ "inputs": {{ "rust-overlay": "rust-overlay" }} }}, "rust-overlay": {}"#,
            github(
                "rust-overlay",
                "1111111111111111111111111111111111111111",
                1_785_318_670
            )
        ));
        let new = lock(r#""root": { "inputs": {} }"#);

        assert_eq!(
            summarise(&old, &new).unwrap_or_default(),
            "Removed:\n- rust-overlay"
        );
    }

    #[test]
    fn a_long_item_wraps_under_its_text() {
        let item = wrap(&format!(
            "- {}: follows {}",
            "a/".repeat(20),
            "b/".repeat(20)
        ));

        for line in item.lines() {
            assert!(line.len() <= BODY_WIDTH, "{line}");
        }
        assert!(item.lines().skip(1).all(|line| line.starts_with("  ")));
    }
}
