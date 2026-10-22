//! which part of the configuration a path belongs to
//!
//! A commit is meant to be about one part of the configuration: one host, one
//! user, what every host shares, the lock file, or the repository around them.
//! The layout says which part a path is in, so the scope is read off the path
//! alone, with one fact from outside it: which hosts there are, since a host's
//! Home Manager entry point sits among a user's files under the host's name.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// one directory per host, each holding that host's NixOS configuration
pub const HOSTS: &str = "hosts";
/// one directory per user, holding that user's Home Manager configuration
const HOME: &str = "home";
/// the NixOS configuration every host shares
const NIXOS: &str = "nixos";
/// the lock file, which changes for reasons of its own
const FLAKE_LOCK: &str = "flake.lock";

/// a part of the configuration
///
/// ordered the way a listing of them reads best: the host and user parts a
/// person edits most come first, the repository around them last
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// `hosts/<host>/**`, and `home/<user>/<host>.nix`, the host's entry
    /// point into each user's Home Manager configuration
    Host(String),
    /// everything else under `home/<user>/`
    User(String),
    /// `nixos/**`
    Nixos,
    /// `flake.lock`
    FlakeLock,
    /// everything else: the flake itself, its exported modules, packages and
    /// overlays, tooling and documentation
    Repo,
}

impl Scope {
    /// the part of the configuration `path`, relative to the root of the
    /// repository, belongs to, given the names of the hosts
    ///
    /// `home/<user>/<name>.nix` is a host's only when `name` is one of
    /// `hosts`; any other file of that shape stays the user's, such as a
    /// configuration for Home Manager on its own.
    pub fn of(path: &str, hosts: &BTreeSet<String>) -> Self {
        if path == FLAKE_LOCK {
            return Self::FlakeLock;
        }

        let parts: Vec<&str> = path.split('/').collect();

        match parts.as_slice() {
            // a file directly under `hosts/` belongs to no host
            [HOSTS, host, _, ..] => Self::Host((*host).to_owned()),
            [NIXOS, _, ..] => Self::Nixos,
            [HOME, user, file] => match file.strip_suffix(".nix") {
                Some(host) if hosts.contains(host) => Self::Host(host.to_owned()),
                _ => Self::User((*user).to_owned()),
            },
            [HOME, user, _, _, ..] => Self::User((*user).to_owned()),
            _ => Self::Repo,
        }
    }

    /// what kind of part this is, said beside its name in a listing
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Host(_) => "host",
            Self::User(_) => "user",
            Self::Nixos => "shared by every host",
            Self::FlakeLock => "lock file",
            Self::Repo => "repository",
        }
    }
}

/// `paths` grouped by the part of the configuration each belongs to, given
/// the names of the hosts
pub fn group(paths: Vec<String>, hosts: &BTreeSet<String>) -> BTreeMap<Scope, Vec<String>> {
    let mut scopes: BTreeMap<Scope, Vec<String>> = BTreeMap::new();
    for path in paths {
        scopes
            .entry(Scope::of(&path, hosts))
            .or_default()
            .push(path);
    }

    scopes
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(name) | Self::User(name) => f.write_str(name),
            Self::Nixos => f.write_str(NIXOS),
            Self::FlakeLock => f.write_str(FLAKE_LOCK),
            Self::Repo => f.write_str("repo"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> BTreeSet<String> {
        ["fw12", "fw16"].map(str::to_owned).into()
    }

    fn scope(path: &str) -> Scope {
        Scope::of(path, &hosts())
    }

    #[test]
    fn hosts_own_their_directory() {
        assert_eq!(scope("hosts/fw16/default.nix"), Scope::Host("fw16".into()));
        assert_eq!(scope("hosts/fw12/secrets.yaml"), Scope::Host("fw12".into()));
    }

    #[test]
    fn a_new_host_is_a_host_before_it_is_listed() {
        assert_eq!(scope("hosts/fwd/default.nix"), Scope::Host("fwd".into()));
    }

    #[test]
    fn a_file_directly_under_hosts_is_no_host() {
        assert_eq!(scope("hosts/README.md"), Scope::Repo);
    }

    #[test]
    fn hosts_own_their_home_entry_points() {
        assert_eq!(scope("home/f/fw16.nix"), Scope::Host("fw16".into()));
        assert_eq!(scope("home/k/fw12.nix"), Scope::Host("fw12".into()));
    }

    #[test]
    fn other_home_files_are_the_user_s() {
        assert_eq!(scope("home/f/global/default.nix"), Scope::User("f".into()));
        assert_eq!(
            scope("home/f/features/desktop/niri.kdl"),
            Scope::User("f".into())
        );
        assert_eq!(scope("home/f/generic.nix"), Scope::User("f".into()));
        assert_eq!(scope("home/f/ssh.pub"), Scope::User("f".into()));
    }

    #[test]
    fn a_file_directly_under_home_is_no_user() {
        assert_eq!(scope("home/README.md"), Scope::Repo);
    }

    #[test]
    fn nixos_is_shared() {
        assert_eq!(scope("nixos/global/nix.nix"), Scope::Nixos);
        assert_eq!(scope("nixos/optional/bluetooth.nix"), Scope::Nixos);
    }

    #[test]
    fn the_lock_file_is_its_own() {
        assert_eq!(scope("flake.lock"), Scope::FlakeLock);
    }

    #[test]
    fn the_rest_is_the_repository() {
        for path in [
            "flake.nix",
            "modules/nixos/impermanence.nix",
            "pkgs/default.nix",
            "prek.toml",
            "README.md",
            ".sops.yaml",
        ] {
            assert_eq!(scope(path), Scope::Repo, "{path}");
        }
    }
}
