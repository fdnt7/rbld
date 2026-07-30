use std::process::ExitCode;

use {
    crate::util::{
        git,
        render::{DIR, DISALLOWED, HEADLINE, NOTE, file_path, paint, rooted},
        terminal::refuse,
    },
    anstyle::{AnsiColor, Style},
    git2::Repository,
    termtree::Tree,
};

const HOST_DIR: &str = "hosts";
/// shared changes sit under no host at all, so they borrow pathspec's exclude
/// syntax to keep every heading path-shaped
const SHARED_HEADER: &str = ":!hosts";

const OTHER_HOST: Style = AnsiColor::Red.on_default();
const THIS_HOST: Style = AnsiColor::Green.on_default();
const SHARED: Style = AnsiColor::Yellow.on_default();

/// everything else sits where it belongs, so its files carry no marking
const ALLOWED: Style = Style::new();

impl crate::cli::Cli {
    pub(in crate::cli) fn check_boundaries(self) -> anyhow::Result<ExitCode> {
        let system = hostname::get()?;
        let system_dir = format!("{HOST_DIR}/{}", system.to_string_lossy());
        let (system_prefix, host_prefix) = (format!("{system_dir}/"), format!("{HOST_DIR}/"));
        let repo = Repository::open(&self.flake)?;

        let (mut host_paths, mut other_host_paths, mut shared_paths) = (vec![], vec![], vec![]);

        for path in git::changed_paths(&repo)? {
            if path.starts_with(&system_prefix) {
                host_paths.push(path);
            } else if path.starts_with(&host_prefix) {
                other_host_paths.push(path);
            } else {
                shared_paths.push(path);
            }
        }

        // the host name is quoted as well as coloured, so it still stands out
        // of the prose once prek captures this hook and strips the styling
        let headline = match (
            host_paths.is_empty(),
            other_host_paths.is_empty(),
            shared_paths.is_empty(),
        ) {
            (_, false, _) => format!(
                "{} {}",
                paint(HEADLINE, "this commit touches a host other than"),
                paint(THIS_HOST.bold(), &format!("`{}`", system.to_string_lossy()))
            ),
            (false, true, false) => paint(
                HEADLINE,
                "this commit mixes host-specific and shared changes",
            ),
            _ => return Ok(ExitCode::SUCCESS),
        };

        let mut tree = rooted(git::workdir(&repo, &self.flake));

        // foreign hosts come first, since they are what makes the commit an
        // error at all; the notes repeat what the colours say, because prek
        // captures this hook and the styling is stripped when it does
        for (label, note, paths) in group_by_host(&other_host_paths) {
            tree.push(group(&label, note, OTHER_HOST, DISALLOWED, paths));
        }
        if !host_paths.is_empty() {
            let paths = host_paths.iter().map(String::as_str);
            tree.push(group(&system_dir, "this host", THIS_HOST, ALLOWED, paths));
        }
        if !shared_paths.is_empty() {
            let paths = shared_paths.iter().map(String::as_str);
            tree.push(group(SHARED_HEADER, "shared", SHARED, ALLOWED, paths));
        }

        refuse(headline, tree);

        Ok(ExitCode::FAILURE)
    }
}

/// a group of changed paths, listed relative to whatever the group already names
fn group<'a>(
    label: &str,
    note: &str,
    heading_style: Style,
    file_style: Style,
    paths: impl IntoIterator<Item = &'a str>,
) -> Tree<String> {
    let root = format!(
        "{} {}",
        heading(heading_style, label),
        paint(NOTE, &format!("({note})"))
    );
    let leaves = paths
        .into_iter()
        .map(|path| Tree::new(file_path(file_style, &relative_to(label, path))));

    Tree::new(root).with_leaves(leaves)
}

/// a heading names a directory, so all of it is bold, but only the host name
/// carries the colour: `hosts/` says nothing about which host it is
fn heading(style: Style, label: &str) -> String {
    let host = label
        .strip_prefix(HOST_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|host| !host.is_empty());

    host.map_or_else(
        || paint(style.bold(), label),
        |host| {
            format!(
                "{}{}",
                paint(DIR, &format!("{HOST_DIR}/")),
                paint(style.bold(), host)
            )
        },
    )
}

/// one group per foreign host, so each is listed under its own heading; a file
/// sitting directly under `hosts/` belongs to no host, so those share a group
fn group_by_host(paths: &[String]) -> Vec<(String, &'static str, Vec<&str>)> {
    let mut groups: Vec<(String, &'static str, Vec<&str>)> = Vec::new();

    for path in paths.iter().map(String::as_str) {
        let rest = path
            .strip_prefix(HOST_DIR)
            .and_then(|r| r.strip_prefix('/'));
        let (heading, note) = match rest.and_then(|rest| rest.split_once('/')) {
            Some((name, _)) => (format!("{HOST_DIR}/{name}"), "other host"),
            None => (format!("{HOST_DIR}/"), "not a host"),
        };

        match groups.iter_mut().find(|(host, ..)| *host == heading) {
            Some((.., group)) => group.push(path),
            None => groups.push((heading, note, vec![path])),
        }
    }

    groups
}

/// trims the part of a path its group heading already spells out, falling back
/// to `hosts/` for a file sitting directly under it
fn relative_to(label: &str, path: &str) -> String {
    path.strip_prefix(label)
        .and_then(|rest| rest.strip_prefix('/'))
        .or_else(|| {
            path.strip_prefix(HOST_DIR)
                .and_then(|rest| rest.strip_prefix('/'))
        })
        .unwrap_or(path)
        .to_owned()
}
